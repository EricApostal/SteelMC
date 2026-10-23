//! Vanilla `WaterAvoidingRandomFlyingGoal` (`goal/WaterAvoidingRandomFlyingGoal.java`): a
//! flying mob wanders toward its view direction, preferring to hover above solid ground.
//!
//! Vanilla extends `WaterAvoidingRandomStrollGoal` and overrides `getPosition`; Steel embeds
//! the landed [`RandomStrollGoal`] and supplies the same position supplier, which is exactly
//! the hook `WaterAvoidingRandomStrollGoal` uses.

use glam::DVec3;

use super::random_pos::{air_and_water_random_pos, hover_random_pos};
use super::random_stroll::RandomStrollGoal;
use super::selector::{Goal, GoalControls};
use crate::entity::PathfinderMob;

/// Vanilla `WaterAvoidingRandomFlyingGoal.getPosition` horizontal distance.
const FLYING_DISTANCE: i32 = 8;
/// Vanilla `WaterAvoidingRandomFlyingGoal.getPosition` hover bounds.
const HOVER_VERTICAL_DISTANCE: i32 = 7;
const HOVER_MAX_HEIGHT: i32 = 3;
const HOVER_MIN_HEIGHT: i32 = 1;
/// Vanilla `AirAndWaterRandomPos.getPos(mob, 8, 4, -2, ...)` bounds.
const AIR_VERTICAL_DISTANCE: i32 = 4;
const AIR_FLYING_HEIGHT: i32 = -2;

/// Vanilla `WaterAvoidingRandomFlyingGoal`.
pub struct WaterAvoidingRandomFlyingGoal {
    stroll: RandomStrollGoal,
}

impl WaterAvoidingRandomFlyingGoal {
    /// Vanilla `new WaterAvoidingRandomFlyingGoal(mob, speedModifier)`.
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            stroll: RandomStrollGoal::new(speed_modifier),
        }
    }

    /// Vanilla `WaterAvoidingRandomFlyingGoal.getPosition`.
    fn flying_position(mob: &dyn PathfinderMob) -> Option<DVec3> {
        // Vanilla `Entity.getViewVector(0.0F)`, reconstructed from the entity rotation.
        let (yaw, pitch) = mob.rotation();
        let yaw = f64::from(yaw.to_radians());
        let pitch = f64::from(pitch.to_radians());
        let view = DVec3::new(
            -yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        );

        hover_random_pos(
            mob,
            FLYING_DISTANCE,
            HOVER_VERTICAL_DISTANCE,
            view.x,
            view.z,
            std::f64::consts::FRAC_PI_2,
            HOVER_MAX_HEIGHT,
            HOVER_MIN_HEIGHT,
        )
        .or_else(|| {
            air_and_water_random_pos(
                mob,
                FLYING_DISTANCE,
                AIR_VERTICAL_DISTANCE,
                AIR_FLYING_HEIGHT,
                view.x,
                view.z,
                std::f64::consts::FRAC_PI_2,
            )
        })
    }
}

impl Goal for WaterAvoidingRandomFlyingGoal {
    fn controls(&self) -> GoalControls {
        self.stroll.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll
            .can_use_with_position(mob, Self::flying_position)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.stop(mob);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_avoiding_random_flying_goal_uses_the_stroll_controls() {
        let goal = WaterAvoidingRandomFlyingGoal::new(1.0);

        assert_eq!(goal.controls(), GoalControls::MOVE);
    }
}
