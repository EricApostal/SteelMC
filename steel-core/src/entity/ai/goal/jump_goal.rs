//! Vanilla `JumpGoal` and `DolphinJumpGoal` (`goal/JumpGoal.java`, `goal/DolphinJumpGoal.java`).
//!
//! `JumpGoal` is vanilla's abstract base (the flags only); Steel exposes it as a concrete
//! goal so dolphins and any other jumping mob can embed it like the other base goals.
//! `DolphinJumpGoal` adds the water-clear, surface-clear, breach and rotation logic, driven
//! through the mob's navigation/velocity/rotation accessors.

use std::cmp::Ordering;
use std::f64::consts::PI;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::{sound_events, vanilla_block_tags::BlockTag};

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `DolphinJumpGoal.STEPS_TO_CHECK`.
const STEPS_TO_CHECK: [i32; 6] = [0, 1, 4, 5, 6, 7];
/// Vanilla `DolphinJumpGoal.canContinueToUse` damped-pitch threshold (`y^2 < 0.03`).
const PITCH_DAMP_LIMIT_SQR: f32 = 0.03;
/// Vanilla `DolphinJumpGoal.canContinueToUse` pitch latch limit in degrees.
const PITCH_LATCH_LIMIT_DEGREES: f32 = 10.0;
/// Vanilla `DolphinJumpGoal.tick` rotation approach rate.
const ROTATION_LERP_RATE: f32 = 0.2;

/// Vanilla `JumpGoal` (abstract base): MOVE + JUMP flags, no behaviour.
#[derive(Default)]
pub struct JumpGoal;

impl JumpGoal {
    /// Vanilla `new JumpGoal()`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Goal for JumpGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::JUMP
    }

    /// Vanilla `JumpGoal` is abstract; the concrete Steel form never activates on its own.
    fn can_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }
}

/// Vanilla `DolphinJumpGoal`.
pub struct DolphinJumpGoal {
    base: JumpGoal,
    interval: i32,
    breached: bool,
}

impl DolphinJumpGoal {
    /// Vanilla `new DolphinJumpGoal(dolphin, interval)`.
    #[must_use]
    pub const fn new(interval: i32) -> Self {
        Self {
            base: JumpGoal::new(),
            interval: reduced_tick_delay(interval),
            breached: false,
        }
    }

    /// Vanilla `DolphinJumpGoal.waterIsClear`.
    fn water_is_clear(
        mob: &dyn PathfinderMob,
        dolphin_pos: steel_utils::BlockPos,
        step: i32,
    ) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        let (motion_x, motion_z) = motion_direction_steps(mob);
        let pos = dolphin_pos.offset(motion_x * step, 0, motion_z * step);
        let state = world.get_block_state(pos);
        state.get_fluid_state().is_water()
            && !state.get_block().has_tag(&BlockTag::BLOCKS_DOLPHIN_JUMP)
    }

    /// Vanilla `DolphinJumpGoal.surfaceIsClear`.
    fn surface_is_clear(
        mob: &dyn PathfinderMob,
        dolphin_pos: steel_utils::BlockPos,
        step: i32,
    ) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        let (motion_x, motion_z) = motion_direction_steps(mob);
        world
            .get_block_state(dolphin_pos.offset(motion_x * step, 1, motion_z * step))
            .is_air()
            && world
                .get_block_state(dolphin_pos.offset(motion_x * step, 2, motion_z * step))
                .is_air()
    }
}

impl Goal for DolphinJumpGoal {
    fn controls(&self) -> GoalControls {
        self.base.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        // Vanilla stores `reducedTickDelay(interval)` in the constructor and rolls
        // `nextInt(this.interval)` against that stored value.
        if rand::random_range(0..self.interval) != 0 {
            return false;
        }

        let dolphin_pos = mob.block_position();
        STEPS_TO_CHECK.iter().all(|step| {
            Self::water_is_clear(mob, dolphin_pos, *step)
                && Self::surface_is_clear(mob, dolphin_pos, *step)
        })
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let velocity = mob.velocity();
        let y = velocity.y as f32;
        let pitch = mob.rotation().1;
        ((y * y).partial_cmp(&PITCH_DAMP_LIMIT_SQR) != Some(Ordering::Less)
            || pitch != 0.0
            || pitch.abs() >= PITCH_LATCH_LIMIT_DEGREES
            || !mob.is_in_water())
            && !mob.on_ground()
    }

    fn is_interruptable(&self) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let (motion_x, motion_z) = motion_direction_steps(mob);
        let velocity = mob.velocity();
        mob.set_velocity(
            velocity + glam::DVec3::new(f64::from(motion_x) * 0.6, 0.7, f64::from(motion_z) * 0.6),
        );
        mob.mob_base().navigation().lock().stop();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let (yaw, _) = mob.rotation();
        mob.set_rotation((yaw, 0.0));
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if !self.breached
            && let Some(world) = mob.level()
        {
            let in_water = world
                .get_block_state(mob.block_position())
                .get_fluid_state()
                .is_water();
            self.breached = in_water;
            if in_water {
                mob.play_sound(&sound_events::ENTITY_DOLPHIN_JUMP, 1.0, 1.0);
            }
        }

        let velocity = mob.velocity();
        let y = velocity.y as f32;
        let (yaw, pitch) = mob.rotation();
        if y * y < PITCH_DAMP_LIMIT_SQR && pitch != 0.0 {
            mob.set_rotation((yaw, rot_lerp(ROTATION_LERP_RATE, pitch, 0.0)));
        } else if velocity.length() > 1.0e-5 {
            let horizontal = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt();
            let rotation = (-velocity.y / horizontal).atan() * 180.0 / PI;
            mob.set_rotation((yaw, rotation as f32));
        }
    }
}

/// Vanilla `Dolphin.getMotionDirection` step pair.
fn motion_direction_steps(mob: &dyn PathfinderMob) -> (i32, i32) {
    mob.direction_yaw().offset_xz()
}

/// Vanilla `Mth.rotLerp`: a wrapped interpolation toward `to` by `delta`.
fn rot_lerp(delta: f32, from: f32, to: f32) -> f32 {
    from + delta * steel_math::wrap_degrees(to - from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_goal_uses_move_and_jump_controls() {
        let goal = JumpGoal::new();

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::JUMP);
        assert!(goal.is_interruptable());
    }

    #[test]
    fn dolphin_jump_goal_is_not_interruptable() {
        let goal = DolphinJumpGoal::new(100);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::JUMP);
        assert!(!goal.is_interruptable());
    }
}
