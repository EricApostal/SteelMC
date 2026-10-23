//! Vanilla `Raider.HoldGroundAttackGoal`: an unengaged patrol leader stops, shouts at
//! nearby raiders to share its target, and holds position until the target closes in.

use glam::DVec3;

use super::selector::{Goal, GoalControls};
use crate::entity::ai::targeting::TargetingConditions;
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
            // Vanilla squares the float radius before the double comparison.
            hostile_radius_sqr: f64::from(hostile_radius * hostile_radius),
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
        shout_at_nearby_raiders(mob, &target, true);
        mob.set_aggressive(true);
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
        let Some(target) = mob.target() else {
            return;
        };
        shout_at_nearby_raiders(mob, &target, false);
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

/// Vanilla's shout loop: nearby raiders inherit the leader's target.
///
/// `set_aggressive` mirrors the two call sites: vanilla `start` only shares the target, while
/// `stop` also marks the shouted raiders aggressive.
fn shout_at_nearby_raiders(mob: &dyn PathfinderMob, target: &SharedEntity, set_aggressive: bool) {
    let Some(world) = mob.level() else {
        return;
    };

    // Vanilla `shoutTargeting`: `forNonCombat().range(8).ignoreLineOfSight().ignoreInvisibilityTesting()`.
    let shout_targeting = TargetingConditions::for_non_combat()
        .range(SHOUT_RANGE)
        .ignore_line_of_sight()
        .ignore_invisibility_testing();
    let area = mob.bounding_box().inflate(SHOUT_RANGE);
    let nearby = world.get_entities_in_aabb_matching(&area, |entity| {
        entity.as_raider().is_some()
            && entity
                .as_living_entity()
                .is_some_and(|raider| shout_targeting.test(world.as_ref(), Some(mob), raider))
    });

    for entity in nearby {
        if entity.id() == mob.id() {
            continue;
        }
        if let Some(other) = entity.as_mob() {
            let _ = other.set_target(Some(target));
            if set_aggressive {
                other.set_aggressive(true);
            }
        }
    }
}
