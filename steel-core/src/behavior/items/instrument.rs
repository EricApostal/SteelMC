//! Goat horn instrument item behavior implementation.
//!
//! Mirrors vanilla `InstrumentItem`: the horn only works while the stack keeps
//! a `minecraft:instrument` component; playing it starts the item's active use,
//! plays the instrument sound, damages the horn and applies the instrument's
//! cooldown.
//!
//! **Vanilla differences:** the instrument's `durability_damage` and the
//! `ItemCooldowns.addCooldown(stack, ticks)` entry point are missing (see the
//! module comments in the play path), so the horn neither self-damages nor
//! applies its own cooldown.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::INSTRUMENT;
use steel_registry::instrument::InstrumentValue;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_events;

use crate::behavior::{InteractionResult, ItemBehavior, ItemUseAnimation, UseItemContext};
use crate::entity::{Entity, LivingEntity};
use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Behavior for instrument items (currently only the goat horn).
#[item_behavior]
pub struct InstrumentItem;

impl InstrumentItem {
    /// Plays the held instrument at the player's own position and awards the
    /// vanilla `INSTRUMENT_PLAY` game event.
    ///
    /// Vanilla plays through `Level.playSound(player, player, ...)`, so the
    /// player is excluded from the broadcast and receives the sound client-side.
    fn play(world: &Arc<World>, player: &Player, instrument: &InstrumentValue) {
        let volume = instrument.range() / 16.0;
        world.play_sound_holder_at(
            instrument.sound_event(),
            SoundSource::Records,
            player.position(),
            volume,
            1.0,
            Some(player.id()),
        );
        world.game_event(
            &vanilla_game_events::INSTRUMENT_PLAY,
            player.block_position(),
            &GameEventContext::new(Some(player as &dyn Entity), None),
        );
    }
}

impl ItemBehavior for InstrumentItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let player: &Arc<Player> = context.player;
        let world: &Arc<World> = context.world;

        // Vanilla reads the instrument off the held stack first and fails the
        // use outright when the component is absent.
        if context
            .inv
            .with_item(|stack| stack.get(INSTRUMENT).is_none())
        {
            return InteractionResult::Fail;
        }

        // Vanilla `InstrumentItem.use` order: start the use, then play.
        context.player.start_using_item(context.hand);
        context.inv.with_item(|stack| {
            if let Some(component) = stack.get(INSTRUMENT) {
                Self::play(world, player, component.value());
            }
        });

        // Vanilla additionally applies `instrument.durabilityDamage()` here and
        // starts `floor(useDuration * 20)` ticks of cooldown. Steel's
        // `InstrumentValue` carries no `durability_damage` field
        // (`steel-registry/src/instrument.rs:22-27`) and `ItemCooldowns` exposes only
        // `add_from_stack` (`steel-core/src/player/item_cooldowns.rs:41`), so both
        // effects are blocked on those APIs.

        let item = context.inv.with_item(|stack| stack.item());
        player.award_stat(&vanilla_stat_types::ITEM_USED, item);
        InteractionResult::Consume
    }

    fn get_use_duration(&self, stack: &ItemStack, _user: &dyn LivingEntity) -> i32 {
        stack.get(INSTRUMENT).map_or(0, |component| {
            (component.value().use_duration() * 20.0).floor() as i32
        })
    }

    fn get_use_animation(&self, _stack: &ItemStack) -> ItemUseAnimation {
        ItemUseAnimation::TootHorn
    }
}
