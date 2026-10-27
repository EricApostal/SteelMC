//! Vanilla dismount location helper

use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::{
    block_state_ext::BlockStateExt,
    properties::BlockStateProperties,
    shapes::{OffsetVoxelShape, VoxelShape},
};
use steel_registry::entity_data::EntityPose;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{vanilla_blocks, vanilla_entities};
use steel_utils::{BlockPos, BlockStateId, WorldAabb, axis::Axis};

use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext};
use crate::entity::Entity;
use crate::entity::ai::walk::WalkPathEvaluator;
use crate::physics::{CollisionWorld, WorldCollisionProvider};
use crate::world::World;

/// Vanilla `DismountHelper.offsetsForDirection`: the eight candidate columns a
/// passenger can step out into, in vanilla's search order.
#[must_use]
pub(crate) const fn offsets_for_direction(forward: steel_utils::Direction) -> [(i32, i32); 8] {
    let right = forward.rotate_y_clockwise();
    let left = right.opposite();
    let back = forward.opposite();
    let (right_x, right_z) = right.offset_xz();
    let (left_x, left_z) = left.offset_xz();
    let (back_x, back_z) = back.offset_xz();
    let (forward_x, forward_z) = forward.offset_xz();

    [
        (right_x, right_z),
        (left_x, left_z),
        (back_x + right_x, back_z + right_z),
        (back_x + left_x, back_z + left_z),
        (forward_x + right_x, forward_z + right_z),
        (forward_x + left_x, forward_z + left_z),
        (back_x, back_z),
        (forward_x, forward_z),
    ]
}

#[must_use]
pub(crate) fn can_dismount_to(world: &Arc<World>, aabb: &WorldAabb) -> bool {
    let collision_world = WorldCollisionProvider::new(world);
    !collision_world.has_block_collision_with_context(aabb, BlockCollisionContext::empty())
        && world.world_border_snapshot().is_within_bounds(*aabb)
}

/// Vanilla `DismountHelper.canDismountTo(CollisionGetter, Vec3, LivingEntity, Pose)`.
#[must_use]
pub(crate) fn can_dismount_to_pose(
    world: &Arc<World>,
    location: DVec3,
    passenger: &dyn Entity,
    dismount_pose: EntityPose,
) -> bool {
    let dimensions = passenger.dimensions_for_pose(dismount_pose);
    let aabb = WorldAabb::entity_box(
        location.x,
        location.y,
        location.z,
        f64::from(dimensions.half_width()),
        f64::from(dimensions.height),
    );
    can_dismount_to(world, &aabb)
}

/// Vanilla `LivingEntity.dismountVehicle`: places `passenger` where `vehicle` released it.
///
/// Vanilla reaches this from `LivingEntity.stopRiding`, so the ride relationship is
/// already torn down and only the position is left to fix up.
pub(crate) fn dismount_vehicle(passenger: &dyn Entity, vehicle: &dyn Entity) {
    let Some(world) = passenger.level() else {
        return;
    };

    let location = if passenger.is_removed() {
        passenger.position()
    } else if !vehicle.is_removed()
        && !world
            .get_block_state(vehicle.block_position())
            .get_block()
            .has_tag(&BlockTag::PORTALS)
    {
        vehicle.get_dismount_location_for_passenger(passenger)
    } else {
        free_dismount_location(&world, passenger, vehicle)
    };

    passenger.dismount_to(location);
}

/// Vanilla `LivingEntity.dismountVehicle`'s fallback used when the vehicle is gone or
/// inside a portal: the passenger's own column, nudged to the closest free spot.
fn free_dismount_location(
    world: &Arc<World>,
    passenger: &dyn Entity,
    vehicle: &dyn Entity,
) -> DVec3 {
    let position = passenger.position();
    let base = DVec3::new(position.x, position.y.max(vehicle.position().y), position.z);
    let dimensions = passenger.dimensions_for_pose(passenger.pose());
    if dimensions.width > 4.0 || dimensions.height > 4.0 {
        return base;
    }

    let width = f64::from(dimensions.width);
    let height = f64::from(dimensions.height);
    let half_height = height / 2.0;
    let center = base + DVec3::new(0.0, half_height, 0.0);
    let allowed_centers = [WorldAabb::of_size(center, width, height, width)];
    let provider = WorldCollisionProvider::for_entity(world, passenger);
    provider
        .find_free_position(&allowed_centers, center, width, height, width)
        .map_or(base, |free| free + DVec3::new(0.0, -half_height, 0.0))
}

/// Vanilla `DismountHelper.findCeilingFrom`.
#[must_use]
#[expect(
    dead_code,
    reason = "vanilla DismountHelper foundation; the AbstractMinecart dismount override uses this next"
)]
pub(crate) fn find_ceiling_from(
    pos: BlockPos,
    blocks: i32,
    mut shape_getter: impl FnMut(BlockPos) -> OffsetVoxelShape,
) -> f64 {
    let mut y = 0;
    while y < blocks {
        let cursor = BlockPos::new(pos.x(), pos.y() + y, pos.z());
        let collision_shape = shape_getter(cursor);
        if !collision_shape.is_empty() {
            return f64::from(pos.y() + y) + collision_shape.min(Axis::Y);
        }
        y += 1;
    }

    f64::INFINITY
}

