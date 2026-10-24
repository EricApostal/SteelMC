//! Vanilla `OcelotAttackGoal` (`goal/OcelotAttackGoal.java`): ocelots and frogs chase a
//! target and bite it on contact, sprinting mid-range.
//!
//! Vanilla stores the concrete `Mob`; Steel receives `&dyn PathfinderMob` per call, which is
//! the object-safe form of the same mob.

use crate::entity::{PathfinderMob, SharedEntity};

use super::selector::{Goal, GoalControls};

/// Vanilla `OcelotAttackGoal.canContinueToUse`: the target gives up past 15 blocks.
const GIVE_UP_RANGE_SQR: f64 = 225.0;
/// Vanilla `OcelotAttackGoal.tick`: the melee contact radius squared is derived from the mob
/// width at runtime; the two speed brackets are fixed speed modifiers.
const CLOSE_SPEED_MODIFIER: f64 = 0.6;
/// Vanilla `OcelotAttackGoal.tick` speed when the target is beyond contact range but within
/// 16 blocks: the mob leaps toward it.
const LEAP_SPEED_MODIFIER: f64 = 1.33;
/// Vanilla `OcelotAttackGoal.tick` speed at every other range.
const FAR_SPEED_MODIFIER: f64 = 0.8;
/// Vanilla `OcelotAttackGoal.tick` melee cooldown.
const ATTACK_INTERVAL_TICKS: i32 = 20;
/// Vanilla `OcelotAttackGoal.tick` look speed and pitch clamp.
const AIM_SPEED: f32 = 30.0;
const AIM_ANGLE: f32 = 30.0;

/// Vanilla `OcelotAttackGoal`.
pub struct OcelotAttackGoal {
    target: Option<SharedEntity>,
    attack_time: i32,
}

impl OcelotAttackGoal {
    /// Vanilla `new OcelotAttackGoal(mob)`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            target: None,
            attack_time: 0,
        }
    }

    /// Vanilla `mob.getBbWidth() * 2.0F * (mob.getBbWidth() * 2.0F)`: the product is evaluated
    /// in float and only then widened to double.
    fn melee_radius_sqr(mob: &dyn PathfinderMob) -> f64 {
        let double_width = mob.bounding_box().width() as f32 * 2.0;
        f64::from(double_width * double_width)
    }
}

impl Default for OcelotAttackGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for OcelotAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };

        self.target = Some(target);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = &self.target else {
            return false;
        };
        if !target.is_alive() {
            return false;
        }

        if mob.position().distance_squared(target.position()) > GIVE_UP_RANGE_SQR {
            return false;
        }

        !mob.mob_base().navigation().lock().is_done() || self.can_use(mob)
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.target = None;
        mob.mob_base().navigation().lock().stop();
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(target) = self.target.clone() else {
            return;
        };

        let target_position = target.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            glam::DVec3::new(target_position.x, target.get_eye_y(), target_position.z),
            AIM_SPEED,
            AIM_ANGLE,
        );

        let melee_radius_sqr = Self::melee_radius_sqr(mob);
        let distance_sqr = mob.position().distance_squared(target.position());
        let speed_modifier = if distance_sqr > melee_radius_sqr && distance_sqr < 16.0 {
            LEAP_SPEED_MODIFIER
        } else if distance_sqr < GIVE_UP_RANGE_SQR {
            CLOSE_SPEED_MODIFIER
        } else {
            FAR_SPEED_MODIFIER
        };

        mob.move_to_pos(target_position, speed_modifier);
        self.attack_time = (self.attack_time - 1).max(0);
        if distance_sqr <= melee_radius_sqr && self.attack_time <= 0 {
            self.attack_time = ATTACK_INTERVAL_TICKS;
            let Some(world) = mob.level() else {
                return;
            };
            let _ = mob.do_hurt_target(entity, &world, &target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocelot_attack_goal_uses_move_and_look_controls() {
        let goal = OcelotAttackGoal::new();

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::LOOK);
        assert!(goal.requires_update_every_tick());
    }
}
