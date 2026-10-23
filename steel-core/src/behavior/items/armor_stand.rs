//! Armor stand item behavior (`ArmorStandItem`).
//!
//! Vanilla reference: `ArmorStandItem.useOn`
//! (`/tmp/vanilla/src/world/item/ArmorStandItem.java:24-60`):
//!
//! 1. `clickedFace == Direction.DOWN` → `FAIL` (armor stands need a floor);
//! 2. `BlockPlaceContext placeContext = new BlockPlaceContext(context)`, so the
//!    spawn block is `placeContext.getClickedPos()` (the replaceable clicked
//!    block, otherwise the block above the clicked face);
//! 3. `pos = Vec3.atBottomCenterOf(blockPos)` and
//!    `box = EntityTypes.ARMOR_STAND.getDimensions().makeBoundingBox(pos)` —
//!    the stand's own 0.5×1.975 footprint anchored at its feet;
//! 4. the placement is only legal when `level.noCollision(null, box)` and
//!    `level.getEntities(null, box).isEmpty()`;
//! 5. server-side: create the stand with `EntitySpawnReason.SPAWN_ITEM_USE`,
//!    snap it to `pos` with
//!    `yRot = Mth.floor((Mth.wrapDegrees(context.getRotation() - 180.0F) + 22.5F) / 45.0F) * 45.0F`
//!    and pitch `0`, add it with passengers, play `ARMOR_STAND_PLACE`
//!    (`Blocks` source, volume `0.75`, pitch `0.8`) at the stand, and emit
//!    `GameEvent.ENTITY_PLACE` from the stand with the player as source;
//! 6. `stack.shrink(1)` and `SUCCESS`.
//!
//! The stand is created through the generated entity factory
//! ([`create_entity_instance`]), so the item only works once
//! `steel-core/src/entity/generated/entities.rs` registers
//! `vanilla_entities::ARMOR_STAND`. Until then creation reports
//! `EntitySpawnError::MissingFactory` and the item returns `FAIL`, exactly like
//! vanilla when `EntityType.create` returns `null`.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_math::wrap_degrees;
use steel_protocol::packets::game::SoundSource;
use steel_registry::sound_events;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_events;
use steel_utils::{BlockPos, Direction, WorldAabb};

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseOnContext};
use crate::entity::{
    add_spawned_entity, apply_implicit_item_stack_components, create_entity_instance,
};
use crate::world::game_event::GameEventContext;

/// Behavior for the `armor_stand` item.
#[item_behavior]
pub struct ArmorStandItem;

impl ArmorStandItem {
    /// Vanilla `EntityDimensions.makeBoundingBox` for the armor stand footprint.
    fn spawn_box(pos: DVec3) -> WorldAabb {
        let dimensions = &vanilla_entities::ARMOR_STAND.dimensions;
        WorldAabb::entity_box(
            pos.x,
            pos.y,
            pos.z,
            f64::from(dimensions.half_width()),
            f64::from(dimensions.height),
        )
    }
}

impl ItemBehavior for ArmorStandItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        if context.hit_result.direction == Direction::Down {
            return InteractionResult::Fail;
        }

        let place_context = context.build_place_context();
        let block_pos = place_context.place_pos();
        let pos = bottom_center(block_pos);
        let spawn_box = Self::spawn_box(pos);

        if !context.world.no_collision(spawn_box)
            || !context.world.get_entities_in_aabb(&spawn_box).is_empty()
        {
            return InteractionResult::Fail;
        }

        let entity =
            match create_entity_instance(context.world, &vanilla_entities::ARMOR_STAND, pos) {
                Ok(entity) => entity,
                Err(error) => {
                    log::warn!("Failed to create armor stand at {block_pos:?}: {error:?}");
                    return InteractionResult::Fail;
                }
            };

        let stack = context.inv.with_item(|item| item.clone());
        apply_implicit_item_stack_components(&entity, &stack);

        // Vanilla `Mth.floor((Mth.wrapDegrees(context.getRotation() - 180.0F) + 22.5F) / 45.0F) * 45.0F`.
        let yaw = ((wrap_degrees(place_context.rotation() - 180.0) + 22.5) / 45.0).floor() * 45.0;
        entity.set_rotation((yaw, 0.0));
        entity.set_old_position_to_current();
        entity.base().set_old_rotation_to_current();

        if let Err(error) = add_spawned_entity(context.world, Arc::clone(&entity)) {
            log::warn!("Failed to add armor stand to the world: {error:?}");
            return InteractionResult::Fail;
        }

        let position = entity.position();
        context.world.play_sound_at(
            &sound_events::ENTITY_ARMOR_STAND_PLACE,
            SoundSource::Blocks,
            position,
            0.75,
            0.8,
            None,
        );
        context.world.game_event_at(
            &vanilla_game_events::ENTITY_PLACE,
            position,
            &GameEventContext::new(Some(&**context.player), None),
        );

        context.inv.with_item(|item| item.shrink(1));
        InteractionResult::Success
    }
}

/// Vanilla `Vec3.atBottomCenterOf`.
fn bottom_center(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()),
        f64::from(pos.z()) + 0.5,
    )
}
