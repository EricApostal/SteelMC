//! Bundle item behavior implementation.
//!
//! Mirrors vanilla `BundleItem`'s active-use half: holding use starts a 200-tick
//! use that drops one contained stack immediately and then one every two ticks
//! after the first ten ticks.
//!
//! **Vanilla differences:** the inventory click handlers
//! (`overrideStackedOnOther`/`overrideOtherStackedOnMe`), the durability-style
//! fullness bar (`isBarVisible`/`getBarWidth`/`getBarColor`), the bundle
//! tooltip and `onDestroyed` scattering have no `ItemBehavior` hook in Steel,
//! so they are not implemented here. Drops already honor the selected index:
//! `BundleContents` keeps the selection as runtime-only state (excluded from
//! every serializer) and a drop removes the selected stack, falling back to
//! the first one when the selection is out of bounds.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::components::BundleContents;
use steel_registry::data_components::vanilla_components::BUNDLE_CONTENTS;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;

use crate::behavior::{InteractionResult, ItemBehavior, ItemUseAnimation, UseItemContext};
use crate::entity::{Entity, LivingEntity, LivingEntityRef};
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::World;

/// Vanilla `BundleItem.TICKS_MAX_THROW_DURATION`.
const USE_DURATION: i32 = 200;
/// Vanilla `BundleItem.TICKS_AFTER_FIRST_THROW`.
const TICKS_AFTER_FIRST_THROW: i32 = 10;
/// Vanilla `BundleItem.TICKS_BETWEEN_THROWS`.
const TICKS_BETWEEN_THROWS: i32 = 2;

/// Behavior for the bundle.
#[item_behavior]
pub struct BundleItem;

impl BundleItem {
    /// Vanilla's random bundle pitch: `0.8 + rand * 0.4`.
    fn sound_pitch() -> f32 {
        0.8 + rand::random::<f32>() * 0.4
    }

    /// Mirrors vanilla `BundleItem.removeOneItemFromBundle` + the drop/sound tail
    /// of `dropContent`.
    ///
    /// Returns `true` when a stack was removed from the bundle and dropped.
    fn drop_content(world: &Arc<World>, player: &Player, stack: &mut ItemStack) -> bool {
        let Some(contents) = stack.get(BUNDLE_CONTENTS) else {
            return false;
        };
        let items = contents.items();
        // Vanilla `BundleContents.Mutable.removeOne`: an out-of-bounds (or
        // cleared) selection falls back to the first stored stack, then the
        // selection is cleared.
        let selected = contents.selected_item_index();
        let index = if selected < 0 || selected as usize >= items.len() {
            0
        } else {
            selected as usize
        };
        let Some(removed) = items.get(index) else {
            return false;
        };

        let dropped = removed.create();
        let remaining: Vec<_> = items
            .iter()
            .enumerate()
            .filter(|(slot, _)| *slot != index)
            .map(|(_, item)| item.clone())
            .collect();
        stack.set(BUNDLE_CONTENTS, BundleContents::new(remaining));

        player.play_sound(
            &sound_events::ITEM_BUNDLE_REMOVE_ONE,
            0.8,
            Self::sound_pitch(),
        );
        let _ = player.drop_item(dropped, true, Prediction::Predicted);

        world.play_sound(
            &sound_events::ITEM_BUNDLE_DROP_CONTENTS,
            SoundSource::Players,
            player.block_position(),
            0.8,
            Self::sound_pitch(),
            None,
        );
        true
    }
}

impl ItemBehavior for BundleItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        context.player.start_using_item(context.hand);
        InteractionResult::Success
    }

    fn get_use_duration(&self, _stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        USE_DURATION
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::Bundle
    }

    fn on_use_tick(
        &self,
        world: &Arc<World>,
        user: LivingEntityRef<'_>,
        stack: &mut ItemStack,
        ticks_remaining: i32,
    ) {
        let Some(player) = user.entity().as_player() else {
            return;
        };

        let is_first_tick = ticks_remaining == USE_DURATION;
        if (is_first_tick
            || (ticks_remaining < USE_DURATION - TICKS_AFTER_FIRST_THROW
                && ticks_remaining % TICKS_BETWEEN_THROWS == 0))
            && Self::drop_content(world, player, stack)
        {
            player.award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        }
    }
}
