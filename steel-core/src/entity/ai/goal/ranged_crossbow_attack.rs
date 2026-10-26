//! Vanilla `RangedCrossbowAttackGoal` (vanilla `RangedCrossbowAttackGoal` /
//! `CrossbowAttackMob.performCrossbowAttack`).
//!
//! The charge/aim/ready state machine is the complete vanilla one. The shot itself is
//! released through `RangedAttackMob::perform_ranged_attack`: any crossbow mob Steel
//! models (`PillagerEntity`, `PiglinEntity`) implements the capability and fires its
//! crossbow at the attack target.

use std::ptr;

use steel_registry::vanilla_items;
use steel_utils::Downcast as _;
use steel_utils::types::InteractionHand;

use super::selector::{Goal, GoalControls};
use crate::behavior::items::get_charge_duration;
use crate::entity::entities::mobs::hostile::piglin::PiglinEntity;
use crate::entity::entities::mobs::illager::PillagerEntity;
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `RangedCrossbowAttackGoal` look speed while charging.
const PERFECT_LOOK_SPEED: f32 = 30.0;
/// Vanilla `RangedCrossbowAttackGoal` pitch clamp while charging.
const PERFECT_LOOK_ANGLE: f32 = 30.0;
/// Vanilla `RangedCrossbowAttackGoal.PATHFINDING_DELAY_RANGE` (`TimeUtil.rangeOfSeconds(1, 2)`).
const PATHFINDING_DELAY_MIN: i32 = 20;
const PATHFINDING_DELAY_MAX: i32 = 40;
/// Vanilla `RangedCrossbowAttackGoal` "wait after charging" range.
const ATTACK_DELAY_MIN: i32 = 20;
const ATTACK_DELAY_RANGE: i32 = 20;

/// Vanilla `RangedCrossbowAttackGoal.CrossbowState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrossbowState {
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// Vanilla `RangedCrossbowAttackGoal`.
pub struct RangedCrossbowAttackGoal {
    speed_modifier: f64,
    attack_radius_sqr: f64,
    crossbow_state: CrossbowState,
    see_time: i32,
    attack_delay: i32,
    update_path_delay: i32,
}

impl RangedCrossbowAttackGoal {
    /// Vanilla `RangedCrossbowAttackGoal(Mob, double, float)`.
    #[must_use]
    pub fn new(speed_modifier: f64, attack_radius: f32) -> Self {
        Self {
            speed_modifier,
            attack_radius_sqr: f64::from(attack_radius) * f64::from(attack_radius),
            crossbow_state: CrossbowState::Uncharged,
            see_time: 0,
            attack_delay: 0,
            update_path_delay: 0,
        }
    }

    /// Vanilla `RangedCrossbowAttackGoal.isHoldingCrossbow`.
    fn is_holding_crossbow(mob: &dyn PathfinderMob) -> bool {
        mob.is_holding(&mut |stack| ptr::eq(stack.item(), &raw const *vanilla_items::CROSSBOW))
    }

