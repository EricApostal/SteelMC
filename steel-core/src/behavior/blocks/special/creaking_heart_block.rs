//! `CreakingHeartBlock` behavior (`net.minecraft.world.level.block.CreakingHeartBlock`).
//!
//! The heart becomes dormant/awake only when both pale-oak logs along its axis match its own axis,
//! and it re-evaluates that on every neighbour shape update (scheduled one tick later). Ticking is
//! selected only while the state is not `uprooted`, exactly like vanilla's `getTicker`.
//!
//! Deviation: Steel has no environment-attribute/timeline foundation, so
//! `EnvironmentAttributes.CREAKING_ACTIVE` (vanilla `gameplay/creaking_active`, driven by the
//! overworld night timeline) cannot be read; a heart with valid logs becomes `dormant`, the
//! attribute's default value. Owner: world/environment-attribute lane.
//! Deviation: vanilla `CreakingHeartBlock.animateTick` plays the client-local
//! `CREAKING_HEART_IDLE` sound when surrounded by logs; Steel has no client block-animation hook.
//! Deviation: the linked `Creaking` lifecycle (`CreakingHeartBlockEntity.removeProtector`,
//! proximity hurt sounds and the comparator's distance signal) lives in the block entity, which
//! currently only persists the link; the block-level `playerWillDestroy`/`onExplosionHit` hooks
//! therefore award experience without tearing the protector down.

use std::ops::RangeInclusive;
use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, CreakingHeartState, Direction, EnumProperty,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::types::{GameType, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, axis::Axis};

use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext};
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::player::Player;
use crate::world::explosion::{ExplosionInfo, default_on_explosion_hit};
use crate::world::{LevelReader, ScheduledTickAccess, World};

const AXIS: &EnumProperty<Axis> = &BlockStateProperties::AXIS;
const STATE: &EnumProperty<CreakingHeartState> = &BlockStateProperties::CREAKING_HEART_STATE;
const NATURAL: &BoolProperty = &BlockStateProperties::NATURAL;

/// Vanilla `CreakingHeartBlock` experience range when a natural heart is broken.
const NATURAL_EXPERIENCE_RANGE: RangeInclusive<i32> = 20..=24;

/// `CreakingHeartBlock` behavior.
#[block_behavior]
pub struct CreakingHeartBlock {
    block: BlockRef,
}

impl CreakingHeartBlock {
    /// Creates a creaking-heart block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `CreakingHeartBlock.hasRequiredLogs`: both neighbors along `axis` are
    /// `#pale_oak_logs` with the same axis as this block.
    #[must_use]
    pub fn has_required_logs(state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        let axis = state.get_value(AXIS);
        let directions = match axis {
            Axis::X => [Direction::East, Direction::West],
            Axis::Y => [Direction::Up, Direction::Down],
            Axis::Z => [Direction::North, Direction::South],
        };
        directions.iter().all(|direction| {
            let neighbour = world.get_block_state(pos.relative(*direction));
            neighbour.get_block().has_tag(&BlockTag::PALE_OAK_LOGS)
                && neighbour.try_get_value(&BlockStateProperties::AXIS) == Some(axis)
        })
    }

    /// Vanilla `CreakingHeartBlock.updateState`: an `uprooted` heart with valid logs becomes
    /// `awake` when the environment says the creaking is active, otherwise `dormant`.
    ///
    /// Deviation: `CREAKING_ACTIVE` is unavailable (see the module docs), so the vanilla
    /// `false` branch (`dormant`) is always taken.
    fn update_state(state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> BlockStateId {
        if state.get_value(STATE) != CreakingHeartState::Uprooted
            || !Self::has_required_logs(state, world, pos)
        {
            return state;
        }
        state.set_value(STATE, CreakingHeartState::Dormant)
    }

    /// Vanilla `CreakingHeartBlock.tryAwardExperience`: natural hearts broken by a player who
    /// does not prevent block drops drop 20-24 experience.
    fn try_award_experience(
        player: &Player,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
    ) {
        if !player.prevents_block_drops()
            && player.game_mode() != GameType::Spectator
            && state.get_value(NATURAL)
        {
            world.pop_experience(pos, rand::random_range(NATURAL_EXPERIENCE_RANGE));
        }
    }
}

impl BlockBehavior for CreakingHeartBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        // Vanilla `CreakingHeartBlock.getStateForPlacement` runs `updateState` on the placed
        // state, so a heart placed between matching logs is immediately dormant/awake.
        let state = self
            .block
            .default_state()
            .set_value(AXIS, context.clicked_face().get_axis());
        Some(Self::update_state(
            state,
            context.world.as_ref(),
            context.place_pos(),
        ))
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        _direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        // Vanilla always re-schedules a one-tick re-evaluation on any shape update.
        world.schedule_block_tick_default(pos, self.block, 1);
        state
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let new_state = Self::update_state(state, world.as_ref(), pos);
        if new_state != state {
            world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
        }
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::CREAKING_HEART,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        // Vanilla returns a ticker only while the heart is not uprooted.
        if state.get_value(STATE) == CreakingHeartState::Uprooted {
            return None;
        }
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::CREAKING_HEART,
        )
    }

    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> BlockStateId {
        // Vanilla first asks the block entity to remove its protector; that lifecycle is not
        // implemented yet (see the module docs), so only the experience award remains.
        Self::try_award_experience(player, state, world, pos);
        state
    }

    fn on_explosion_hit(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        explosion: &ExplosionInfo<'_>,
        on_drop: &mut dyn FnMut(ItemStack, BlockPos),
    ) {
        // Vanilla `CreakingHeartBlock.onExplosionHit`: natural hearts destroyed by a
        // player-caused explosion still award experience.
        if explosion
            .block_interaction
            .should_affect_blocklike_entities()
            && let Some(indirect) = explosion.indirect_source
            && let Some(player) = indirect.as_ref().downcast_ref::<Player>()
        {
            Self::try_award_experience(player, state, world, pos);
        }
        default_on_explosion_hit(state, world, pos, explosion, on_drop);
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _moved_by_piston: bool,
    ) {
        // Vanilla `CreakingHeartBlock.affectNeighborsAfterRemoval` →
        // `Containers.updateNeighboursAfterDestroy`.
        world.update_neighbor_for_output_signal(pos, state.get_block());
    }

    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        world: &dyn LevelReader,
        pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        // Vanilla reads `CreakingHeartBlockEntity.getAnalogOutputSignal`, which is the scaled
        // distance to the linked creaking (15 down to 1) or 0 when no protector is linked.
        // Steel's block entity never links one yet (see the module docs), so 0 is exact here.
        if state.get_value(STATE) == CreakingHeartState::Uprooted {
            return 0;
        }
        let _ = (world, pos);
        0
    }
}
