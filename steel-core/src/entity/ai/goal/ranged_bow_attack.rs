//! Vanilla `RangedBowAttackGoal` (vanilla `net.minecraft.world.entity.ai.goal.RangedBowAttackGoal`).
//!
//! The goal drives the bow draw/strafe state machine and releases the shot through
//! [`RangedAttackMob::perform_ranged_attack`] — the Steel capability behind vanilla's
//! `RangedAttackMob` bound — with `BowItem.getPowerForTime` as the power.
//!
//! This is the bow counterpart of the crossbow goal, not a duplicate of it: vanilla keeps
//! `RangedBowAttackGoal` and `RangedCrossbowAttackGoal` as separate goals because the charge
//! timing, strafing and item-use handling differ.

use std::ptr;

use glam::DVec3;
use steel_math::wrap_degrees;
use steel_registry::vanilla_items;
use steel_utils::types::InteractionHand;

use super::selector::{Goal, GoalControls};
use crate::entity::{Mob, PathfinderMob, SharedEntity};

/// Vanilla `RangedBowAttackGoal` "has seen the target for this long" threshold.
const SEE_TIME: i32 = 20;
/// Vanilla `RangedBowAttackGoal` strafe refresh interval.
const STRAFE_TIME: i32 = 20;
/// Vanilla `RangedBowAttackGoal` full-draw tick count (`BowItem.getPowerForTime(20) == 1.0`).
const FULL_DRAW_TICKS: i32 = 20;
/// Vanilla `RangedBowAttackGoal` aim speed used by `Mob.lookAt` / `LookControl.setLookAt`.
const LOOK_Y_MAX_ROT_SPEED: f32 = 30.0;
const LOOK_X_MAX_ROT_ANGLE: f32 = 30.0;

/// Vanilla `BowItem.getPowerForTime`.
fn get_power_for_time(time_held: i32) -> f32 {
    let mut power = time_held as f32 / 20.0;
    power = (power * power + power * 2.0) / 3.0;
    power.min(1.0)
}

/// Vanilla `Mob.lookAt(Entity, float, float)`.
fn look_at_entity(mob: &dyn Mob, target: &SharedEntity) {
    let position = mob.position();
    let xd = target.position().x - position.x;
    let zd = target.position().z - position.z;
    let yd = target.get_eye_y() - mob.get_eye_y();
    let horizontal = (xd * xd + zd * zd).sqrt();

    let yaw_delta = (zd.atan2(xd).to_degrees() - 90.0) as f32;
    let pitch_delta = (-(yd.atan2(horizontal)).to_degrees()) as f32;
    let (yaw, pitch) = mob.rotation();
    let yaw =
        yaw + wrap_degrees(yaw_delta - yaw).clamp(-LOOK_Y_MAX_ROT_SPEED, LOOK_Y_MAX_ROT_SPEED);
    let pitch = pitch
        + wrap_degrees(pitch_delta - pitch).clamp(-LOOK_X_MAX_ROT_ANGLE, LOOK_X_MAX_ROT_ANGLE);
    mob.set_rotation((yaw, pitch));
}

/// Vanilla `LookControl.setLookAt(Entity, float, float)`.
fn set_look_control_at(mob: &dyn PathfinderMob, target: &SharedEntity) {
    let eye = DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
    mob.mob_base().controls().lock().look_control.set_look_at(
        eye,
        LOOK_Y_MAX_ROT_SPEED,
        LOOK_X_MAX_ROT_ANGLE,
    );
}

/// Vanilla `RangedBowAttackGoal`.
pub(crate) struct RangedBowAttackGoal {
    speed_modifier: f64,
    attack_interval_min: i32,
    attack_radius_sqr: f64,
    attack_time: i32,
    see_time: i32,
    strafing_clockwise: bool,
    strafing_backwards: bool,
    strafing_time: i32,
}

