//! Cushion item behavior (`CushionItem`).
//!
//! Vanilla reference: `CushionItem.useOn`
//! (`/tmp/vanilla/src/world/item/CushionItem.java:28-75`):
//!
//! 1. `recalculateContextForSpecialCollisionShapes(context)`: when the clicked
//!    block is in `BlockTags.CUSHION_USES_COLLISION_SHAPE`, the hit result is
//!    re-clipped against that block's *collision* shape (0.001 block beyond the
//!    original hit) so cushions stack on blocks with non-full collision boxes;
//! 2. only the `UP` face is accepted (`FAIL` otherwise);
//! 3. `BlockPlaceContext` gives the placement block; the cushion spawns at
//!    `Vec3.atCenterOfWithY(blockPos, clickLocation.y)` — block center in X/Z,
//!    the *click* height in Y;
//! 4. `Cushion.canBePlacedAt(level, spawnAABB)` (anchor below, not buried, not
//!    covered by suffocating blocks) and no existing cushion inside the spawn
//!    box are required (`FAIL` otherwise);
//! 5. the cushion is created with `EntitySpawnReason.SPAWN_ITEM_USE`, snapped to
//!    the spawn position with `Direction.fromYRot(placeContext.getRotation()).toYRot()`
//!    yaw and pitch `0`, added, `destroyIfInFire`, `CUSHION_PLACE`
//!    (`Blocks` source, volume `0.75`, pitch `0.8`), `GameEvent.ENTITY_PLACE`
//!    from the cushion, and `stack.consume(1, player)`;
//! 6. `SUCCESS`.
//!
//! Steel has no `Cushion` entity, no `CUSHION` factory in
//! `steel-core/src/entity/generated/entities.rs`, and no
//! `Cushion::canBePlacedAt` / `Cushion::destroyIfInFire` helper, so steps 4-5
//! can only reach the generated factory, which reports
//! `EntitySpawnError::MissingFactory`; the item then returns `FAIL` like vanilla
//! when `EntityType.create` returns `null`.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::blocks::shapes::OffsetVoxelShape;
use steel_registry::sound_events;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_events;
use steel_utils::WorldAabb;

use crate::behavior::context::{BlockHitResult, BlockPlaceContext, PlacementSource, UseOnContext};
use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext, InteractionResult, ItemBehavior};
use crate::entity::{
    Entity, add_spawned_entity, apply_implicit_item_stack_components, create_entity_instance,
};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `CushionItem.COLLISION_SHAPE_RAYCAST_EPSILON`.
const COLLISION_SHAPE_RAYCAST_EPSILON: f64 = 0.001;

/// Behavior for the sixteen `*_cushion` items.
#[item_behavior]
pub struct CushionItem;

impl CushionItem {
    /// Vanilla `recalculateContextForSpecialCollisionShapes`.
    ///
    /// Returns the effective hit result: when the clicked block is in
    /// `#minecraft:cushion_uses_collision_shape`, the click ray is extended by
    /// [`COLLISION_SHAPE_RAYCAST_EPSILON`] and clipped against that block's
    /// collision shape; the closest collision hit replaces the block-outline
    /// hit result.
    fn recalculate_context_for_special_collision_shapes(
        context: &UseOnContext<'_>,
    ) -> BlockHitResult {
        let world = context.world;
        let clicked_pos = context.hit_result.block_pos;
        let clicked_state = world.get_block_state(clicked_pos);

        if !clicked_state
            .get_block()
            .has_tag(&BlockTag::CUSHION_USES_COLLISION_SHAPE)
        {
            return context.hit_result.clone();
        }

        let player = context.player;
        let ray_from = DVec3::new(player.position().x, player.get_eye_y(), player.position().z);
        let click_location = context.hit_result.location;
        let ray = click_location - ray_from;
        let Some(ray) = ray.try_normalize() else {
            return context.hit_result.clone();
        };
        let ray_to = click_location + ray * COLLISION_SHAPE_RAYCAST_EPSILON;

        let behavior = BLOCK_BEHAVIORS.get_behavior(clicked_state.get_block());
        let collision_context = BlockCollisionContext::empty();
        let shape = OffsetVoxelShape::new(
            behavior.get_collision_shape(
                clicked_state,
                world.as_ref(),
                clicked_pos,
                collision_context,
            ),
            behavior.get_collision_shape_offset(
                clicked_state,
                world.as_ref(),
                clicked_pos,
                collision_context,
            ),
        );

        match World::clip_shape(clicked_pos, ray_from, ray_to, shape) {
            Some(hit) => BlockHitResult {
                location: hit.location,
                direction: hit.direction,
                block_pos: hit.block_pos,
                miss: hit.miss,
                inside: hit.inside,
                world_border_hit: hit.world_border_hit,
            },
            None => context.hit_result.clone(),
        }
    }

