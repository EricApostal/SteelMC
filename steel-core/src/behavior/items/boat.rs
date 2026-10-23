//! Boat item behavior (`BoatItem`).
//!
//! Vanilla reference: `BoatItem.use`
//! (`/tmp/vanilla/src/world/item/BoatItem.java:33-80`):
//!
//! 1. raycast from the player's eyes with `ClipContext.Fluid.ANY`; a miss
//!    returns `PASS` (the item is not consumed);
//! 2. entity-pickup guard: entities that can be picked and whose bounding box
//!    inflated by their pick radius contains the player's eye position would be
//!    interacted with instead, so the item returns `PASS`. The search box is the
//!    player's bounding box extended by five blocks along the view vector and
//!    inflated by one;
//! 3. a block hit creates the boat at the exact hit location with
//!    `EntitySpawnReason.SPAWN_ITEM_USE`, applies the item stack components,
//!    sets the yaw to the player's yaw, and requires
//!    `level.noCollision(boat, boat.getBoundingBox())`;
//! 4. server-side the boat is added, `GameEvent.ENTITY_PLACE` fires at the hit
//!    location and one boat is consumed;
//! 5. `Stats.ITEM_USED` is awarded and `SUCCESS` is returned.
//!
//! Boats are created through the generated entity factory
//! ([`create_entity_instance`]). Until
//! `steel-core/src/entity/generated/entities.rs` registers the boat entity types
//! (`vanilla_entities::OAK_BOAT`, …), creation reports
//! `EntitySpawnError::MissingFactory` and the item returns `FAIL`, exactly like
//! vanilla when `EntityType.create` returns `null`.

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_events;
use steel_utils::WorldAabb;

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::item_utils::get_player_pov_hit_result;
use crate::entity::{
    Entity, add_spawned_entity, apply_implicit_item_stack_components, create_entity_instance,
};
use crate::world::ClipFluid;
use crate::world::game_event::GameEventContext;

/// Vanilla `BoatItem` pickup-guard reach along the view vector.
const PICKUP_GUARD_REACH: f64 = 5.0;
/// Vanilla `BoatItem` pickup-guard box inflation.
const PICKUP_GUARD_INFLATION: f64 = 1.0;

/// Behavior for every boat item (`oak_boat` … `bamboo_raft`).
#[item_behavior]
pub struct BoatItem {
    /// The boat entity type this item places.
    #[json_arg(vanilla_entities, json = "entity_type")]
    pub entity_type: EntityTypeRef,
}

impl BoatItem {
    /// Creates a boat item behavior for `entity_type`.
    #[must_use]
    pub const fn new(entity_type: EntityTypeRef) -> Self {
        Self { entity_type }
    }

    /// Vanilla entity-pickup guard: returns `true` when a pickable entity whose
    /// inflated bounding box contains the player's eyes would take the
    /// interaction instead of placing a boat.
    fn entity_blocks_placement(context: &UseItemContext<'_>) -> bool {
        let player = context.player;
        let view_vector = player.look_angle();
        let search_box = player
            .bounding_box()
            .expand_towards(view_vector * PICKUP_GUARD_REACH)
            .inflate(PICKUP_GUARD_INFLATION);
        let eye_position = player.position().with_y(player.get_eye_y());
        let player_id = player.id();

        context
            .world
            .get_entities_in_aabb(&search_box)
            .into_iter()
            .filter(|entity| entity.id() != player_id && entity.is_pickable())
            .any(|entity| {
                // Vanilla `Entity.getPickRadius` is `0.0` for everything Steel
                // models today, so the inflation is a no-op.
                let pick_box: WorldAabb = entity.bounding_box();
                pick_box.contains(eye_position)
            })
    }
}

impl ItemBehavior for BoatItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let hit_result = get_player_pov_hit_result(context.world, context.player, ClipFluid::Any);
        if hit_result.miss {
            return InteractionResult::Pass;
        }

        if Self::entity_blocks_placement(context) {
            return InteractionResult::Pass;
        }

        let entity =
            match create_entity_instance(context.world, self.entity_type, hit_result.location) {
                Ok(entity) => entity,
                Err(error) => {
                    log::warn!(
                        "Failed to create boat {} at {:?}: {error:?}",
                        self.entity_type.key,
                        hit_result.location
                    );
                    return InteractionResult::Fail;
                }
            };

        let stack = context.inv.with_item(|item| item.clone());
        apply_implicit_item_stack_components(&entity, &stack);

        // Vanilla `boat.setYRot(player.getYRot())`: only the yaw changes.
        let (player_yaw, _) = context.player.rotation();
        let (_, pitch) = entity.rotation();
        entity.set_rotation((player_yaw, pitch));

        if !context.world.no_collision(entity.bounding_box()) {
            return InteractionResult::Fail;
        }

        if let Err(error) = add_spawned_entity(context.world, Arc::clone(&entity)) {
            log::warn!("Failed to add boat to the world: {error:?}");
            return InteractionResult::Fail;
        }
        context.world.game_event_at(
            &vanilla_game_events::ENTITY_PLACE,
            hit_result.location,
            &GameEventContext::new(Some(&**context.player), None),
        );

        let infinite = context.player.has_infinite_materials();
        context.inv.with_item(|item| item.consume_one(infinite));
        context
            .player
            .award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        InteractionResult::Success
    }
}
