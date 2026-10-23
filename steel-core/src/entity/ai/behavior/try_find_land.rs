//! Vanilla `TryFindLand`: a water-bound mob swims to the nearest dry standing spot.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::fluid::FluidStateExt as _;
use steel_utils::{BlockPos, Direction};

use super::utils;
use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext};
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::world::LevelReader as _;

/// Vanilla `TryFindLand.COOLDOWN_TICKS`.
const COOLDOWN_TICKS: i64 = 60;

/// Vanilla `TryFindLand`.
pub struct TryFindLand {
    range: i32,
    speed_modifier: f32,
    next_ok_start_time: i64,
}

impl TryFindLand {
    /// Vanilla `TryFindLand.create(range, speedModifier)`.
    #[must_use]
    pub fn create(range: i32, speed_modifier: f32) -> OneShot<Self> {
        OneShot::new(Self {
            range,
            speed_modifier,
            next_ok_start_time: 0,
        })
        .with_required_memories(vec![
            &mem::ATTACK_TARGET,
            &mem::WALK_TARGET,
            &mem::LOOK_TARGET,
        ])
    }

    /// Vanilla `canStandOn`: no collision at the position and a sturdy block below.
    fn can_stand_on(ctx: &BehaviorContext<'_>, pos: BlockPos) -> bool {
        let state = ctx.level.get_block_state(pos);
        let shape = BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .get_collision_shape(state, ctx.level, pos, BlockCollisionContext::empty());
        if !shape.is_empty() {
            return false;
        }

        let below = pos.below();
        let below_state = ctx.level.get_block_state(below);
        ctx.level.is_face_sturdy(below_state, below, Direction::Up)
    }
}

impl Trigger for TryFindLand {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let body_pos = ctx.body.block_position();
        if !ctx
            .level
            .get_block_state(body_pos)
            .get_fluid_state()
            .is_water()
        {
            return false;
        }

        if ctx.timestamp < self.next_ok_start_time {
            self.next_ok_start_time = ctx.timestamp + COOLDOWN_TICKS;
            return true;
        }

        let found = body_pos.find_closest_match(self.range, self.range, |pos| {
            (pos.x() != body_pos.x() || pos.z() != body_pos.z()) && Self::can_stand_on(ctx, pos)
        });
        if let Some(target_pos) = found {
            utils::set_walk_and_look_target_to_pos(ctx, target_pos, self.speed_modifier, 1);
        }

        self.next_ok_start_time = ctx.timestamp + COOLDOWN_TICKS;
        true
    }
}
