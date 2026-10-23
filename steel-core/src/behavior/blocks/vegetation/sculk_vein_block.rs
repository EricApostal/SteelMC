//! Vanilla `SculkVeinBlock`.
//!
//! Sculk veins are a multiface block that spreads across the surfaces around
//! sculk. The spread entry points are exposed for the sculk charge system:
//! [`SculkVeinBlock::regrow`] (place a vein on explicit faces),
//! [`SculkVeinBlock::attempt_spread_vein`] (the same-space spreader) and
//! [`SculkVeinBlock::on_discharged`] (drop the faces that touch sculk).
//!
//! Deviation: vanilla's `SculkBehaviour.attemptUseCharge` /
//! `updateDecayDelay` operate on a live `SculkSpreader` with `ChargeCursor`s
//! created by `SculkSpreader.createLevelSpreader()`; Steel only has the private,
//! `WorldGenRegion`-based spreader in `worldgen::feature::features::sculk_patch`,
//! so charge consumption stays with the catalyst block entity
//! (owner: worldgen/sculk).

use rand::Rng;
use steel_macros::block_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty, Direction};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId, types::UpdateFlags};

use crate::behavior::block::BlockBehavior;
use crate::behavior::blocks::MultifaceBlock;
use crate::behavior::blocks::multiface_face_property;
use crate::behavior::context::BlockPlaceContext;
use crate::world::{LevelAccessor, LevelReader, ScheduledTickAccess};

use super::BlockRef;

const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Vanilla `SculkVeinBlock`.
#[block_behavior]
pub struct SculkVeinBlock {
    multiface: MultifaceBlock,
}