    /// Vanilla `EntityType.getSpawnAABB` for the cushion footprint.
    fn spawn_box(entity_pos: DVec3) -> WorldAabb {
        let dimensions = &vanilla_entities::CUSHION.dimensions;
        WorldAabb::entity_box(
            entity_pos.x,
            entity_pos.y,
            entity_pos.z,
            f64::from(dimensions.half_width()),
            f64::from(dimensions.height),
        )
    }
}

impl ItemBehavior for CushionItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let recalculated = Self::recalculate_context_for_special_collision_shapes(context);
        let world = context.world;

        if recalculated.direction != Direction::Up {
            return InteractionResult::Fail;
        }

        let place_context = BlockPlaceContext::new(
            world,
            PlacementSource::player_hand(context.player, &context.inv),
            &recalculated,
        );
        let block_pos = place_context.place_pos();
        let entity_pos = DVec3::new(
            f64::from(block_pos.x()) + 0.5,
            recalculated.location.y,
            f64::from(block_pos.z()) + 0.5,
        );
        let spawn_box = Self::spawn_box(entity_pos);

        // Vanilla `Cushion.canBePlacedAt(level, spawnAABB)` (anchor below, not
        // buried in the resting slice, not covered by suffocating blocks) has no
        // Steel equivalent yet; see the module documentation.
        if world.has_entity_in_aabb_matching(&spawn_box, |entity| {
            entity.entity_type() == &vanilla_entities::CUSHION
        }) {
            return InteractionResult::Fail;
        }

        let entity = match create_entity_instance(world, &vanilla_entities::CUSHION, entity_pos) {
            Ok(entity) => entity,
            Err(error) => {
                log::warn!("Failed to create cushion at {block_pos:?}: {error:?}");
                return InteractionResult::Fail;
            }
        };

        let stack = context.inv.with_item(|item| item.clone());
        apply_implicit_item_stack_components(&entity, &stack);

        let yaw = Direction::from_yaw(place_context.rotation()).to_yaw();
        entity.set_rotation((yaw, 0.0));
        entity.set_old_position_to_current();
        entity.base().set_old_rotation_to_current();

        if let Err(error) = add_spawned_entity(world, Arc::clone(&entity)) {
            log::warn!("Failed to add cushion to the world: {error:?}");
            return InteractionResult::Fail;
        }

        // Vanilla `Cushion.destroyIfInFire` has no Steel equivalent yet; the
        // cushion entity lane owns it (see the module documentation).
        let position = entity.position();
        world.play_sound_at(
            &sound_events::ENTITY_CUSHION_PLACE,
            SoundSource::Blocks,
            position,
            0.75,
            0.8,
            None,
        );
        world.game_event_at(
            &vanilla_game_events::ENTITY_PLACE,
            position,
            &GameEventContext::new(Some(&**context.player), None),
        );

        let infinite = context.player.has_infinite_materials();
        context.inv.with_item(|item| item.consume_one(infinite));
        InteractionResult::Success
    }
}
