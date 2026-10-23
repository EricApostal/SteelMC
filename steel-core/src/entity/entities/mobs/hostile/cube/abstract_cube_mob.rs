//! Vanilla `AbstractCubeMob` shared state
//! (`net.minecraft.world.entity.monster.cubemob.AbstractCubeMob`).
//!
//! Vanilla expresses the shared cube behaviour as an abstract superclass. Steel has no entity
//! inheritance and the synced size lives inside the generated per-entity data struct, so the
//! shared pieces that can be factored out are the size constants plus the two pieces of plain
//! runtime state the slime and magma cube both own: the squish animation and the
//! `CubeMobMoveControl` emulation. Each cube mob keeps its own copy and drives them from its own
//! tick, exactly like vanilla's two concrete subclasses share the abstract base.

/// Vanilla `AbstractCubeMob.MIN_SIZE`.
pub const MIN_CUBE_SIZE: i32 = 1;
/// Vanilla `AbstractCubeMob.MAX_SIZE`.
pub const MAX_CUBE_SIZE: i32 = 127;
/// Vanilla `AbstractCubeMob.setSize` movement-speed base.
pub const CUBE_MOVEMENT_SPEED_BASE: f64 = 0.2;
/// Vanilla `AbstractCubeMob.setSize` movement-speed per-size step.
pub const CUBE_MOVEMENT_SPEED_STEP: f64 = 0.1;
/// Vanilla `AbstractCubeMob.getSoundVolume` per-size factor.
pub const CUBE_SOUND_VOLUME_PER_SIZE: f32 = 0.4;
/// Vanilla `AbstractCubeMob.getJumpDelay` base bounds.
pub const JUMP_DELAY_BASE_MIN: i32 = 10;
pub const JUMP_DELAY_BASE_RANGE: i32 = 20;
/// Vanilla `AbstractCubeMob.remove` split count bounds.
pub const SPLIT_COUNT_MIN: i32 = 2;
pub const SPLIT_COUNT_RANGE: i32 = 3;
/// Vanilla `AbstractCubeMob.tick` squish interpolation factor.
const SQUISH_INTERPOLATION: f32 = 0.5;
/// Vanilla `AbstractCubeMob.decreaseSquish` factor.
const SQUISH_DECAY: f32 = 0.6;

/// Vanilla `AbstractCubeMob` squish animation state (`oSquish`/`squish`/`targetSquish`).
#[derive(Debug, Clone, Copy, Default)]
pub struct CubeSquish {
    /// Previous tick's squish value (`oSquish`).
    pub old: f32,
    /// Current squish value (`squish`).
    pub current: f32,
    /// Target squish value (`targetSquish`).
    pub target: f32,
}

impl CubeSquish {
    /// Creates default squish state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            old: 0.0,
            current: 0.0,
            target: 0.0,
        }
    }

    /// Vanilla `AbstractCubeMob.tick`: `squish += (targetSquish - squish) * 0.5F`.
    pub fn tick(&mut self) {
        self.old = self.current;
        self.current += (self.target - self.current) * SQUISH_INTERPOLATION;
    }

    /// Vanilla `AbstractCubeMob.decreaseSquish`.
    pub fn decrease_squish(&mut self) {
        self.target *= SQUISH_DECAY;
    }
}

/// Vanilla `AbstractCubeMob.CubeMobMoveControl` server-side state.
///
/// Vanilla installs a `MoveControl` subclass that turns the cube toward a chosen yaw and jumps
/// on a delay; Steel has no per-entity move-control subclasses, so the same fields are owned by
/// the entity and applied from its tick.
#[derive(Debug, Clone, Copy)]
pub struct CubeMoveState {
    /// Vanilla `CubeMobMoveControl.yRot`.
    pub y_rot: f32,
    /// Vanilla `CubeMobMoveControl.jumpDelay`.
    pub jump_delay: i32,
    /// Vanilla `CubeMobMoveControl.isAggressive`.
    pub is_aggressive: bool,
    /// Vanilla `MoveControl.speedModifier` set by `setWantedMovement`.
    pub wanted_speed: Option<f64>,
}

