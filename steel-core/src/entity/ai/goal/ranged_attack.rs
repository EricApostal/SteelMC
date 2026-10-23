//! Vanilla `RangedAttackGoal` (`goal/RangedAttackGoal.java`): a `RangedAttackMob` keeps its
//! distance, aims at its target and fires when it has had line of sight for 5 ticks.
//!
//! Vanilla stores the shooting mob as a `RangedAttackMob` field; Steel reaches the capability
//! through [`Entity::as_ranged_attack_mob`](crate::entity::Entity::as_ranged_attack_mob) so
//! the goal stays mob-agnostic, exactly as the landed `RangedCrossbowAttackGoal` does for
//! pillagers.
//!
//! **Dependency**: the projectile release itself is gated on the arrow factory
//! (`ENTITIES.has_factory(&vanilla_entities::ARROW)`), mirroring `RangedCrossbowAttackGoal`:
//! until the arrow entity lands there is nothing to shoot, and
//! [`RangedAttackMob::perform_ranged_attack`] reports that nothing was released so the goal
//! keeps its vanilla timing state instead of fabricating a projectile.

use crate::entity::{PathfinderMob, SharedEntity};

use super::selector::{Goal, GoalControls};

/// Vanilla `RangedAttackGoal` line-of-sight ticks required before firing.
const SEE_TIME_THRESHOLD: i32 = 5;
/// Vanilla `RangedAttackGoal.tick` aim speed.
const AIM_Y_MAX_ROT_SPEED: f32 = 30.0;
/// Vanilla `RangedAttackGoal.tick` aim pitch clamp.
const AIM_X_MAX_ROT_ANGLE: f32 = 30.0;
/// Vanilla `RangedAttackGoal.tick` power clamp.
const MIN_ATTACK_POWER: f32 = 0.1;
const MAX_ATTACK_POWER: f32 = 1.0;

/// Vanilla `RangedAttackGoal`.
pub struct RangedAttackGoal {
    target: Option<SharedEntity>,
    attack_time: i32,
    speed_modifier: f64,
    see_time: i32,
    attack_interval_min: i32,
    attack_interval_max: i32,
    attack_radius: f32,
    attack_radius_sqr: f32,
}

impl RangedAttackGoal {
    /// Vanilla `new RangedAttackGoal(RangedAttackMob, double, int, float)`.
    #[must_use]
    pub const fn new(speed_modifier: f64, attack_interval: i32, attack_radius: f32) -> Self {
        Self::with_interval_range(
            speed_modifier,
            attack_interval,
            attack_interval,
            attack_radius,
        )
    }

    /// Vanilla `new RangedAttackGoal(RangedAttackMob, double, int, int, float)`.
    #[must_use]
    pub const fn with_interval_range(
        speed_modifier: f64,
        attack_interval_min: i32,
        attack_interval_max: i32,
        attack_radius: f32,
    ) -> Self {
        Self {
            target: None,
            attack_time: -1,
            speed_modifier,
            see_time: 0,
            attack_interval_min,
            attack_interval_max,
            attack_radius,
            attack_radius_sqr: attack_radius * attack_radius,
        }
    }

    /// Vanilla `RangedAttackGoal.tick` distance-scaled attack delay.
    fn attack_delay_for_distance(&self, distance: f32) -> i32 {
        let scaled = distance * (self.attack_interval_max - self.attack_interval_min) as f32;
        (scaled + self.attack_interval_min as f32) as i32
    }

    /// Vanilla `Mth.lerp(distance, min, max)` for the first cooldown tick, where
    /// `distance` is already the radius-normalized `sqrt(distSqr) / attackRadius`.
    fn attack_interval_lerp(&self, distance: f32) -> i32 {
        let delta = f64::from(distance);
        let min = f64::from(self.attack_interval_min);
        let max = f64::from(self.attack_interval_max);
        (min + (max - min) * delta) as i32
    }
}

impl Goal for RangedAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        // Vanilla rejects a `RangedAttackMob`-less mob in the constructor; Steel keeps the
        // goal inert for such a mob instead.
        if mob.as_ranged_attack_mob().is_none() {
            return false;
        }

        let Some(target) = mob.target() else {
            return false;
        };
        if !target.is_alive() {
            return false;
        }

        self.target = Some(target);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.can_use(mob) {
            return true;
        }

        self.target.as_ref().is_some_and(|target| target.is_alive())
            && !mob.mob_base().navigation().lock().is_done()
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.target = None;
        self.see_time = 0;
        self.attack_time = -1;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(target) = self.target.clone() else {
            return;
        };

        let target_distance_sqr = mob.position().distance_squared(target.position());
        let has_line_of_sight = mob.has_line_of_sight_cached(target.as_ref());
        if has_line_of_sight {
            self.see_time += 1;
        } else {
            self.see_time = 0;
        }

        if !(target_distance_sqr > f64::from(self.attack_radius_sqr))
            && self.see_time >= SEE_TIME_THRESHOLD
        {
            mob.mob_base().navigation().lock().stop();
        } else {
            mob.move_to_pos(target.position(), self.speed_modifier);
        }

        let target_position = target.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            glam::DVec3::new(target_position.x, target.get_eye_y(), target_position.z),
            AIM_Y_MAX_ROT_SPEED,
            AIM_X_MAX_ROT_ANGLE,
        );

        let distance = (target_distance_sqr.sqrt() as f32) / self.attack_radius;
        self.attack_time -= 1;
        if self.attack_time == 0 {
            if !has_line_of_sight {
                return;
            }

            let power = distance.clamp(MIN_ATTACK_POWER, MAX_ATTACK_POWER);
            if let Some(ranged_attack_mob) = mob.as_ranged_attack_mob() {
                ranged_attack_mob.perform_ranged_attack(&target, power);
            }
            self.attack_time = self.attack_delay_for_distance(distance);
        } else if self.attack_time < 0 {
            self.attack_time = self.attack_interval_lerp(distance);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranged_attack_goal_uses_move_and_look_controls() {
        let goal = RangedAttackGoal::new(1.0, 60, 10.0);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::LOOK);
        assert!(goal.requires_update_every_tick());
    }

    #[test]
    fn attack_delay_scales_between_the_interval_bounds() {
        let goal = RangedAttackGoal::with_interval_range(1.0, 20, 60, 10.0);

        assert_eq!(goal.attack_radius_sqr, 100.0);
        // dist 0 -> min interval, dist 1 -> max interval (both floored).
        assert_eq!(goal.attack_delay_for_distance(0.0), 20);
        assert_eq!(goal.attack_delay_for_distance(1.0), 60);
        assert_eq!(goal.attack_interval_lerp(0.0), 20);
        assert_eq!(goal.attack_interval_lerp(1.0), 60);
    }
}
