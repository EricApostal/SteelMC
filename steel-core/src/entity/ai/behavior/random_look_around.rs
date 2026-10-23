//! Vanilla `RandomLookAround`: glance in a random direction, then cool down.

use std::sync::Arc;

use glam::DVec3;
use steel_utils::value_providers::IntProvider;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::{BlockPosTracker, PositionTracker};

/// Vanilla `RandomLookAround(IntProvider, float, float, float)`.
pub struct RandomLookAround {
    state: BehaviorState,
    interval: IntProvider,
    max_yaw: f32,
    min_pitch: f32,
    pitch_range: f32,
}

impl RandomLookAround {
    /// Vanilla `RandomLookAround(interval, maxYaw, minPitch, maxPitch)`.
    ///
    /// # Panics
    ///
    /// Panics when `min_pitch > max_pitch`, mirroring vanilla's constructor.
    #[must_use]
    pub fn new(interval: IntProvider, max_yaw: f32, min_pitch: f32, max_pitch: f32) -> Self {
        assert!(
            min_pitch <= max_pitch,
            "Minimum pitch is larger than maximum pitch! {min_pitch} > {max_pitch}"
        );

        let mut state = BehaviorState::with_entry_condition(Vec::new());
        state
            .entry_condition
            .push((&mem::LOOK_TARGET, MemoryStatus::ValueAbsent));
        state
            .entry_condition
            .push((&mem::GAZE_COOLDOWN_TICKS, MemoryStatus::ValueAbsent));

        Self {
            state,
            interval,
            max_yaw,
            min_pitch,
            pitch_range: max_pitch - min_pitch,
        }
    }

    /// Vanilla samples the provider with `body.getRandom()`. Steel's behavior RNG
    /// convention is `rand`, so the provider is driven by a `LegacyRandom` seeded
    /// from it: same distribution, different source.
    fn sample_interval(&self) -> i32 {
        let mut random =
            steel_utils::random::legacy_random::LegacyRandom::from_seed(rand::random());
        self.interval.sample(&mut random)
    }
}

/// Vanilla `Mth.wrapDegrees`.
fn wrap_degrees(degrees: f32) -> f32 {
    let mut wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

impl Behavior for RandomLookAround {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let pitch = (rand::random::<f32>() * self.pitch_range + self.min_pitch).clamp(-90.0, 90.0);
        let (yaw, _) = ctx.body.rotation();
        let rotation =
            wrap_degrees(yaw + 2.0 * rand::random::<f32>() * self.max_yaw - self.max_yaw);
        let new_look_vec = ctx.body.calculate_view_vector(pitch, rotation);

        let body_pos = ctx.body.position();
        let eye_position = DVec3::new(body_pos.x, ctx.body.get_eye_y(), body_pos.z);
        ctx.set_memory(
            &mem::LOOK_TARGET,
            Arc::new(BlockPosTracker::from_vec(eye_position + new_look_vec))
                as Arc<dyn PositionTracker>,
        );
        ctx.set_memory(&mem::GAZE_COOLDOWN_TICKS, self.sample_interval());
    }
}
