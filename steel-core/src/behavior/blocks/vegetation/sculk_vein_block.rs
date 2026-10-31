//! Vanilla `SculkVeinBlock`.
//!
//! Sculk veins are a multiface block that spreads across the surfaces around sculk. This module
//! owns both the block behavior and the sculk-vein `MultifaceSpreader` configuration vanilla keeps
//! in `SculkVeinBlock.SculkVeinSpreaderConfig`:
//!
//! * [`SculkVeinBlock::spread_veins`] — vanilla `MultifaceSpreader.spreadAll` for the default
//!   spread order (`SAME_POSITION`, `SAME_PLANE`, `WRAP_AROUND`) or the same-space order.
//! * [`SculkVeinBlock::attempt_place_sculk`] — vanilla `SculkVeinBlock.attemptPlaceSculk`, which
//!   converts a `#minecraft:sculk_replaceable` neighbour into sculk and spreads veins from it.
//! * [`SculkVeinBlock::attempt_use_charge`] — vanilla `SculkVeinBlock.attemptUseCharge`.
//! * [`SculkVeinBlock::regrow`], [`SculkVeinBlock::on_discharged`],
//!   [`SculkVeinBlock::has_substrate_access`].
//!
//! The live-level `SculkSpreader` driving these is `behavior::blocks::sculk::spreader`.

use std::sync::Arc;

use rand::Rng;
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty, Direction};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::sound_events;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_fluids;
use steel_utils::{BlockPos, BlockStateId, types::UpdateFlags};

use crate::behavior::block::BlockBehavior;
use crate::behavior::blocks::MultifaceBlock;
use crate::behavior::blocks::multiface_face_property;
use crate::behavior::blocks::next_int_bounded;
use crate::behavior::blocks::sculk::spreader::{ChargeCursor, SculkSpreader};
use crate::behavior::blocks::vegetation::multiface_block::{
    MultifaceSpreadPos, MultifaceSpreadType, multiface_spread_pos,
};
use crate::behavior::context::BlockPlaceContext;
use crate::world::{LevelAccessor, LevelReader, ScheduledTickAccess, World};

use super::BlockRef;

const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;