#[must_use]
pub(crate) fn find_safe_dismount_location(
    world: &Arc<World>,
    entity: &dyn Entity,
    block_pos: BlockPos,
    check_dangerous: bool,
) -> Option<DVec3> {
    if check_dangerous && is_block_dangerous(entity, world.get_block_state(block_pos)) {
        return None;
    }

    let floor_height = block_floor_height(
        non_climbable_shape(world, block_pos),
        non_climbable_shape(world, block_pos.below()),
    );
    if !is_block_floor_valid(floor_height) {
        return None;
    }

    if check_dangerous
        && floor_height <= 0.0
        && is_block_dangerous(entity, world.get_block_state(block_pos.below()))
    {
        return None;
    }

    let position = DVec3::new(
        f64::from(block_pos.x()) + 0.5,
        f64::from(block_pos.y()) + floor_height,
        f64::from(block_pos.z()) + 0.5,
    );
    let dimensions = entity.dimensions_for_pose(EntityPose::Standing);
    let aabb = WorldAabb::entity_box(
        position.x,
        position.y,
        position.z,
        f64::from(dimensions.half_width()),
        f64::from(dimensions.height),
    );
    if !can_dismount_to(world, &aabb) {
        return None;
    }

    if entity.entity_type() == &vanilla_entities::PLAYER
        && (world
            .get_block_state(block_pos)
            .get_block()
            .has_tag(&BlockTag::INVALID_SPAWN_INSIDE)
            || world
                .get_block_state(block_pos.above())
                .get_block()
                .has_tag(&BlockTag::INVALID_SPAWN_INSIDE))
    {
        return None;
    }

    Some(position)
}

pub(crate) fn non_climbable_shape(world: &Arc<World>, pos: BlockPos) -> OffsetVoxelShape {
    let state = world.get_block_state(pos);
    let block = state.get_block();
    let is_open_trapdoor = block.has_tag(&BlockTag::TRAPDOORS)
        && state.try_get_value(&BlockStateProperties::OPEN) == Some(true);

    if block.has_tag(&BlockTag::CLIMBABLE) || is_open_trapdoor {
        return OffsetVoxelShape::without_offset(VoxelShape::EMPTY);
    }

    block_collision_shape(world, pos)
}

/// Vanilla `BlockGetter.getBlockFloorHeight(BlockPos)`: the height a passenger
/// standing in `pos` rests at, or negative infinity when there is no floor.
#[must_use]
pub(crate) fn get_block_floor_height(world: &Arc<World>, pos: BlockPos) -> f64 {
    block_floor_height(
        block_collision_shape(world, pos),
        block_collision_shape(world, pos.below()),
    )
}

/// Vanilla `BlockState.getCollisionShape`: the plain collision shape of `pos`.
fn block_collision_shape(world: &Arc<World>, pos: BlockPos) -> OffsetVoxelShape {
    let state = world.get_block_state(pos);
    let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
    let shape =
        behavior.get_collision_shape(state, world.as_ref(), pos, BlockCollisionContext::empty());
    if shape.is_empty() {
        return OffsetVoxelShape::without_offset(shape);
    }

    OffsetVoxelShape::new(
        shape,
        behavior.get_collision_shape_offset(
            state,
            world.as_ref(),
            pos,
            BlockCollisionContext::empty(),
        ),
    )
}

fn block_floor_height(block_shape: OffsetVoxelShape, below_block_shape: OffsetVoxelShape) -> f64 {
    if !block_shape.is_empty() {
        return block_shape.max(Axis::Y);
    }

    let below_floor = below_block_shape.max(Axis::Y);
    if below_floor >= 1.0 {
        below_floor - 1.0
    } else {
        f64::NEG_INFINITY
    }
}

/// Vanilla `DismountHelper.isBlockFloorValid`.
#[must_use]
pub(crate) fn is_block_floor_valid(block_floor_height: f64) -> bool {
    !block_floor_height.is_infinite() && block_floor_height < 1.0
}

fn is_block_dangerous(entity: &dyn Entity, state: BlockStateId) -> bool {
    // TODO: mirror vanilla entitytype.immuneto when the entity types carry entity specific immune block tag
    if !entity.fire_immune() && WalkPathEvaluator::is_burning_block(state) {
        return true;
    }

    let block = state.get_block();
    block == &vanilla_blocks::WITHER_ROSE
        || block == &vanilla_blocks::SWEET_BERRY_BUSH
        || block == &vanilla_blocks::CACTUS
        || block == &vanilla_blocks::POWDER_SNOW
}
