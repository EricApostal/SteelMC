//! `FrogspawnBlock` behavior (`net.minecraft.world.level.block.FrogspawnBlock`).
//!
//! Vanilla places frogspawn on `#supports_frogspawn` blocks under a non-empty fluid, schedules a
//! hatch tick between 3 600 and 12 000 ticks, hatches 2-5 tadpoles, and is destroyed by falling
//! blocks passing through it.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::sound_events;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, BlockStateId, Direction};

use crate::behavior::{BlockBehavior, BlockPlaceContext};
use crate::entity::{Entity, Mob, add_spawned_entity, create_entity_instance};
use crate::world::{LevelReader, ScheduledTickAccess, World};

/// Vanilla `FrogspawnBlock.MIN_TADPOLES_SPAWN`.
pub const MIN_TADPOLES_SPAWN: i32 = 2;
/// Vanilla `FrogspawnBlock.MAX_TADPOLES_SPAWN`.
pub const MAX_TADPOLES_SPAWN: i32 = 5;
/// Vanilla `FrogspawnBlock.DEFAULT_MIN_HATCH_TICK_DELAY`.
pub const DEFAULT_MIN_HATCH_TICK_DELAY: i32 = 3_600;
/// Vanilla `FrogspawnBlock.DEFAULT_MAX_HATCH_TICK_DELAY`.
pub const DEFAULT_MAX_HATCH_TICK_DELAY: i32 = 12_000;

/// `FrogspawnBlock` behavior.
#[block_behavior]
pub struct FrogspawnBlock {
    block: BlockRef,
}

impl FrogspawnBlock {
    /// Creates a frogspawn block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `FrogspawnBlock.getFrogspawnHatchDelay`.
    #[must_use]
    pub fn hatch_delay() -> i32 {
        rand::random_range(DEFAULT_MIN_HATCH_TICK_DELAY..DEFAULT_MAX_HATCH_TICK_DELAY)
    }

    /// Vanilla `FrogspawnBlock.mayPlaceOn`: the block below supports frogspawn (block tag or
    /// fluid tag) and has no fluid above it.
    fn may_place_on(world: &dyn LevelReader, pos: BlockPos) -> bool {
        let below = world.get_block_state(pos);
        let fluid_state = below.get_fluid_state();
        let supports = below.get_block().has_tag(&BlockTag::SUPPORTS_FROGSPAWN)
            || fluid_state
                .fluid_id
                .has_tag(&steel_registry::vanilla_fluid_tags::FluidTag::SUPPORTS_FROGSPAWN);
        supports
            && world
                .get_block_state(pos.above())
                .get_fluid_state()
                .is_empty()
    }

    /// Vanilla `FrogspawnBlock.getRandomTadpolePositionOffset`: a horizontal offset inside the
    /// tadpole hitbox (`Mth.clamp(random.nextDouble(), 0.2, 0.7999999970197678)`).
    fn random_tadpole_position_offset() -> f64 {
        rand::random::<f64>().clamp(0.2, 0.799_999_997_019_767_8)
    }

    /// Vanilla `FrogspawnBlock.spawnTadpoles`: `MIN_TADPOLES_SPAWN..=MAX_TADPOLES_SPAWN`
    /// persistent tadpoles around the block's lower half with a random yaw.
    fn spawn_tadpoles(world: &Arc<World>, pos: BlockPos) {
        let amount = rand::random_range(MIN_TADPOLES_SPAWN..=MAX_TADPOLES_SPAWN);
        for _ in 1..=amount {
            let spawn_at = DVec3::new(
                f64::from(pos.x()) + Self::random_tadpole_position_offset(),
                f64::from(pos.y()) - 0.5,
                f64::from(pos.z()) + Self::random_tadpole_position_offset(),
            );
            let Ok(tadpole) = create_entity_instance(world, &vanilla_entities::TADPOLE, spawn_at)
            else {
                continue;
            };
            tadpole.snap_to(spawn_at, rand::random_range(1..=360) as f32, 0.0);
            if let Some(mob) = tadpole.as_mob() {
                mob.set_persistence_required();
            }
            let _ = add_spawned_entity(world, tadpole);
        }
    }
}

impl BlockBehavior for FrogspawnBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    fn can_survive(&self, _state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        Self::may_place_on(world, pos.below())
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
        if self.can_survive(state, world, pos) {
            state
        } else {
            vanilla_blocks::AIR.default_state()
        }
    }

    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if old_state.get_block() == self.block {
            return;
        }
        world.schedule_block_tick_default(pos, self.block, Self::hatch_delay());
    }

    fn tick(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        if !self.can_survive(state, world.as_ref(), pos) {
            world.destroy_block(pos, false);
            return;
        }

        // Vanilla `FrogspawnBlock.hatchFrogspawn`.
        world.destroy_block(pos, false);
        world.play_block_sound(&sound_events::BLOCK_FROGSPAWN_HATCH, pos, 1.0, 1.0, None);
        Self::spawn_tadpoles(world, pos);
    }

    fn entity_inside(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        entity: &dyn Entity,
        _effect_collector: &mut crate::entity::InsideBlockEffectCollector,
        _is_precise: bool,
    ) {
        if entity.entity_type() == &vanilla_entities::FALLING_BLOCK {
            world.destroy_block(pos, false);
        }
    }
}
