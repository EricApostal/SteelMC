//! Vanilla `TryFindLandNearLiquid`: find dry land adjacent to the given fluid.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::{BlockPos, Direction, Identifier};

use super::utils;
use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext};
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::world::LevelReader as _;

/// Vanilla `TryFindLandNearLiquid` cooldown between accepted searches.
const COOLDOWN_TICKS: i64 = 40;

/// Vanilla `TryFindLandNearLiquid(range, speedModifier, fluidTag)`.
pub struct TryFindLandNearLiquid {
    range: i32,
    speed_modifier: f32,
    fluid_tag: Identifier,
    next_ok_start_time: i64,
}

impl TryFindLandNearLiquid {
    /// Vanilla `TryFindLandNearLiquid.create(range, speedModifier, fluidTag)`.
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

    /// Vanilla's `findFirst` predicate: air with a solid floor and fluid below a
    /// horizontal neighbor.
    fn is_land_near_liquid(
        ctx: &BehaviorContext<'_>,
        pos: BlockPos,
        fluid_tag: &Identifier,
    ) -> bool {
        if !BLOCK_BEHAVIORS
            .get_behavior(ctx.level.get_block_state(pos).get_block())
            .get_collision_shape(
                ctx.level.get_block_state(pos),
                ctx.level,
                pos,
                BlockCollisionContext::empty(),
            )
            .is_empty()
        {
            return false;
        }

        let below = pos.below();
        if BLOCK_BEHAVIORS
            .get_behavior(ctx.level.get_block_state(below).get_block())
            .get_collision_shape(
                ctx.level.get_block_state(below),
                ctx.level,
                pos,
                BlockCollisionContext::empty(),
            )
            .is_empty()
        {
            return false;
        }

        for direction in Direction::HORIZONTAL {
            let neighbor = pos.relative(direction);
            if ctx.level.get_block_state(neighbor).is_air()
                && ctx
                    .level
                    .get_block_state(neighbor.below())
                    .get_fluid_state()
                    .fluid_id
                    .has_tag(fluid_tag)
            {
                return true;
            }
        }

        false
    }
}

impl Trigger for TryFindLandNearLiquid {
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
            self.next_ok_start_time = ctx.timestamp + COOLDOWN_TICKS;
            return true;
        }

        let fluid_tag = self.fluid_tag.clone();
        let found = body_pos.find_closest_match(self.range, self.range, |pos| {
            (pos.x() != body_pos.x() || pos.z() != body_pos.z())
                && Self::is_land_near_liquid(ctx, pos, &fluid_tag)
        });
        if let Some(target_pos) = found {
            utils::set_walk_and_look_target_to_pos(ctx, target_pos, self.speed_modifier, 0);
        }

        self.next_ok_start_time = ctx.timestamp + COOLDOWN_TICKS;
        true
    }
}
