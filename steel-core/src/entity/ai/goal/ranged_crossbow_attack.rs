//! Vanilla `RangedCrossbowAttackGoal` (vanilla `RangedCrossbowAttackGoal` /
//! `CrossbowAttackMob.performCrossbowAttack`).
//!
//! The charge/aim/ready state machine is the complete vanilla one. The shot itself goes
//! through [`RangedAttackMob::perform_ranged_attack`], the Steel capability behind vanilla's
//! `RangedAttackMob` bound; a crossbow mob that does not implement that capability (e.g. a
//! pillager without `CrossbowAttackMob`) simply has nothing to release.

use steel_registry::vanilla_items;
use steel_utils::Downcast as _;
use steel_utils::types::InteractionHand;

use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `RangedCrossbowAttackGoal` look speed while charging.
const PERFECT_LOOK_SPEED: f32 = 30.0;
/// Vanilla `RangedCrossbowAttackGoal` pitch clamp while charging.
const PERFECT_LOOK_ANGLE: f32 = 30.0;
/// Vanilla `CrossbowItem.getChargeDuration` for a crossbow without quick-charge.
const CROSSBOW_CHARGE_DURATION: i32 = 25;
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
        mob.is_holding(&mut |stack| std::ptr::eq(stack.item(), &*vanilla_items::CROSSBOW))
    }

    /// Vanilla `RangedCrossbowAttackGoal.isValidTarget`.
    fn is_valid_target(mob: &dyn PathfinderMob) -> bool {
        mob.target().is_some_and(|target| target.is_alive())
    }

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow` for the pillar of this goal: only
    /// pillagers are `CrossbowAttackMob`s, so the concrete capability is reached by key.
    fn set_charging_crossbow(mob: &dyn PathfinderMob, charging: bool) {
        if let Some(pillager) =
            mob.downcast_ref::<crate::entity::entities::mobs::illager::PillagerEntity>()
        {
            pillager.set_charging_crossbow(charging);
        }
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
            // Vanilla also clears `DataComponents.CHARGED_PROJECTILES` on the use stack.
            // Steel has no `CrossbowItem` behavior that loads projectiles, so the stack can
            // never carry charged projectiles here; there is nothing to clear.
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };

        let has_line_of_sight = mob.has_line_of_sight_cached(target.as_ref());
        let had_line_of_sight = self.see_time > 0;
        if has_line_of_sight != had_line_of_sight {
            self.see_time = 0;
        }
        if has_line_of_sight {
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

                if mob.get_tick_count_using_item() >= CROSSBOW_CHARGE_DURATION {
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
                if has_line_of_sight {
                    // Vanilla `CrossbowAttackMob.performCrossbowAttack` through
                    // `Mob.performRangedAttack(target, 1.0F)`. The `RangedAttackMob` bound of
                    // the vanilla goal is the capability Steel dispatches through; a mob
                    // without it has no projectile to release.
                    if let Some(ranged_attack_mob) = mob.as_ranged_attack_mob() {
                        ranged_attack_mob.perform_ranged_attack(&target, 1.0);
                    }
                    self.crossbow_state = CrossbowState::Uncharged;
                }
            }
        }
    }
}
