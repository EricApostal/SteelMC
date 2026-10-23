//! Minecart item behavior (`MinecartItem`).
//!
//! Vanilla reference: `MinecartItem.useOn`
//! (`/tmp/vanilla/src/world/item/MinecartItem.java:26-60`):
//!
//! 1. the clicked block must be in `BlockTags.RAILS`, otherwise `FAIL`;
//! 2. the rail shape comes from `BaseRailBlock.getShapeProperty()` and defaults
//!    to `RailShape.NORTH_SOUTH` for rail-tagged blocks that are not
//!    `BaseRailBlock` instances;
//! 3. a sloped shape raises the spawn position by `0.5`, on top of the constant
//!    `0.0625` rail height: `pos + (0.5, 0.0625 + slopeOffset, 0.5)`;
//! 4. `AbstractMinecart.createMinecart(level, x, y, z, type, EntitySpawnReason.DISPENSER, stack, player)`
//!    creates the cart at that position and applies the item stack components;
//! 5. when `AbstractMinecart.useExperimentalMovement(level)` is enabled the cart
//!    is rejected if another `AbstractMinecart` already occupies its bounding
//!    box;
//! 6. server-side the cart is added and `GameEvent.ENTITY_PLACE` fires at the
//!    clicked rail position with the block below as the affected state;
//! 7. `stack.shrink(1)` (unconditionally, unlike boats) and `SUCCESS`.
//!
//! Vanilla's `useExperimentalMovement` tests the `minecart_improvements`
//! feature flag, which is experimental and therefore absent from a default
//! world. Steel has no feature-flag system at all (no `FeatureFlag` /
//! `enabled_features` / `MINECART_IMPROVEMENTS` hits under `steel-core/src` or
//! `steel-registry/src`), so the guard is skipped exactly like a vanilla world
//! with default features.
//!
//! Minecarts are created through the generated entity factory
//! ([`create_entity_instance`]). Until
//! `steel-core/src/entity/generated/entities.rs` registers the minecart entity
//! types (`vanilla_entities::MINECART`, …), creation reports
//! `EntitySpawnError::MissingFactory` and the item returns `FAIL`, exactly like
//! vanilla when `AbstractMinecart.createMinecart` returns `null`.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, EnumProperty, RailShape};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_game_events;

use crate::behavior::{BLOCK_BEHAVIORS, InteractionResult, ItemBehavior, UseOnContext};
use crate::entity::{
    add_spawned_entity, apply_implicit_item_stack_components, create_entity_instance,
};
use crate::world::game_event::GameEventContext;

/// Vanilla `MinecartItem` constant rail height offset.
const RAIL_HEIGHT_OFFSET: f64 = 0.0625;
/// Vanilla `MinecartItem` extra offset for sloped rail shapes.
const SLOPE_HEIGHT_OFFSET: f64 = 0.5;

const RAIL_SHAPE: &EnumProperty<RailShape> = &BlockStateProperties::RAIL_SHAPE;
const RAIL_SHAPE_STRAIGHT: &EnumProperty<RailShape> = &BlockStateProperties::RAIL_SHAPE_STRAIGHT;

/// Behavior for every minecart item (`minecart`, `chest_minecart`, …).
#[item_behavior]
pub struct MinecartItem {
    /// The minecart entity type this item places.
    #[json_arg(vanilla_entities, json = "type")]
    pub entity_type: EntityTypeRef,
}

impl MinecartItem {
    /// Creates a minecart item behavior for `entity_type`.
    #[must_use]
    pub const fn new(entity_type: EntityTypeRef) -> Self {
        Self { entity_type }
    }

    /// Vanilla `BaseRailBlock.getShapeProperty()` lookup for a rail state.
    ///
    /// Straight rail blocks (powered/detector) use the straight shape property;
    /// every other rail block uses the full shape property. Non-rail blocks fall
    /// back to `RailShape.NORTH_SOUTH`, matching vanilla's
    /// `blockState.getBlock() instanceof BaseRailBlock` test.
    fn rail_shape(state: steel_utils::BlockStateId) -> RailShape {
        let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
        let Some(rail) = behavior.as_rail() else {
            return RailShape::NorthSouth;
        };

        let property = if rail.is_straight() {
            RAIL_SHAPE_STRAIGHT
        } else {
            RAIL_SHAPE
        };
        state
            .try_get_value(property)
            .unwrap_or(RailShape::NorthSouth)
    }
}

impl ItemBehavior for MinecartItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let pos = context.hit_result.block_pos;
        let state = context.world.get_block_state(pos);

        if !state.get_block().has_tag(&BlockTag::RAILS) {
            return InteractionResult::Fail;
        }

        let shape = Self::rail_shape(state);
        let slope_offset = if shape.is_slope() {
            SLOPE_HEIGHT_OFFSET
        } else {
            0.0
        };
        let spawn_pos = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + RAIL_HEIGHT_OFFSET + slope_offset,
            f64::from(pos.z()) + 0.5,
        );

        let entity = match create_entity_instance(context.world, self.entity_type, spawn_pos) {
            Ok(entity) => entity,
            Err(error) => {
                log::warn!(
                    "Failed to create minecart {} at {spawn_pos:?}: {error:?}",
                    self.entity_type.key
                );
                return InteractionResult::Fail;
            }
        };

        let stack = context.inv.with_item(|item| item.clone());
        apply_implicit_item_stack_components(&entity, &stack);

        if let Err(error) = add_spawned_entity(context.world, Arc::clone(&entity)) {
            log::warn!("Failed to add minecart to the world: {error:?}");
            return InteractionResult::Fail;
        }
        context.world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            pos,
            &GameEventContext::new(
                Some(&**context.player),
                Some(context.world.get_block_state(pos.below())),
            ),
        );

        context.inv.with_item(|item| item.shrink(1));
        InteractionResult::Success
    }
}
