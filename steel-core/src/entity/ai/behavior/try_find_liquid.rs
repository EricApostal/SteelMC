//! Vanilla `TryFindLiquid`: walk to a liquid block that is reachable from above.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;
use steel_utils::Identifier;

use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::world::LevelReader as _;

/// Vanilla `TryFindLiquid`'s shorter re-check delay (`timestamp + 20L + 2L`).
const SHORT_COOLDOWN_TICKS: i64 = 22;
/// Vanilla `TryFindLiquid`'s post-search cooldown.
const COOLDOWN_TICKS: i64 = 40;
/// Vanilla `pos.closerToCenterThan(body.position(), 1.5)`.
const CLOSE_TO_CENTER_DISTANCE: f64 = 1.5;

/// Vanilla `TryFindLiquid(range, speedModifier, fluidTag)`.
pub struct TryFindLiquid {
    range: i32,
    speed_modifier: f32,
    fluid_tag: Identifier,
    next_ok_start_time: i64,
}

impl TryFindLiquid {
    /// Vanilla `TryFindLiquid.create(range, speedModifier, fluidTag)`.
    #[must_use]
    pub fn create(range: i32, speed_modifier: f32, fluid_tag: Identifier) -> OneShot<Self> {
        OneShot::new(Self {
            range,
            speed_modifier,
            fluid_tag,
            next_ok_start_time: 0,
        })
        .with_required_memories(vec![
            &mem::ATTACK_TARGET,
            &mem::WALK_TARGET,
            &mem::LOOK_TARGET,
        ])
    }
}

impl Trigger for TryFindLiquid {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let body_pos = ctx.body.block_position();
        if ctx
            .level
            .get_block_state(body_pos)
            .get_fluid_state()
            .fluid_id
            .has_tag(&self.fluid_tag)
        {
            return false;
        }

        if ctx.timestamp < self.next_ok_start_time {
            self.next_ok_start_time = ctx.timestamp + SHORT_COOLDOWN_TICKS;
            return true;
        }

        // Vanilla `forEachUntil` over the manhattan-ordered box: abort on the first
        // liquid with air above, otherwise remember the first liquid the body is
        // not already standing in.
        let mut found_pos = None;
        for pos in body_pos.within_manhattan(self.range, self.range, self.range) {
            if pos.x() == body_pos.x() && pos.z() == body_pos.z() {
                continue;
            }

            let state = ctx.level.get_block_state(pos);
            if state.get_block().key != vanilla_blocks::WATER.key
                && !state.get_fluid_state().fluid_id.has_tag(&self.fluid_tag)
            {
                continue;
            }

            if ctx.level.get_block_state(pos.above()).is_air() {
                found_pos = Some(pos);
                break;
            }

            if found_pos.is_none()
                && !pos.closer_to_center_than(ctx.body.position(), CLOSE_TO_CENTER_DISTANCE)
            {
                found_pos = Some(pos);
            }
        }

        if let Some(target_pos) = found_pos {
            utils::set_walk_and_look_target_to_pos(ctx, target_pos, self.speed_modifier, 0);
        }

        self.next_ok_start_time = ctx.timestamp + COOLDOWN_TICKS;
        true
    }
}
