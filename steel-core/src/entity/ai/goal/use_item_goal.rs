//! Vanilla `UseItemGoal`: equips an item, uses it, then clears the main hand.

use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_utils::types::InteractionHand;

use crate::entity::PathfinderMob;
use crate::entity::ai::goal::selector::{Goal, GoalControls};

/// Vanilla `UseItemGoal.canUseSelector`: `Predicate<Mob>` over Steel's erased goal mob.
type CanUseSelector = Box<dyn Fn(&dyn PathfinderMob) -> bool + Send>;

/// Port of vanilla `UseItemGoal`.
///
/// Equips `item` in the main hand and starts using it while `can_use_selector`
/// accepts the mob, then clears the slot again once the use ends. Completion is
/// driven entirely by the living item-use pipeline (`updating_using_item`), so
/// the goal owns no controls and needs no per-tick update.
pub struct UseItemGoal {
    item: ItemStack,
    finish_using_sound: Option<SoundEventRef>,
    can_use_selector: CanUseSelector,
}

impl UseItemGoal {
    /// Creates the goal with vanilla's
    /// `UseItemGoal(mob, item, finishUsingSound, canUseSelector)` arguments.
    ///
    /// `finish_using_sound` is played when the goal stops, and `can_use_selector`
    /// decides when the mob wants to use `item`.
    #[must_use]
    pub(crate) fn new(
        item: ItemStack,
        finish_using_sound: Option<SoundEventRef>,
        can_use_selector: impl Fn(&dyn PathfinderMob) -> bool + Send + 'static,
    ) -> Self {
        Self {
            item,
            finish_using_sound,
            can_use_selector: Box::new(can_use_selector),
        }
    }
}

impl Goal for UseItemGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        (self.can_use_selector)(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.is_using_item()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.set_item_in_hand(
            InteractionHand::MainHand,
            self.item.copy_with_count(self.item.count()),
        );
        mob.start_using_item(InteractionHand::MainHand);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        mob.set_item_in_hand(InteractionHand::MainHand, ItemStack::empty());
        if let Some(sound) = self.finish_using_sound {
            mob.play_sound(sound, 1.0, rand::random::<f32>() * 0.2 + 0.9);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::entity_type::EntityTypeRef;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::locks::SyncMutex;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::{
        Entity, EntityBase, LivingEntity, LivingEntityBase, Mob, MobBase, impl_test_downcast_type,
    };
    use crate::inventory::equipment::EquipmentSlot;

    struct TestPathfinderMob {
        base: EntityBase,
        living_base: LivingEntityBase,
        mob_base: MobBase,
        mob_flags: SyncMutex<i8>,
        health: SyncMutex<f32>,
    }

    impl TestPathfinderMob {
        fn new() -> Self {
            init_vanilla_registry();
            init_behaviors();
            Self {
                base: EntityBase::new(
                    1,
                    DVec3::ZERO,
                    vanilla_entities::PIG.dimensions,
                    Weak::new(),
                ),
                living_base: LivingEntityBase::new(&vanilla_entities::PIG),
                mob_base: MobBase::new(),
                mob_flags: SyncMutex::new(0),
                health: SyncMutex::new(10.0),
            }
        }
    }

    impl_test_downcast_type!(TestPathfinderMob);

    impl Entity for TestPathfinderMob {
        fn base(&self) -> &EntityBase {
            &self.base
        }

        fn entity_type(&self) -> EntityTypeRef {
            &vanilla_entities::PIG
        }
    }

    impl LivingEntity for TestPathfinderMob {
        fn living_base(&self) -> &LivingEntityBase {
            &self.living_base
        }

        fn get_health(&self) -> f32 {
            *self.health.lock()
        }

        fn set_health(&self, health: f32) {
            *self.health.lock() = health;
        }
    }

    impl Mob for TestPathfinderMob {
        fn mob_base(&self) -> &MobBase {
            &self.mob_base
        }

        fn mob_flags(&self) -> i8 {
            *self.mob_flags.lock()
        }

        fn set_mob_flags(&self, flags: i8) {
            *self.mob_flags.lock() = flags;
        }
    }

    impl PathfinderMob for TestPathfinderMob {}

    /// Vanilla `UseItemGoal.start`/`stop` equip then clear `MAINHAND`, only
    /// touch that slot, and `canContinueToUse` tracks the mob's active use.
    #[test]
    fn use_item_goal_equips_starts_and_clears_the_main_hand() {
        let mob = TestPathfinderMob::new();
        let mut goal = UseItemGoal::new(ItemStack::new(&vanilla_items::POTION), None, |_| true);

        assert!(goal.can_use(&mob));
        assert!(!mob.is_using_item());

        goal.start(&mob);
        assert!(
            mob.get_item_in_hand(InteractionHand::MainHand)
                .is(&vanilla_items::POTION)
        );
        assert!(mob.is_using_item());
        assert!(goal.can_continue_to_use(&mob));

        let mut off_hand_empty = false;
        mob.with_equipment_slot(EquipmentSlot::OffHand, &mut |item| {
            off_hand_empty = item.is_empty();
        });
        assert!(off_hand_empty, "vanilla only equips MAINHAND");

        // Completion ends the use; the goal then stops and clears the slot.
        mob.stop_using_item();
        assert!(!mob.is_using_item());
        assert!(!goal.can_continue_to_use(&mob));

        goal.stop(&mob);
        assert!(
            mob.get_item_in_hand(InteractionHand::MainHand).is_empty(),
            "vanilla `UseItemGoal.stop` clears the main hand"
        );
    }

    /// Vanilla `UseItemGoal.canUse` is exactly the selector, and the goal claims
    /// no control flags, so it never fights movement/look/jump/target goals.
    #[test]
    fn use_item_goal_is_selector_gated_and_claims_no_controls() {
        let mob = TestPathfinderMob::new();
        let mut goal =
            UseItemGoal::new(ItemStack::new(&vanilla_items::MILK_BUCKET), None, |_| false);

        assert!(!goal.can_use(&mob));
        assert_eq!(goal.controls(), GoalControls::EMPTY);
        assert!(!goal.controls().intersects(
            GoalControls::MOVE | GoalControls::LOOK | GoalControls::JUMP | GoalControls::TARGET
        ));
        // Vanilla `Goal.requiresUpdateEveryTick` default; `UseItemGoal` does not override it.
        assert!(!Goal::requires_update_every_tick(&goal));
        assert!(Goal::is_interruptable(&goal));
    }

    /// A mob already using an item is not restarted by `start`, mirroring
    /// vanilla's `!this.isUsingItem()` gate inside `startUsingItem`.
    #[test]
    fn use_item_goal_start_does_not_restart_an_active_use() {
        let mob = TestPathfinderMob::new();
        let mut goal =
            UseItemGoal::new(ItemStack::new(&vanilla_items::MILK_BUCKET), None, |_| true);

        mob.set_item_in_hand(
            InteractionHand::MainHand,
            ItemStack::new(&vanilla_items::POTION),
        );
        mob.start_using_item(InteractionHand::MainHand);
        let remaining = mob.get_use_item_remaining_ticks();
        assert!(remaining > 0);

        goal.start(&mob);

        assert!(mob.is_using_item());
        assert_eq!(
            mob.get_use_item_remaining_ticks(),
            remaining,
            "an active use keeps its countdown"
        );
    }
}
