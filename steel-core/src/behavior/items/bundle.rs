//! Bundle item behavior implementation.
//!
//! Mirrors vanilla `BundleItem`: holding use starts a 200-tick use that drops
//! one contained stack immediately and then one every two ticks after the first
//! ten ticks; clicking a bundle inserts or extracts stacks through
//! `overrideStackedOnOther`/`overrideOtherStackedOnMe`, respecting the weight
//! budget.
//!
//! **Vanilla differences:** the durability-style fullness bar
//! (`isBarVisible`/`getBarWidth`/`getBarColor`), the bundle tooltip and
//! `onDestroyed` scattering have no hook in Steel, so they are not implemented
//! here. Drops already honor the selected index: `BundleContents` keeps the
//! selection as runtime-only state (excluded from every serializer) and a drop
//! removes the selected stack, falling back to the first one when the selection
//! is out of bounds.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::components::BundleContents;
use steel_registry::data_components::vanilla_components::BUNDLE_CONTENTS;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;

use crate::behavior::{
    ITEM_BEHAVIORS, InteractionResult, ItemBehavior, ItemUseAnimation, UseItemContext,
};
use crate::entity::{Entity, LivingEntity, LivingEntityRef};
use crate::inventory::click::ClickAction;
use crate::inventory::lock::ContainerLockGuard;
use crate::inventory::slots::Slot;
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

    /// Vanilla `BundleItem.playInsertSound`.
    fn play_insert_sound(player: &Player) {
        player.play_sound(
            &sound_events::ITEM_BUNDLE_INSERT,
            0.8,
            0.8 + rand::random::<f32>() * 0.4,
        );
    }

    /// Vanilla `BundleItem.playInsertFailSound`.
    fn play_insert_fail_sound(player: &Player) {
        player.play_sound(&sound_events::ITEM_BUNDLE_INSERT_FAIL, 1.0, 1.0);
    }

    /// Vanilla `BundleItem.playRemoveOneSound`.
    fn play_remove_one_sound(player: &Player) {
        player.play_sound(
            &sound_events::ITEM_BUNDLE_REMOVE_ONE,
            0.8,
            Self::sound_pitch(),
        );
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

/// Vanilla `BundleContents.canItemBeInBundle`.
fn can_item_be_in_bundle(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && ITEM_BEHAVIORS
            .get_behavior(stack.item())
            .can_fit_inside_container_items()
}

/// Vanilla `BundleContents.Mutable.tryTransfer`: moves as much of the slot's
/// stack into the bundle as its weight budget allows.
fn try_transfer(
    contents: &mut BundleContents,
    slot: &dyn Slot,
    guard: &mut ContainerLockGuard,
    player: &Player,
) -> i32 {
    let slot_item = slot.get_item(guard).clone();
    let max_amount = contents.max_amount_to_add(&slot_item);
    if !can_item_be_in_bundle(&slot_item) {
        return 0;
    }
    let mut taken = slot.safe_take(guard, slot_item.count(), max_amount, player);
    if taken.is_empty() {
        return 0;
    }
    contents.try_insert(&mut taken)
}

impl ItemBehavior for BundleItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        context.player.start_using_item(context.hand);
        InteractionResult::Success
    }

    fn override_stacked_on_other(
        &self,
        stack: &mut ItemStack,
        slot: &dyn Slot,
        guard: &mut ContainerLockGuard,
        action: ClickAction,
        player: &Player,
    ) -> bool {
        let Some(mut contents) = stack.get(BUNDLE_CONTENTS).cloned() else {
            return false;
        };
        let slot_item = slot.get_item(guard).clone();
        match action {
            ClickAction::Primary if !slot_item.is_empty() => {
                if try_transfer(&mut contents, slot, guard, player) > 0 {
                    Self::play_insert_sound(player);
                } else {
                    Self::play_insert_fail_sound(player);
                }
                stack.set(BUNDLE_CONTENTS, contents);
                true
            }
            ClickAction::Secondary if slot_item.is_empty() => {
                if let Some(removed) = contents.remove_one() {
                    let amount = removed.count();
                    let mut remainder = slot.safe_insert(guard, removed, amount);
                    if remainder.count() > 0 {
                        contents.try_insert(&mut remainder);
                    } else {
                        Self::play_remove_one_sound(player);
                    }
                }
                stack.set(BUNDLE_CONTENTS, contents);
                true
            }
            _ => false,
        }
    }

    fn override_other_stacked_on_me(
        &self,
        stack: &mut ItemStack,
        other: &mut ItemStack,
        slot: &dyn Slot,
        guard: &mut ContainerLockGuard,
        action: ClickAction,
        player: &Player,
    ) -> bool {
        // An empty cursor with a primary click only clears the selection and
        // lets the default pickup run.
        if action == ClickAction::Primary && other.is_empty() {
            if let Some(contents) = stack.get(BUNDLE_CONTENTS).cloned() {
                stack.set(BUNDLE_CONTENTS, contents.with_toggled_selected_item(-1));
            }
            return false;
        }

        let Some(mut contents) = stack.get(BUNDLE_CONTENTS).cloned() else {
            return false;
        };
        match action {
            ClickAction::Primary if !other.is_empty() => {
                if slot.allow_modification(guard, player) && contents.try_insert(other) > 0 {
                    Self::play_insert_sound(player);
                } else {
                    Self::play_insert_fail_sound(player);
                }
                stack.set(BUNDLE_CONTENTS, contents);
                true
            }
            ClickAction::Secondary if other.is_empty() => {
                if slot.allow_modification(guard, player)
                    && let Some(removed) = contents.remove_one()
                {
                    Self::play_remove_one_sound(player);
                    *other = removed;
                }
                stack.set(BUNDLE_CONTENTS, contents);
                true
            }
            _ => {
                stack.set(BUNDLE_CONTENTS, contents.with_toggled_selected_item(-1));
                false
            }
        }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::data_components::components::BundleContents;
    use steel_registry::data_components::vanilla_components::BUNDLE_CONTENTS;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::item_stack_template::ItemStackTemplate;
    use steel_registry::{init_vanilla_registry, vanilla_items};
    use steel_utils::locks::{IntoShared as _, Shared};

    use crate::behavior::init_behaviors;
    use crate::inventory::click::{Click, MouseButton};
    use crate::inventory::container::{Container as _, SimpleContainer};
    use crate::inventory::lock::ContainerRef;
    use crate::inventory::menu::kinds::BasicKind;
    use crate::inventory::menu::{Menu, MenuBuilder};
    use crate::inventory::slots::NormalSlot;
    use crate::player::Player;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    fn bundle_stack(contents: Vec<ItemStack>) -> ItemStack {
        let templates: Vec<ItemStackTemplate> = contents
            .iter()
            .map(|stack| ItemStackTemplate::from_stack(stack).expect("template"))
            .collect();
        let mut bundle = ItemStack::new(&vanilla_items::BUNDLE);
        bundle.set(
            BUNDLE_CONTENTS,
            if templates.is_empty() {
                BundleContents::empty()
            } else {
                BundleContents::new(templates)
            },
        );
        bundle
    }

    fn bundle_total(stack: &ItemStack) -> i32 {
        stack.get(BUNDLE_CONTENTS).map_or(0, |contents| {
            contents.items().iter().map(ItemStackTemplate::count).sum()
        })
    }

    struct Fixture {
        player: Arc<Player>,
        container: Shared<SimpleContainer>,
        menu: Menu,
        slot: usize,
    }

    fn fixture(world_name: &'static str) -> Fixture {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world(world_name);
        let world = &world_fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "BundleTester", 1).build();
        let _keep_world = world_fixture;

        let container = SimpleContainer::new(1).into_shared();
        let container_ref = ContainerRef::from(Arc::clone(&container));
        let mut builder = MenuBuilder::new(None, 1);
        let slots = builder.custom_section([NormalSlot::new(container_ref, 0)]);
        Fixture {
            player,
            container,
            menu: builder.build(BasicKind {}),
            slot: slots.start(),
        }
    }

    #[test]
    fn primary_click_inserts_the_whole_slot_stack_into_a_bundle() {
        let mut fixture = fixture("bundle_click_insert_all");
        fixture
            .container
            .lock()
            .set_item(0, ItemStack::with_count(&vanilla_items::STONE, 64));
        *fixture.menu.behavior_mut().carried_mut() = bundle_stack(Vec::new());

        fixture.menu.clicked(
            Click::Pickup {
                slot: fixture.slot,
                button: MouseButton::Left,
            },
            &fixture.player,
        );

        assert_eq!(bundle_total(fixture.menu.behavior().carried()), 64);
        assert!(fixture.container.lock().get_item(0).is_empty());
    }

    #[test]
    fn bundle_weight_limit_leaves_the_remainder_in_the_slot() {
        let mut fixture = fixture("bundle_click_weight_limit");
        fixture
            .container
            .lock()
            .set_item(0, ItemStack::with_count(&vanilla_items::STONE, 64));
        // A bundle holds 64 stone; prefill half of it.
        *fixture.menu.behavior_mut().carried_mut() =
            bundle_stack(vec![ItemStack::with_count(&vanilla_items::STONE, 32)]);

        fixture.menu.clicked(
            Click::Pickup {
                slot: fixture.slot,
                button: MouseButton::Left,
            },
            &fixture.player,
        );

        assert_eq!(bundle_total(fixture.menu.behavior().carried()), 64);
        assert_eq!(fixture.container.lock().get_item(0).count(), 32);
    }

    #[test]
    fn secondary_click_extracts_one_stack_from_a_bundle_in_the_slot() {
        let mut fixture = fixture("bundle_click_extract");
        fixture.container.lock().set_item(
            0,
            bundle_stack(vec![ItemStack::with_count(&vanilla_items::STONE, 3)]),
        );
        assert!(fixture.menu.behavior().carried().is_empty());

        fixture.menu.clicked(
            Click::Pickup {
                slot: fixture.slot,
                button: MouseButton::Right,
            },
            &fixture.player,
        );

        let carried = fixture.menu.behavior().carried().clone();
        assert_eq!(carried.item.key, vanilla_items::STONE.key);
        assert_eq!(carried.count(), 3);
        assert_eq!(bundle_total(fixture.container.lock().get_item(0)), 0);
    }
}
