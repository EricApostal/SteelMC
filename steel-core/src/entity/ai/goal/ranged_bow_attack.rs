//! Vanilla `RangedBowAttackGoal` (vanilla `net.minecraft.world.entity.ai.goal.RangedBowAttackGoal`).
//!
//! **Dependency**: vanilla releases an `Arrow` through `RangedAttackMob.performRangedAttack`
//! and `ProjectileUtil.getMobArrow`; Steel has no `arrow` entity factory yet, so the
//! charge/strafe state machine below is complete and only the projectile release is gated on
//! that factory, exactly like [`RangedCrossbowAttackGoal`](super::RangedCrossbowAttackGoal).
//!
//! This is the bow counterpart of the crossbow goal, not a duplicate of it: vanilla keeps
//! `RangedBowAttackGoal` and `RangedCrossbowAttackGoal` as separate goals because the charge
//! timing, strafing and item-use handling differ.

use steel_registry::vanilla_items;

use steel_utils::types::InteractionHand;

use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `RangedBowAttackGoal.SEE_TIME`.
const SEE_TIME: i32 = 20;
/// Vanilla `RangedBowAttackGoal` strafe refresh interval.
const STRAFE_TIME: i32 = 20;
/// Vanilla `BowItem.getPowerForTime` full-draw tick count.
const FULL_DRAW_TICKS: i32 = 20;

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

    /// Vanilla `RangedBowAttackGoal.isHoldingBow`.
    fn is_holding_bow(mob: &dyn PathfinderMob) -> bool {
        mob.is_holding(&mut |stack| std::ptr::eq(stack.item(), &*vanilla_items::BOW))
    }

    /// Vanilla `RangedAttackMob.performRangedAttack` projectile release.
    ///
    /// Gated on the missing `arrow` factory: without it there is nothing to shoot, so the
    /// goal keeps the vanilla draw/strafe state instead of fabricating a projectile.
    fn release_shot(_mob: &dyn PathfinderMob, _target: &SharedEntity) -> bool {
        crate::entity::ENTITIES.has_factory(&steel_registry::vanilla_entities::ARROW)
    }

    /// Vanilla `Mob.lookAt(target, 30.0F, 30.0F)`.
    fn look_at_target(mob: &dyn PathfinderMob, target: &SharedEntity) {
        let eye = glam::DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
        mob.mob_base()
            .controls()
            .lock()
            .look_control
            .set_look_at(eye, 30.0, 30.0);
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
            Self::look_at_target(mob, &target);
        } else {
            Self::look_at_target(mob, &target);
        }

        if mob.is_using_item() {
            if !has_line_of_sight && self.see_time < -60 {
                mob.stop_using_item();
            } else if has_line_of_sight {
                let pull_time = mob.get_tick_count_using_item();
                if pull_time >= FULL_DRAW_TICKS {
                    mob.stop_using_item();
                    if Self::release_shot(mob, &target) {
                        self.attack_time = self.attack_interval_min;
                    }
                }
            }
        } else {
            self.attack_time -= 1;
            if self.attack_time <= 0 && self.see_time >= -60 {
                mob.start_using_item(InteractionHand::MainHand);
            }
        }
    }
}