    /// Vanilla `RangedCrossbowAttackGoal.isValidTarget`.
    fn is_valid_target(mob: &dyn PathfinderMob) -> bool {
        mob.target().is_some_and(|target| target.is_alive())
    }

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow` for the mobs Steel models
    /// as `CrossbowAttackMob`s: the pillager and the piglin both read the flag
    /// for their crossbow arm pose (`IllagerArmPose.CROSSBOW_CHARGE`,
    /// `PiglinArmPose.CROSSBOW_CHARGE`).
    fn set_charging_crossbow(mob: &dyn PathfinderMob, charging: bool) {
        if let Some(pillager) = mob.downcast_ref::<PillagerEntity>() {
            pillager.set_charging_crossbow(charging);
        } else if let Some(piglin) = mob.downcast_ref::<PiglinEntity>() {
            piglin.set_charging_crossbow(charging);
        }
    }

    /// Vanilla `CrossbowItem.getChargeDuration(mob.getUseItem(), mob)`: the use
    /// hand is the weapon-holding hand the charge started in, so Quick Charge
    /// shortens the release threshold exactly as it shortens the charge itself
    /// (25 ticks without it).
    fn charge_duration(mob: &dyn PathfinderMob) -> i32 {
        get_charge_duration(&mob.get_item_in_hand(Self::weapon_holding_hand(mob)))
    }

    /// Vanilla `ProjectileUtil.getWeaponHoldingHand`: the main hand when it holds the
    /// crossbow, otherwise the off hand.
    fn weapon_holding_hand(mob: &dyn PathfinderMob) -> InteractionHand {
        if mob
            .get_item_in_hand(InteractionHand::MainHand)
            .is(&vanilla_items::CROSSBOW)
        {
            InteractionHand::MainHand
        } else {
            InteractionHand::OffHand
        }
    }

    /// Vanilla `RangedCrossbowAttackGoal.canRun`: only an uncharged crossbow is run with the
    /// full speed modifier.
    const fn can_run(&self) -> bool {
        matches!(self.crossbow_state, CrossbowState::Uncharged)
    }
}

impl Goal for RangedCrossbowAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::is_valid_target(mob) && Self::is_holding_crossbow(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::is_valid_target(mob)
            && (self.can_use(mob) || !mob.mob_base().navigation().lock().is_done())
            && Self::is_holding_crossbow(mob)
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(false);
        let _ = mob.set_target(None);
        self.see_time = 0;
        if mob.is_using_item() {
            mob.stop_using_item();
            Self::set_charging_crossbow(mob, false);
            // Vanilla also clears `DataComponents.CHARGED_PROJECTILES` on the use stack
            // here, but it reads that stack through `getUseItem()` after
            // `stopUsingItem()` has already reset `useItem` to `ItemStack.EMPTY`, so the
            // clear never reaches the crossbow in hand and there is nothing to clear.
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };

        let sees_target = mob.has_line_of_sight_cached(target.as_ref());
        let saw_target = self.see_time > 0;
        if sees_target != saw_target {
            self.see_time = 0;
        }
        if sees_target {
            self.see_time += 1;
        } else {
            self.see_time -= 1;
        }

        let distance_sqr = mob.position().distance_squared(target.position());
        let needs_to_move =
            (distance_sqr > self.attack_radius_sqr || self.see_time < 5) && self.attack_delay == 0;
        if needs_to_move {
            self.update_path_delay -= 1;
            if self.update_path_delay <= 0 {
                let speed_modifier = if self.can_run() {
                    self.speed_modifier
                } else {
                    self.speed_modifier * 0.5
                };
                mob.move_to_pos(target.position(), speed_modifier);
                self.update_path_delay = PATHFINDING_DELAY_MIN
                    + rand::random_range(0..=PATHFINDING_DELAY_MAX - PATHFINDING_DELAY_MIN);
            }
        } else {
            self.update_path_delay = 0;
            mob.mob_base().navigation().lock().stop();
        }

        let target_eye =
            glam::DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
        mob.mob_base().controls().lock().look_control.set_look_at(
            target_eye,
            PERFECT_LOOK_SPEED,
            PERFECT_LOOK_ANGLE,
        );

        match self.crossbow_state {
            CrossbowState::Uncharged => {
                if !needs_to_move {
                    mob.start_using_item(Self::weapon_holding_hand(mob));
                    self.crossbow_state = CrossbowState::Charging;
                    Self::set_charging_crossbow(mob, true);
                }
            }
            CrossbowState::Charging => {
                if !mob.is_using_item() {
                    self.crossbow_state = CrossbowState::Uncharged;
                }

                if mob.get_tick_count_using_item() >= Self::charge_duration(mob) {
                    mob.release_using_item(entity);
                    self.crossbow_state = CrossbowState::Charged;
                    self.attack_delay =
                        ATTACK_DELAY_MIN + rand::random_range(0..ATTACK_DELAY_RANGE);
                    Self::set_charging_crossbow(mob, false);
                }
            }
            CrossbowState::Charged => {
                self.attack_delay -= 1;
                if self.attack_delay == 0 {
                    self.crossbow_state = CrossbowState::ReadyToAttack;
                }
            }
            CrossbowState::ReadyToAttack => {
                if sees_target {
                    // Vanilla `CrossbowAttackMob.performCrossbowAttack` through
                    // `Mob.performRangedAttack(target, 1.0F)`: the `1.0F` only satisfies the
                    // `RangedAttackMob` signature, and the mob's own override decides the real
                    // bolt speed.
                    if let Some(ranged_attack_mob) = mob.as_ranged_attack_mob() {
                        ranged_attack_mob.perform_ranged_attack(&target, 1.0);
                    }
                    self.crossbow_state = CrossbowState::Uncharged;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::Identifier;
    use steel_utils::types::InteractionHand;

    use super::RangedCrossbowAttackGoal;
    use crate::entity::LivingEntity as _;
    use crate::entity::SharedEntity;
    use crate::entity::entities::mobs::hostile::piglin::PiglinEntity;
    use crate::entity::entities::mobs::illager::PillagerEntity;

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow` has to reach every
    /// crossbow mob, not just the pillager: the piglin reads the same flag for
    /// its `PiglinArmPose.CROSSBOW_CHARGE`.
    #[test]
    fn charging_flag_reaches_both_crossbow_mobs() {
        init_vanilla_registry();
        let pillager =
            PillagerEntity::new(&vanilla_entities::PILLAGER, 1, DVec3::ZERO, Weak::new());
        let piglin = PiglinEntity::new(&vanilla_entities::PIGLIN, 2, DVec3::ZERO, Weak::new());

        assert!(!pillager.is_charging_crossbow());
        assert!(!piglin.is_charging_crossbow());

        RangedCrossbowAttackGoal::set_charging_crossbow(&pillager, true);
        RangedCrossbowAttackGoal::set_charging_crossbow(&piglin, true);
        assert!(pillager.is_charging_crossbow(), "the pillager charges");
        assert!(piglin.is_charging_crossbow(), "the piglin charges too");

        RangedCrossbowAttackGoal::set_charging_crossbow(&pillager, false);
        RangedCrossbowAttackGoal::set_charging_crossbow(&piglin, false);
        assert!(!pillager.is_charging_crossbow());
        assert!(!piglin.is_charging_crossbow());
    }

