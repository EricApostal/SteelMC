//! Vanilla `SculkBlock`.
//!
//! Vanilla reference: `net.minecraft.world.level.block.SculkBlock`. `SculkBlock` only adds two
//! things on top of `DropExperienceBlock`: the `SculkBehaviour` charge-cursor spread rule and a
//! `ConstantInt.of(1)` experience drop. The spread rule is `SculkBlock::attempt_use_charge`,
//! driven by the live-level `SculkSpreader` the catalyst block entity updates.

use std::sync::Arc;

use rand::Rng;
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_fluids;
use steel_utils::value_providers::IntProvider;
use steel_utils::{BlockPos, BlockStateId, types::UpdateFlags};

use crate::behavior::blocks::next_int_bounded;
use crate::behavior::blocks::sculk::spreader::{ChargeCursor, SculkSpreader};
use crate::behavior::{BlockBehavior, BlockPlaceContext, try_drop_experience};
use crate::world::World;

/// Vanilla `SculkBlock.GROWTH_INHIBITOR_RANGE`.
pub const SCULK_GROWTH_INHIBITOR_RANGE: i32 = 4;

/// Vanilla `SculkBlock`.
#[block_behavior]
pub struct SculkBlock {
    block: BlockRef,
}

impl SculkBlock {
    /// Creates sculk behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `SculkBlock.attemptUseCharge`.
    ///
    /// One `chargeDecayRate` draw in ten consumes charge: either it grows a sculk sensor/shrieker
    /// on top of the cursor (costing `growthSpawnCost`) when the cursor is outside the
    /// `noGrowthRadius` ball and the spot is free of `#sculk_growth_inhibitors`, or it decays by
    /// one charge near the catalyst / by `getDecayPenalty` further out after an
    /// `additionalDecayRate` draw.
    pub fn attempt_use_charge(
        world: &Arc<World>,
        rng: &mut dyn Rng,
        origin: BlockPos,
        spreader: &SculkSpreader,
        cursor: &ChargeCursor,
    ) -> i32 {
        let charge = cursor.charge();
        if charge == 0 || next_int_bounded(rng, spreader.charge_decay_rate()) != 0 {
            return charge;
        }

        let cursor_pos = cursor.pos();
        let close_to_catalyst = is_closer_than(cursor_pos, origin, spreader.no_growth_radius());
        if !close_to_catalyst && Self::can_place_growth(world, cursor_pos) {
            let growth_spawn_cost = spreader.growth_spawn_cost();
            if next_int_bounded(rng, growth_spawn_cost) < charge {
                let growth_pos = cursor_pos.above();
                let growth_state = Self::random_growth_state(
                    world,
                    rng,
                    growth_pos,
                    spreader.is_world_generation(),
                );
                world.set_block(growth_pos, growth_state, UpdateFlags::UPDATE_ALL);
                // Vanilla plays the growth state's sound type place sound at the cursor.
                world.play_sound(
                    growth_state.get_block().config.sound_type.place_sound,
                    SoundSource::Blocks,
                    cursor_pos,
                    1.0,
                    1.0,
                    None,
                );
            }
            return (charge - growth_spawn_cost).max(0);
        }

        if next_int_bounded(rng, spreader.additional_decay_rate()) != 0 {
            return charge;
        }
        if close_to_catalyst {
            return charge - 1;
        }
        charge - Self::decay_penalty(spreader, cursor_pos, origin, charge)
    }

    /// Vanilla `SculkBlock.getDecayPenalty`.
    fn decay_penalty(
        spreader: &SculkSpreader,
        pos: BlockPos,
        origin: BlockPos,
        charge: i32,
    ) -> i32 {
        let no_growth_radius = spreader.no_growth_radius() as f32;
        let distance_squared = f64::from(pos.x() - origin.x()).powi(2)
            + f64::from(pos.y() - origin.y()).powi(2)
            + f64::from(pos.z() - origin.z()).powi(2);
        let distance = distance_squared.sqrt() as f32;
        let outer_distance_squared = (distance - no_growth_radius).powi(2);
        let max_reach = 24 - spreader.no_growth_radius();
        let max_reach_squared = (max_reach * max_reach) as f32;
        let factor = (outer_distance_squared / max_reach_squared).min(1.0);
        (((charge as f32) * factor * 0.5) as i32).max(1)
    }

    /// Vanilla `SculkBlock.getRandomGrowthState`: one in eleven is a shrieker, the rest sensors,
    /// waterlogged when the target block holds fluid.
    fn random_growth_state(
        world: &Arc<World>,
        rng: &mut dyn Rng,
        pos: BlockPos,
        is_world_generation: bool,
    ) -> BlockStateId {
        let state = if next_int_bounded(rng, 11) == 0 {
            vanilla_blocks::SCULK_SHRIEKER
                .default_state()
                .set_value(&BlockStateProperties::CAN_SUMMON, is_world_generation)
        } else {
            vanilla_blocks::SCULK_SENSOR.default_state()
        };

        if state
            .try_get_value(&BlockStateProperties::WATERLOGGED)
            .is_some()
            && !world.get_block_state(pos).get_fluid_state().is_empty()
        {
            state.set_value(&BlockStateProperties::WATERLOGGED, true)
        } else {
            state
        }
    }

    /// Vanilla `SculkBlock.canPlaceGrowth`.
    fn can_place_growth(world: &Arc<World>, pos: BlockPos) -> bool {
        let state_above = world.get_block_state(pos.above());
        let fluid_above = state_above.get_fluid_state();
        let can_grow_above = state_above.is_air()
            || (state_above.get_block() == &vanilla_blocks::WATER
                && fluid_above.is_source_of_type(&vanilla_fluids::WATER));
        if !can_grow_above {
            return false;
        }

        let mut growth_count = 0;
        for z in -SCULK_GROWTH_INHIBITOR_RANGE..=SCULK_GROWTH_INHIBITOR_RANGE {
            for y in 0..=2 {
                for x in -SCULK_GROWTH_INHIBITOR_RANGE..=SCULK_GROWTH_INHIBITOR_RANGE {
                    let state = world.get_block_state(pos.offset(x, y, z));
                    if state
                        .get_block()
                        .has_tag(&BlockTag::SCULK_GROWTH_INHIBITORS)
                    {
                        growth_count += 1;
                    }
                    // Vanilla `BlockMatcher.atMostMatched(2)`.
                    if growth_count > 2 {
                        return false;
                    }
                }
            }
        }

        true
    }
}

/// Vanilla `BlockPos.closerThan(Vec3i, double)`.
fn is_closer_than(pos: BlockPos, origin: BlockPos, radius: i32) -> bool {
    let dx = i64::from(pos.x()) - i64::from(origin.x());
    let dy = i64::from(pos.y()) - i64::from(origin.y());
    let dz = i64::from(pos.z()) - i64::from(origin.z());
    dx * dx + dy * dy + dz * dz < i64::from(radius) * i64::from(radius)
}

impl BlockBehavior for SculkBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn spawn_after_break(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        tool: &ItemStack,
        drop_experience: bool,
    ) {
        if drop_experience {
            // Vanilla `SculkBlock` constructor: `ConstantInt.of(1)`.
            try_drop_experience(world, pos, tool, &IntProvider::Constant(1));
        }
    }
}
