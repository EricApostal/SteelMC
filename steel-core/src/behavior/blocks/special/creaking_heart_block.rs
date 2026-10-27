//! `CreakingHeartBlock` behavior (`net.minecraft.world.level.block.CreakingHeartBlock`).
//!
//! The heart becomes dormant/awake only when both pale-oak logs along its axis match its own axis,
//! and it re-evaluates that on every neighbour shape update (scheduled one tick later). Ticking is
//! selected only while the state is not `uprooted`, exactly like vanilla's `getTicker`.
//!
//! `EnvironmentAttributes.CREAKING_ACTIVE` (vanilla `gameplay/creaking_active`, driven by the
//! overworld night timeline) is sampled from the dimension's timelines via
//! [`crate::behavior::blocks::vegetation::bool_attribute`], the crate's public environment-attribute
//! bridge. Deviation: a public environment-attribute getter on `World`
//! (`world::environment`, owner: world/environment lane) is still missing; the bridge re-samples the
//! same `minecraft:gameplay/creaking_active` track the vanilla attribute system reads.
//! Deviation: vanilla `CreakingHeartBlock.animateTick` plays the client-local
//! `CREAKING_HEART_IDLE` sound when surrounded by logs; Steel has no client block-animation hook.
//! The linked `Creaking` lifecycle (`CreakingHeartBlockEntity.removeProtector`, the comparator's
//! distance signal and the hurt sounds) is wired through the block entity.

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

use crate::behavior::blocks::vegetation::bool_attribute;
use crate::behavior::{BlockBehavior, BlockEntityCreation, BlockPlaceContext};
use crate::block_entity::entities::CreakingHeartBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::player::Player;
use crate::world::explosion::{ExplosionInfo, default_on_explosion_hit};
use crate::world::{LevelReader, ScheduledTickAccess, World};

const AXIS: &EnumProperty<Axis> = &BlockStateProperties::AXIS;
const STATE: &EnumProperty<CreakingHeartState> = &BlockStateProperties::CREAKING_HEART_STATE;
const NATURAL: &BoolProperty = &BlockStateProperties::NATURAL;

/// Vanilla `EnvironmentAttributes.CREAKING_ACTIVE` key (`gameplay/creaking_active`).
pub const CREAKING_ACTIVE_ATTRIBUTE: &str = "minecraft:gameplay/creaking_active";

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

    /// Vanilla `EnvironmentAttributes.CREAKING_ACTIVE` sampled at `pos`.
    ///
    /// The attribute is timeline-driven (`minecraft:gameplay/creaking_active`, an `or`-combined
    /// boolean track on the overworld day timeline) and defaults to `false`, exactly like vanilla's
    /// `EnvironmentAttributeSystem.getValue(CREAKING_ACTIVE, pos)`.
    #[must_use]
    pub fn creaking_active(world: &World, _pos: BlockPos) -> bool {
        bool_attribute(world, CREAKING_ACTIVE_ATTRIBUTE, false)
    }

    /// Vanilla `CreakingHeartBlock.updateState`: an `uprooted` heart with valid logs becomes
    /// `awake` when the environment says the creaking is active, otherwise `dormant`.
    fn update_state(state: BlockStateId, world: &World, pos: BlockPos) -> BlockStateId {
        if state.get_value(STATE) != CreakingHeartState::Uprooted
            || !Self::has_required_logs(state, world, pos)
        {
            return state;
        }
        let heart_state = if Self::creaking_active(world, pos) {
            CreakingHeartState::Awake
        } else {
            CreakingHeartState::Dormant
        };
        state.set_value(STATE, heart_state)
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
        // Vanilla `CreakingHeartBlock.playerWillDestroy` first asks the block entity to remove its
        // protector (with a player-attack damage source), then awards experience.
        if let Some(block_entity) = world.get_block_entity(pos)
            && let Some(heart) = block_entity.downcast_ref::<CreakingHeartBlockEntity>()
        {
            heart.remove_protector(world);
        }
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
        // Vanilla `CreakingHeartBlock.getAnalogOutputSignal`: the block entity's cached signal, the
        // scaled distance to the linked creaking (15 down to 1) or 0 when no protector is linked.
        if state.get_value(STATE) == CreakingHeartState::Uprooted {
            return 0;
        }
        world.get_block_entity(pos).map_or(0, |block_entity| {
            block_entity
                .downcast_ref::<CreakingHeartBlockEntity>()
                .map_or(0, CreakingHeartBlockEntity::analog_output_signal)
        })
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_world_clocks};
    use steel_utils::ChunkPos;

    use crate::behavior::init_behaviors;
    use crate::block_entity::init_block_entities;
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};

    use super::*;

    const HEART_POS: BlockPos = BlockPos::new(4, 64, 4);

    fn uprooted_heart_state() -> BlockStateId {
        vanilla_blocks::CREAKING_HEART
            .default_state()
            .set_value(AXIS, Axis::Y)
            .set_value(STATE, CreakingHeartState::Uprooted)
    }

    /// Places a heart flanked by matching-axis pale-oak logs in a real world.
    fn heart_world() -> (TestWorld, Arc<World>) {
        init_vanilla_registry();
        init_behaviors();
        init_block_entities();
        let world_fixture = fresh_test_world("creaking_heart_attribute");
        let world = Arc::clone(&world_fixture.world);
        insert_ready_full_chunk(&world, ChunkPos::from_block_pos(HEART_POS));
        let log = vanilla_blocks::PALE_OAK_LOG.default_state();
        assert!(world.set_block(HEART_POS.above(), log, UpdateFlags::UPDATE_ALL));
        assert!(world.set_block(HEART_POS.below(), log, UpdateFlags::UPDATE_ALL));
        (world_fixture, world)
    }

    /// Vanilla `CreakingHeartBlock.updateState` reads `EnvironmentAttributes.CREAKING_ACTIVE`: an
    /// uprooted heart with valid logs becomes `awake` while the daytime timeline says the creaking
    /// is active and `dormant` otherwise. Before the fix the `awake` branch was unreachable.
    #[test]
    fn update_state_follows_the_creaking_active_attribute() {
        let (_fixture, world) = heart_world();
        let behavior = CreakingHeartBlock::new(&vanilla_blocks::CREAKING_HEART);
        let state = uprooted_heart_state();

        // Overworld day timeline keyframe 12600 enables `minecraft:gameplay/creaking_active`.
        assert!(world.set_block(HEART_POS, state, UpdateFlags::UPDATE_ALL));
        assert_eq!(
            world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, 18_000),
            Some(())
        );
        behavior.tick(state, &world, HEART_POS);
        assert_eq!(
            world.get_block_state(HEART_POS).get_value(STATE),
            CreakingHeartState::Awake,
            "the heart must awake while the creaking is active"
        );

        // Daytime disables the attribute (the next keyframe flips it off at 23401).
        assert!(world.set_block(HEART_POS, state, UpdateFlags::UPDATE_ALL));
        assert_eq!(
            world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, 1_000),
            Some(())
        );
        behavior.tick(state, &world, HEART_POS);
        assert_eq!(
            world.get_block_state(HEART_POS).get_value(STATE),
            CreakingHeartState::Dormant,
            "the heart must stay dormant while the creaking is inactive"
        );
    }
}