impl RangedBowAttackGoal {
    /// Vanilla `RangedBowAttackGoal(Mob, double, int, float)`.
    #[must_use]
    pub(crate) fn new(speed_modifier: f64, attack_interval_min: i32, attack_radius: f32) -> Self {
        Self {
            speed_modifier,
            attack_interval_min,
            attack_radius_sqr: f64::from(attack_radius) * f64::from(attack_radius),
            attack_time: -1,
            see_time: 0,
            strafing_clockwise: false,
            strafing_backwards: false,
            strafing_time: -1,
        }
    }

    /// Vanilla `RangedBowAttackGoal.setMinAttackInterval`.
    pub(crate) const fn set_min_attack_interval(&mut self, ticks: i32) {
        self.attack_interval_min = ticks;
    }

    /// Vanilla `RangedBowAttackGoal.isHoldingBow`.
    fn is_holding_bow(mob: &dyn PathfinderMob) -> bool {
        mob.is_holding(&mut |stack| ptr::eq(stack.item(), &*vanilla_items::BOW))
    }

    /// Vanilla `ProjectileUtil.getWeaponHoldingHand`: the main hand when it holds the bow,
    /// otherwise the off hand.
    fn weapon_holding_hand(mob: &dyn PathfinderMob) -> InteractionHand {
        if mob
            .get_item_in_hand(InteractionHand::MainHand)
            .is(&vanilla_items::BOW)
        {
            InteractionHand::MainHand
        } else {
            InteractionHand::OffHand
        }
    }
}

impl Goal for RangedBowAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target().is_some() && Self::is_holding_bow(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        (self.can_use(mob) || !mob.mob_base().navigation().lock().is_done())
            && Self::is_holding_bow(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(true);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(false);
        self.see_time = 0;
        self.attack_time = -1;
        mob.stop_using_item();
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };

        let distance_sqr = mob.position().distance_squared(target.position());
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

        if distance_sqr <= self.attack_radius_sqr && self.see_time >= SEE_TIME {
            mob.mob_base().navigation().lock().stop();
            self.strafing_time += 1;
        } else {
            mob.move_to_pos(target.position(), self.speed_modifier);
            self.strafing_time = -1;
        }

        if self.strafing_time >= STRAFE_TIME {
            if rand::random::<f32>() < 0.3 {
                self.strafing_clockwise = !self.strafing_clockwise;
            }
            if rand::random::<f32>() < 0.3 {
                self.strafing_backwards = !self.strafing_backwards;
            }
            self.strafing_time = 0;
        }

        if self.strafing_time > -1 {
            if distance_sqr > self.attack_radius_sqr * 0.75 {
                self.strafing_backwards = false;
            } else if distance_sqr < self.attack_radius_sqr * 0.25 {
                self.strafing_backwards = true;
            }

            mob.mob_base().controls().lock().move_control.strafe(
                if self.strafing_backwards { -0.5 } else { 0.5 },
                if self.strafing_clockwise { 0.5 } else { -0.5 },
            );
            // Vanilla also turns the ridden `Mob` vehicle toward the target, then the rider.
            if let Some(vehicle) = mob.controlled_vehicle()
                && let Some(vehicle_mob) = vehicle.as_mob()
            {
                look_at_entity(vehicle_mob, &target);
            }
            look_at_entity(mob, &target);
        } else {
            set_look_control_at(mob, &target);
        }

        if mob.is_using_item() {
            if !has_line_of_sight && self.see_time < -60 {
                mob.stop_using_item();
            } else if has_line_of_sight {
                let pull_time = mob.get_tick_count_using_item();
                if pull_time >= FULL_DRAW_TICKS {
                    mob.stop_using_item();
                    if let Some(ranged_attack_mob) = mob.as_ranged_attack_mob() {
                        let _ = ranged_attack_mob
                            .perform_ranged_attack(&target, get_power_for_time(pull_time));
                    }
                    self.attack_time = self.attack_interval_min;
                }
            }
        } else {
            self.attack_time -= 1;
            if self.attack_time <= 0 && self.see_time >= -60 {
                mob.start_using_item(Self::weapon_holding_hand(mob));
            }
        }
    }
}
