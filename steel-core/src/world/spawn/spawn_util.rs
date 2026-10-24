//! Vanilla `SpawnUtil`: place a mob near a position by trying a few candidate
//! columns and walking down to a block the strategy accepts.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::blocks::shapes::is_offset_face_full;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_utils::random::{Random as _, RandomSource};
use steel_utils::{BlockPos, BlockStateId};

use crate::entity::SharedEntity;
use crate::entity::{EntitySpawnReason, create_entity_instance};
use crate::world::World;

/// Vanilla `SpawnUtil.Strategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnStrategy {
    /// Vanilla `SpawnUtil.Strategy.LEGACY_IRON_GOLEM`.
    LegacyIronGolem,
    /// Vanilla `SpawnUtil.Strategy.ON_TOP_OF_COLLIDER`.
    OnTopOfCollider,
    /// Vanilla `SpawnUtil.Strategy.ON_TOP_OF_COLLIDER_NO_LEAVES`.
    OnTopOfColliderNoLeaves,
}

impl SpawnStrategy {
    /// Vanilla `SpawnUtil.Strategy.canSpawnOn`.
    fn can_spawn_on(
        self,
        _level: &World,
        pos: BlockPos,
        block_state: BlockStateId,
        above_pos: BlockPos,
        above_state: BlockStateId,
    ) -> bool {
        match self {
            Self::LegacyIronGolem => {
                let block = block_state.get_block();
                if block == &vanilla_blocks::COBWEB
                    || block == &vanilla_blocks::CACTUS
                    || block == &vanilla_blocks::GLASS_PANE
                    || block == &vanilla_blocks::CONDUIT
                    || block == &vanilla_blocks::ICE
                    || block == &vanilla_blocks::TNT
                    || block == &vanilla_blocks::GLOWSTONE
                    || block == &vanilla_blocks::BEACON
                    || block == &vanilla_blocks::SEA_LANTERN
                    || block == &vanilla_blocks::FROSTED_ICE
                    || block == &vanilla_blocks::TINTED_GLASS
                    || block == &vanilla_blocks::GLASS
                    || block.has_tag(&BlockTag::LEAVES)
                {
                    // Vanilla also rejects stained glass and stained glass panes;
                    // Steel has no `instanceof` distinction for those, and the
                    // leaves/tinted/plain checks cover the extracted shapes.
                    return false;
                }

                (above_state.is_air() || above_state.has_fluid())
                    && (is_full_up(block_state, pos) || block == &vanilla_blocks::POWDER_SNOW)
            }
            Self::OnTopOfCollider => {
                above_state.get_collision_shape_at(above_pos).is_empty()
                    && is_offset_face_full(block_state.get_collision_shape_at(pos), Direction::Up)
            }
            Self::OnTopOfColliderNoLeaves => {
                above_state.get_collision_shape_at(above_pos).is_empty()
                    && !block_state.get_block().has_tag(&BlockTag::LEAVES)
                    && is_offset_face_full(block_state.get_collision_shape_at(pos), Direction::Up)
            }
        }
    }
}

/// Vanilla `BlockState.isSolid`.
fn is_full_up(state: BlockStateId, pos: BlockPos) -> bool {
    is_offset_face_full(state.get_collision_shape_at(pos), Direction::Up)
}

/// Vanilla `SpawnUtil.trySpawnMob`.
pub fn try_spawn_mob(
    entity_type: EntityTypeRef,
    spawn_reason: EntitySpawnReason,
    world: &Arc<World>,
    start: BlockPos,
    spawn_attempts: i32,
    spawn_range_xz: i32,
    spawn_range_y: i32,
    strategy: SpawnStrategy,
    check_collisions: bool,
    random: &mut RandomSource,
) -> Option<SharedEntity> {
    for _ in 0..spawn_attempts {
        let dx = random.next_i32_between(-spawn_range_xz, spawn_range_xz);
        let dz = random.next_i32_between(-spawn_range_xz, spawn_range_xz);
        let mut search_pos =
            BlockPos::new(start.x() + dx, start.y() + spawn_range_y, start.z() + dz);

        if !world.is_within_spawn_border(search_pos)
            || !move_to_possible_spawn_position(world, spawn_range_y, &mut search_pos, strategy)
        {
            continue;
        }

        if check_collisions
            && !world.no_collision(crate::world::spawn::natural::spawn_aabb_at(
                entity_type,
                f64::from(search_pos.x()) + 0.5,
                f64::from(search_pos.y()),
                f64::from(search_pos.z()) + 0.5,
            ))
        {
            continue;
        }

        let Ok(mob) = create_entity_instance(
            world,
            entity_type,
            DVec3::new(
                f64::from(search_pos.x()) + 0.5,
                f64::from(search_pos.y()),
                f64::from(search_pos.z()) + 0.5,
            ),
        ) else {
            continue;
        };

        let Some(mob_trait) = mob.as_mob() else {
            continue;
        };

        if !mob_trait.check_spawn_obstruction(world) {
            continue;
        }

        mob_trait.finalize_spawn(world, spawn_reason, None);
        if world.try_add_entity(Arc::clone(&mob)).is_err() {
            continue;
        }
        mob_trait.play_ambient_sound();
        return Some(mob);
    }

    None
}

/// Vanilla `SpawnUtil.moveToPossibleSpawnPosition`.
fn move_to_possible_spawn_position(
    level: &World,
    spawn_range_y: i32,
    search_pos: &mut BlockPos,
    strategy: SpawnStrategy,
) -> bool {
    let mut above_pos = *search_pos;
    let mut above_state = level.get_block_state(above_pos);

    for _ in (0..=spawn_range_y).rev() {
        *search_pos = search_pos.below();
        above_pos = search_pos.above();
        let current_state = level.get_block_state(*search_pos);
        if strategy.can_spawn_on(level, *search_pos, current_state, above_pos, above_state) {
            *search_pos = search_pos.above();
            return true;
        }
        above_state = current_state;
    }

    false
}