impl SculkVeinBlock {
    /// Creates a new sculk vein block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            multiface: MultifaceBlock::new(block),
        }
    }

    /// Vanilla `SculkVeinBlock.regrow`: place a vein on every face in `faces`
    /// that can attach to a neighbour, preserving the existing waterlogged state.
    pub fn regrow(
        world: &dyn LevelAccessor,
        pos: BlockPos,
        existing: BlockStateId,
        faces: &[Direction],
    ) -> bool {
        let mut new_state = vanilla_blocks::SCULK_VEIN.default_state();
        let mut has_at_least_one_face = false;

        for face in faces {
            if MultifaceBlock::can_attach_to(world, pos, *face) {
                new_state = new_state.set_value(multiface_face_property(*face), true);
                has_at_least_one_face = true;
            }
        }

        if !has_at_least_one_face {
            return false;
        }

        if !existing.get_fluid_state().is_empty() {
            new_state = new_state.set_value(WATERLOGGED, true);
        }

        world.set_block_state(pos, new_state, UpdateFlags::UPDATE_ALL);
        true
    }

    /// Vanilla `SculkVeinBlock.onDischarged`: drop the faces that rest against
    /// sculk, and become air or water once no face is left.
    pub fn on_discharged(
        world: &dyn LevelAccessor,
        state: BlockStateId,
        pos: BlockPos,
        _rng: &mut dyn Rng,
    ) {
        if state.get_block() != &vanilla_blocks::SCULK_VEIN {
            return;
        }

        let mut state = state;
        for direction in Direction::ALL {
            let property = multiface_face_property(direction);
            if state.get_value(property)
                && world.get_block_state(pos.relative(direction)).get_block()
                    == &vanilla_blocks::SCULK
            {
                state = state.set_value(property, false);
            }
        }

        if !Direction::ALL
            .iter()
            .any(|dir| state.get_value(multiface_face_property(*dir)))
        {
            state = if world.get_block_state(pos).get_fluid_state().is_empty() {
                vanilla_blocks::AIR.default_state()
            } else {
                vanilla_blocks::WATER.default_state()
            };
        }

        world.set_block_state(pos, state, UpdateFlags::UPDATE_ALL);
    }

    /// Vanilla `SculkBehaviour.attemptSpreadVein` without explicit facings: the
    /// same-position spreader fills the remaining faces of this block.
    ///
    /// Deviation: vanilla uses a dedicated same-space `MultifaceSpreader`; this
    /// walks the faces directly because `MultifaceSpreader`'s placement helper is
    /// private to `vegetation::multiface_block` (owner: behavior/blocks).
    pub fn attempt_spread_vein(
        world: &dyn LevelAccessor,
        state: BlockStateId,
        pos: BlockPos,
        rng: &mut dyn Rng,
    ) -> bool {
        if state.get_block() != &vanilla_blocks::SCULK_VEIN {
            return false;
        }

        let mut placed = 0u32;
        for starting_face in Direction::ALL {
            if !MultifaceBlock::has_face(state, starting_face) {
                continue;
            }

            let mut spread_directions = Direction::ALL;
            Self::shuffle_directions(&mut spread_directions, rng);
            for spread_direction in spread_directions {
                if spread_direction.axis() == starting_face.axis()
                    || MultifaceBlock::has_face(state, spread_direction)
                {
                    continue;
                }

                if !Self::can_spread_into(world, pos, pos, spread_direction, state) {
                    continue;
                }

                let new_state = state.set_value(multiface_face_property(spread_direction), true);
                if world.set_block_state(pos, new_state, UpdateFlags::UPDATE_CLIENTS) {
                    placed += 1;
                }
            }
        }

        placed > 0
    }

    /// Vanilla `SculkVeinBlock.hasSubstrateAccess`: whether any occupied face
    /// rests against a `#minecraft:sculk_replaceable` block.
    #[must_use]
    pub fn has_substrate_access(
        world: &dyn LevelReader,
        state: BlockStateId,
        pos: BlockPos,
    ) -> bool {
        if state.get_block() != &vanilla_blocks::SCULK_VEIN {
            return false;
        }

        Direction::ALL.iter().any(|direction| {
            MultifaceBlock::has_face(state, *direction)
                && world
                    .get_block_state(pos.relative(*direction))
                    .get_block()
                    .has_tag(&BlockTag::SCULK_REPLACEABLE)
        })
    }

    /// Vanilla `SculkVeinSpreaderConfig.stateCanBeReplaced`.
    fn can_spread_into(
        world: &dyn LevelReader,
        source_pos: BlockPos,
        placement_pos: BlockPos,
        placement_direction: Direction,
        existing_state: BlockStateId,
    ) -> bool {
        let against = world.get_block_state(placement_pos.relative(placement_direction));
        if against.get_block() == &vanilla_blocks::SCULK
            || against.get_block() == &vanilla_blocks::SCULK_CATALYST
            || against.get_block() == &vanilla_blocks::MOVING_PISTON
        {
            return false;
        }

        let dx = source_pos.x() - placement_pos.x();
        let dy = source_pos.y() - placement_pos.y();
        let dz = source_pos.z() - placement_pos.z();
        if dx.abs() + dy.abs() + dz.abs() == 2 {
            let neighbour_pos = source_pos.relative(placement_direction.opposite());
            let neighbour_state = world.get_block_state(neighbour_pos);
            if world.is_face_sturdy(neighbour_state, neighbour_pos, placement_direction) {
                return false;
            }
        }

        let fluid_state = existing_state.get_fluid_state();
        if !fluid_state.is_empty() && !fluid_state.is_water() {
            return false;
        }

        if existing_state.get_block().has_tag(&BlockTag::FIRE) {
            return false;
        }

        existing_state.is_replaceable()
            || existing_state.get_block() == &vanilla_blocks::SCULK_VEIN
            || existing_state.is_air()
            || (existing_state.get_block() == &vanilla_blocks::WATER && fluid_state.is_source())
    }

    fn shuffle_directions(directions: &mut [Direction; 6], rng: &mut dyn Rng) {
        for i in (1..directions.len()).rev() {
            let j = rng.random_range(0..=i);
            directions.swap(i, j);
        }
    }
}

impl BlockBehavior for SculkVeinBlock {
    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        self.multiface
            .update_shape(state, world, pos, direction, neighbor_pos, neighbor_state)
    }

    fn can_survive(&self, state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        self.multiface.can_survive(state, world, pos)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.multiface.get_state_for_placement(context)
    }

    fn can_be_replaced(&self, state: BlockStateId, context: &BlockPlaceContext<'_>) -> bool {
        self.multiface.can_be_replaced(state, context)
    }
}