impl CubeMoveState {
    /// Creates idle cube movement state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            y_rot: 0.0,
            jump_delay: 0,
            is_aggressive: false,
            wanted_speed: None,
        }
    }

    /// Vanilla `CubeMobMoveControl.setDirection`.
    pub const fn set_direction(&mut self, y_rot: f32, is_aggressive: bool) {
        self.y_rot = y_rot;
        self.is_aggressive = is_aggressive;
    }

    /// Vanilla `CubeMobMoveControl.setWantedMovement`.
    pub const fn set_wanted_movement(&mut self, speed_modifier: f64) {
        self.wanted_speed = Some(speed_modifier);
    }

    /// Vanilla `CubeMobMoveControl.tick` jump-delay decrement.
    pub const fn tick_jump_delay(&mut self, base_delay: i32) -> bool {
        if self.jump_delay <= 0 {
            self.jump_delay = base_delay;
            if self.is_aggressive {
                self.jump_delay /= 3;
            }
            true
        } else {
            self.jump_delay -= 1;
            false
        }
    }
}

impl Default for CubeMoveState {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `AbstractCubeMob.getSoundPitch` (`tiny ? 1.4 : 0.8` factor).
#[must_use]
pub fn cube_sound_pitch(is_tiny: bool) -> f32 {
    let pitch_adjuster = if is_tiny { 1.4 } else { 0.8 };
    ((rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0) * pitch_adjuster
}

/// Vanilla `AbstractCubeMob.setSize` clamp.
#[must_use]
pub fn clamp_cube_size(size: i32) -> i32 {
    size.clamp(MIN_CUBE_SIZE, MAX_CUBE_SIZE)
}

/// Vanilla `AbstractCubeMob.setSize` movement speed for a size.
#[must_use]
pub const fn cube_movement_speed(size: i32) -> f64 {
    CUBE_MOVEMENT_SPEED_BASE + CUBE_MOVEMENT_SPEED_STEP * size as f64
}

/// Vanilla `AbstractCubeMob.setCubeMobHealth`: max health is `size * size`.
#[must_use]
pub const fn cube_max_health(size: i32) -> f64 {
    (size * size) as f64
}

/// Vanilla `AbstractCubeMob.getSoundVolume` (`0.4F * size`).
#[must_use]
pub const fn cube_sound_volume(size: i32) -> f32 {
    CUBE_SOUND_VOLUME_PER_SIZE * size as f32
}

/// Vanilla `AbstractCubeMob.getJumpDelay` (`random.nextInt(20) + 10`).
#[must_use]
pub fn cube_jump_delay() -> i32 {
    JUMP_DELAY_BASE_MIN + rand::random_range(0..JUMP_DELAY_BASE_RANGE)
}

/// Vanilla `AbstractCubeMob.getSplitCount` (`2 + random.nextInt(3)`).
#[must_use]
pub fn cube_split_count() -> i32 {
    SPLIT_COUNT_MIN + rand::random_range(0..SPLIT_COUNT_RANGE)
}

#[cfg(test)]
mod tests {
    use super::{CubeMoveState, CubeSquish, clamp_cube_size, cube_max_health};

    #[test]
    fn cube_size_helpers_match_vanilla() {
        assert_eq!(clamp_cube_size(0), 1);
        assert_eq!(clamp_cube_size(200), 127);
        assert_eq!(cube_max_health(3), 9.0);
    }

    #[test]
    fn squish_interpolates_toward_target() {
        let mut squish = CubeSquish::new();
        squish.target = 1.0;
        squish.tick();
        assert!((squish.current - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn aggressive_cubes_jump_three_times_sooner() {
        let mut state = CubeMoveState::new();
        state.is_aggressive = true;
        assert!(state.tick_jump_delay(9));
        assert_eq!(state.jump_delay, 3);
    }
}