/// Vanilla `MultifaceSpreader.DEFAULT_SPREAD_ORDER`.
const DEFAULT_SPREAD_TYPES: [MultifaceSpreadType; 3] = [
    MultifaceSpreadType::SamePosition,
    MultifaceSpreadType::SamePlane,
    MultifaceSpreadType::WrapAround,
];
/// Vanilla `SculkVeinBlock`'s same-space spreader (`SpreadType.SAME_POSITION` only).
const SAME_SPACE_SPREAD_TYPES: [MultifaceSpreadType; 1] = [MultifaceSpreadType::SamePosition];

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

    /// Vanilla `MultifaceSpreader.spreadAll` with the sculk-vein spreader config.
    ///
    /// Returns the number of placed faces. `same_space_only` selects the same-space spreader
    /// (`SAME_POSITION` only) instead of `DEFAULT_SPREAD_ORDER`.
    pub fn spread_veins(
        world: &dyn LevelAccessor,
        state: BlockStateId,
        pos: BlockPos,
        same_space_only: bool,
    ) -> i64 {
        let mut count = 0;
        for starting_face in Direction::ALL {
            if !Self::can_spread_from(state, starting_face) {
                continue;
            }
            for spread_direction in Direction::ALL {
                let Some(spread_pos) = Self::get_spread_from_face_toward_direction(
                    world,
                    state,
                    pos,
                    starting_face,
                    spread_direction,
                    same_space_only,
                ) else {
                    continue;
                };
                if Self::spread_to_face(world, &spread_pos) {
                    count += 1;
                }
            }
        }
        count
    }

    /// Vanilla `MultifaceSpreader.getSpreadFromFaceTowardDirection`.
    fn get_spread_from_face_toward_direction(
        world: &dyn LevelAccessor,
        state: BlockStateId,
        pos: BlockPos,
        starting_face: Direction,
        spread_direction: Direction,
        same_space_only: bool,
    ) -> Option<MultifaceSpreadPos> {
        if spread_direction.axis() == starting_face.axis() {
            return None;
        }

        if !Self::is_other_block_valid_as_source(state)
            && (!MultifaceBlock::has_face(state, starting_face)
                || MultifaceBlock::has_face(state, spread_direction))
        {
            return None;
        }

        let spread_types = if same_space_only {
            SAME_SPACE_SPREAD_TYPES.as_slice()
        } else {
            DEFAULT_SPREAD_TYPES.as_slice()
        };
        for spread_type in spread_types {
            let spread_pos =
                multiface_spread_pos(pos, spread_direction, starting_face, *spread_type);
            if Self::can_spread_into(world, pos, &spread_pos) {
                return Some(spread_pos);
            }
        }

        None
    }

    /// Vanilla `MultifaceSpreader.spreadToFace`: place the veil state and report whether it was
    /// written.
    fn spread_to_face(world: &dyn LevelAccessor, spread_pos: &MultifaceSpreadPos) -> bool {
        let old_state = world.get_block_state(spread_pos.pos);
        let Some(new_state) =
            Self::state_for_placement(world, old_state, spread_pos.pos, spread_pos.face)
        else {
            return false;
        };
        // Vanilla `MultifaceSpreader.placeBlock` uses `Block.UPDATE_CLIENTS`.
        world.set_block_state(spread_pos.pos, new_state, UpdateFlags::UPDATE_CLIENTS)
    }

    /// Vanilla `SculkVeinSpreaderConfig.stateCanBeReplaced` followed by
    /// `MultifaceBlock.isValidStateForPlacement`.
    fn can_spread_into(
        world: &dyn LevelReader,
        source_pos: BlockPos,
        spread_pos: &MultifaceSpreadPos,
    ) -> bool {
        let existing_state = world.get_block_state(spread_pos.pos);
        Self::state_can_be_replaced(world, source_pos, spread_pos, existing_state)
            && Self::is_valid_state_for_placement(
                world,
                existing_state,
                spread_pos.pos,
                spread_pos.face,
            )
    }

    /// Vanilla `SculkVeinSpreaderConfig.stateCanBeReplaced`.
    fn state_can_be_replaced(
        world: &dyn LevelReader,
        source_pos: BlockPos,
        spread_pos: &MultifaceSpreadPos,
        existing_state: BlockStateId,
    ) -> bool {
        let against = world.get_block_state(spread_pos.pos.relative(spread_pos.face));
        if against.get_block() == &vanilla_blocks::SCULK
            || against.get_block() == &vanilla_blocks::SCULK_CATALYST
            || against.get_block() == &vanilla_blocks::MOVING_PISTON
        {
            return false;
        }

        let dx = source_pos.x() - spread_pos.pos.x();
        let dy = source_pos.y() - spread_pos.pos.y();
        let dz = source_pos.z() - spread_pos.pos.z();
        if dx.abs() + dy.abs() + dz.abs() == 2 {
            let neighbour_pos = source_pos.relative(spread_pos.face.opposite());
            let neighbour_state = world.get_block_state(neighbour_pos);
            if world.is_face_sturdy(neighbour_state, neighbour_pos, spread_pos.face) {
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

        existing_state.is_replaceable() || Self::default_state_can_be_replaced(existing_state)
    }

    /// Vanilla `MultifaceSpreader.DefaultSpreaderConfig.stateCanBeReplaced`.
    fn default_state_can_be_replaced(existing_state: BlockStateId) -> bool {
        existing_state.is_air()
            || existing_state.get_block() == &vanilla_blocks::SCULK_VEIN
            || (existing_state.get_block() == &vanilla_blocks::WATER
                && existing_state.get_fluid_state().is_source())
    }

    /// Vanilla `MultifaceBlock.isValidStateForPlacement`.
    fn is_valid_state_for_placement(
        world: &dyn LevelReader,
        old_state: BlockStateId,
        placement_pos: BlockPos,
        placement_direction: Direction,
    ) -> bool {
        if old_state.get_block() == &vanilla_blocks::SCULK_VEIN
            && MultifaceBlock::has_face(old_state, placement_direction)
        {
            return false;
        }

        MultifaceBlock::can_attach_to(world, placement_pos, placement_direction)
    }

    /// Vanilla `MultifaceBlock.getStateForPlacement` for the sculk vein spreader.
    fn state_for_placement(
        world: &dyn LevelReader,
        old_state: BlockStateId,
        placement_pos: BlockPos,
        placement_direction: Direction,
    ) -> Option<BlockStateId> {
        if !Self::is_valid_state_for_placement(world, old_state, placement_pos, placement_direction)
        {
            return None;
        }

        let mut new_state = if old_state.get_block() == &vanilla_blocks::SCULK_VEIN {
            old_state
        } else {
            let state = vanilla_blocks::SCULK_VEIN.default_state();
            let fluid_state = old_state.get_fluid_state();
            if fluid_state.is_source_of_type(&vanilla_fluids::WATER) {
                state.set_value(WATERLOGGED, true)
            } else {
                state
            }
        };
        new_state = new_state.set_value(multiface_face_property(placement_direction), true);
        Some(new_state)
    }

    /// Vanilla `SculkVeinSpreaderConfig.isOtherBlockValidAsSource`: every block except sculk
    /// veins may spread from any face.
    fn is_other_block_valid_as_source(state: BlockStateId) -> bool {
        state.get_block() != &vanilla_blocks::SCULK_VEIN
    }

    /// Vanilla `SpreadConfig.canSpreadFrom`.
    fn can_spread_from(state: BlockStateId, face: Direction) -> bool {
        Self::is_other_block_valid_as_source(state) || MultifaceBlock::has_face(state, face)
    }

    /// Vanilla `SculkVeinBlock.attemptUseCharge`.
    ///
    /// With vein spreading enabled, a sculk block placed into a replaceable neighbour costs one
    /// charge; otherwise the cursor loses half its charge on one `chargeDecayRate` draw in ten.
    pub fn attempt_use_charge(
        world: &Arc<World>,
        rng: &mut dyn Rng,
        spreader: &SculkSpreader,
        spread_veins: bool,
        cursor: &ChargeCursor,
    ) -> i32 {
        if spread_veins && Self::attempt_place_sculk(world, rng, spreader, cursor.pos()) {
            return cursor.charge() - 1;
        }
        if next_int_bounded(rng, spreader.charge_decay_rate()) == 0 {
            ((cursor.charge() as f32) * 0.5) as i32
        } else {
            cursor.charge()
        }
    }

    /// Vanilla `SculkVeinBlock.attemptPlaceSculk`.
    fn attempt_place_sculk(
        world: &Arc<World>,
        rng: &mut dyn Rng,
        spreader: &SculkSpreader,
        pos: BlockPos,
    ) -> bool {
        let state = world.get_block_state(pos);
        let mut directions = Direction::ALL;
        for direction in (1..directions.len()).rev() {
            let j = next_int_bounded(rng, direction as i32 + 1) as usize;
            directions.swap(direction, j);
        }

        for support in directions {
            if !MultifaceBlock::has_face(state, support) {
                continue;
            }

            let support_pos = pos.relative(support);
            let support_state = world.get_block_state(support_pos);
            if !support_state
                .get_block()
                .has_tag(spreader.replaceable_blocks())
            {
                continue;
            }

            let sculk = vanilla_blocks::SCULK.default_state();
            world.set_block(support_pos, sculk, UpdateFlags::UPDATE_ALL);
            world.play_sound(
                &sound_events::BLOCK_SCULK_SPREAD,
                SoundSource::Blocks,
                support_pos,
                1.0,
                1.0,
                None,
            );
            let _ = Self::spread_veins(world, sculk, support_pos, false);

            let skip = support.opposite();
            for direction in Direction::ALL {
                if direction == skip {
                    continue;
                }
                let vein_pos = support_pos.relative(direction);
                let possible_vein = world.get_block_state(vein_pos);
                if possible_vein.get_block() == &vanilla_blocks::SCULK_VEIN {
                    Self::on_discharged(world, possible_vein, vein_pos, rng);
                }
            }

            return true;
        }

        false
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
