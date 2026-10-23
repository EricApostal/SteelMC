//! Vanilla `Raider.HoldGroundAttackGoal`: an unengaged patrol leader stops, shouts at
//! nearby raiders to share its target, and holds position until the target closes in.

use glam::DVec3;

use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `HoldGroundAttackGoal.shoutTargeting` range.
const SHOUT_RANGE: f64 = 8.0;
/// Vanilla `Raider.HoldGroundAttackGoal` look speed for the hold-ground stare.
const LOOK_Y_MAX_ROT_SPEED: f32 = 30.0;
/// Vanilla `Raider.HoldGroundAttackGoal` pitch clamp for the hold-ground stare.
const LOOK_X_MAX_ROT_ANGLE: f32 = 30.0;

/// Vanilla `Raider.HoldGroundAttackGoal`.
pub struct HoldGroundAttackGoal {
    hostile_radius_sqr: f64,
}

impl HoldGroundAttackGoal {
    /// Vanilla `HoldGroundAttackGoal(AbstractIllager, float)`.
    #[must_use]
    pub fn new(hostile_radius: f32) -> Self {
        Self {
            hostile_radius_sqr: f64::from(hostile_radius) * f64::from(hostile_radius),
        }
    }
}

impl Goal for HoldGroundAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let Some(raider) = mob.as_raider() else {
            return false;
        };

        let last_hurt_by_is_player = mob
            .last_hurt_by_mob()
            .is_some_and(|attacker| attacker.as_player().is_some());

        raider.current_raid(&world).is_none()
            && raider.is_patrolling()
            && mob.target().is_some()
            && !mob.is_aggressive()
            && !last_hurt_by_is_player
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let Some(target) = mob.target() else {
            return;
        };
        shout_at_nearby_raiders(mob, &target);
        mob.set_aggressive(true);
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
        let Some(target) = mob.target() else {
            return;
        };
        shout_at_nearby_raiders(mob, &target);
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };

        let distance = mob.position().distance_squared(target.position());
        if distance > self.hostile_radius_sqr {
            let target_eye =
                DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
            mob.mob_base().controls().lock().look_control.set_look_at(
                target_eye,
                LOOK_Y_MAX_ROT_SPEED,
                LOOK_X_MAX_ROT_ANGLE,
            );
            if rand::random_range(0..50) == 0 {
                mob.play_ambient_sound();
            }
        } else {
            mob.set_aggressive(true);
        }
    }
}

/// Vanilla's shout loop: nearby raiders inherit the leader's target and become aggressive.
fn shout_at_nearby_raiders(mob: &dyn PathfinderMob, target: &SharedEntity) {
    let Some(world) = mob.level() else {
        return;
    };

    // Vanilla shouts through `TargetingConditions.forNonCombat().range(8)`.
    let area = mob.bounding_box().inflate(SHOUT_RANGE);
    let nearby = world.get_entities_in_aabb_matching(&area, |entity| {
        entity
            .as_raider()
            .is_some_and(|raider| raider.current_raid_id().is_some())
    });

    for entity in nearby {
        if entity.id() == mob.id() {
            continue;
        }
        if let Some(other) = entity.as_mob() {
            other.set_target(Some(target));
            other.set_aggressive(true);
        }
    }
}