    /// Vanilla releases on `getTicksUsingItem() >= CrossbowItem.getChargeDuration
    /// (mob.getUseItem(), mob)`, so Quick Charge shortens the goal's release
    /// threshold instead of leaving it pinned at the plain 25 ticks.
    #[test]
    fn release_threshold_uses_the_held_crossbows_charge_duration() {
        init_vanilla_registry();
        let pillager =
            PillagerEntity::new(&vanilla_entities::PILLAGER, 1, DVec3::ZERO, Weak::new());

        pillager.set_item_in_hand(
            InteractionHand::MainHand,
            ItemStack::new(&vanilla_items::CROSSBOW),
        );
        assert_eq!(RangedCrossbowAttackGoal::charge_duration(&pillager), 25);

        let mut quick = ItemStack::new(&vanilla_items::CROSSBOW);
        quick.set_enchantments(&[(Identifier::vanilla_static("quick_charge"), 3)], false);
        pillager.set_item_in_hand(InteractionHand::MainHand, quick);
        assert_eq!(
            RangedCrossbowAttackGoal::charge_duration(&pillager),
            10,
            "the goal releases at the crossbow's own charge duration"
        );
    }

    /// The goal's `ReadyToAttack` arm releases through the mob itself: without
    /// this capability the raid's ready pillager released nothing at all.
    #[test]
    fn pillager_exposes_the_ranged_attack_capability() {
        init_vanilla_registry();
        let pillager =
            PillagerEntity::new(&vanilla_entities::PILLAGER, 1, DVec3::ZERO, Weak::new());
        let body: SharedEntity = Arc::new(pillager);
        assert!(
            body.as_ranged_attack_mob().is_some(),
            "the goal's ready state has nothing to release without this capability"
        );
    }
}
