//! Hanging entity item behavior (`HangingEntityItem`).
//!
//! Vanilla reference: `HangingEntityItem.useOn`
//! (`/tmp/vanilla/src/world/item/HangingEntityItem.java:36-70`):
//!
//! 1. `pos = clickedPos`, `clickedFace = getClickedFace()`,
//!    `blockPos = pos.relative(clickedFace)`;
//! 2. `mayPlace(player, clickedFace, stack, blockPos)` gates the placement
//!    (overridden by [`ItemFrameItem`]);
//! 3. the concrete hanging entity is constructed directly:
//!    `Painting.create(level, blockPos, clickedFace)` (empty when no variant
//!    fits → `CONSUME`), `new ItemFrame(level, blockPos, clickedFace)`, or
//!    `new GlowItemFrame(level, blockPos, clickedFace)`; any other type returns
//!    `SUCCESS` without placing anything;
//! 4. `EntityType.createDefaultStackConfig(level, stack, player).apply(entity)`;
//! 5. when `entity.survives()`: `playPlacementSound()`,
//!    `gameEvent(player, ENTITY_PLACE, entity.position())`, `addFreshEntity`,
//!    `stack.shrink(1)`, `SUCCESS`; otherwise `CONSUME`.
//!
//! The hanging entities are built through their concrete constructors, exactly
//! like vanilla: the generic entity factory
//! ([`create_entity_instance`](crate::entity::create_entity_instance)) can only
//! place an entity at a position, while a hanging entity is defined by its
//! backing block *and* facing (and, for paintings, by the variant chosen for the
//! available space).

use std::sync::Arc;

use steel_macros::item_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_events;
use steel_utils::{BlockPos, Direction, Downcast as _};

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseOnContext};
use crate::entity::entities::ItemFrameEntity;
use crate::entity::entities::objects::display_ui::{GlowItemFrameEntity, PaintingEntity};
use crate::entity::{
    SharedEntity, add_spawned_entity, apply_implicit_item_stack_components, next_entity_id,
};
use crate::player::Player;
use crate::world::game_event::GameEventContext;

/// Outcome of resolving the hanging entity for a placement attempt.
enum HangingCreation {
    /// The entity was constructed and is ready for the stack-config/survival
    /// steps.
    Entity(SharedEntity),
    /// Vanilla `Painting.create` found no placeable variant at this spot, which
    /// the item reports as `CONSUME`.
    NoSpace,
    /// Vanilla does not know this hanging type and reports `SUCCESS` without
    /// placing anything.
    Unknown,
}

/// Behavior for items that place a hanging entity (paintings and item frames).
///
/// `painting` uses this class directly; `item_frame`/`glow_item_frame` use
/// [`ItemFrameItem`], which only overrides the `mayPlace` gate exactly like
/// vanilla `ItemFrameItem`.
#[item_behavior(class = "HangingEntityItem")]
pub struct HangingEntityItem {
    /// The hanging entity type this item places.
    #[json_arg(vanilla_entities, json = "type")]
    pub entity_type: EntityTypeRef,
}

impl HangingEntityItem {
    /// Creates a hanging entity item behavior for `entity_type`.
    #[must_use]
    pub const fn new(entity_type: EntityTypeRef) -> Self {
        Self { entity_type }
    }

    /// Vanilla `HangingEntityItem.mayPlace`.
    ///
    /// Requires a horizontal clicked face and adventure-mode placement
    /// permission.
    fn may_place(
        player: &Player,
        direction: Direction,
        stack: &ItemStack,
        block_pos: BlockPos,
    ) -> bool {
        !direction.get_axis().is_vertical() && player.may_use_item_at(block_pos, direction, stack)
    }

    /// Runs vanilla `useOn` with the caller's `mayPlace` override.
    pub(crate) fn use_on_with(
        &self,
        context: &mut UseOnContext,
        may_place: fn(&Player, Direction, &ItemStack, BlockPos) -> bool,
    ) -> InteractionResult {
        let clicked_pos = context.hit_result.block_pos;
        let clicked_face = context.hit_result.direction;
        let block_pos = clicked_face.relative(clicked_pos);
        let stack = context.inv.with_item(|item| item.clone());

        if !may_place(context.player, clicked_face, &stack, block_pos) {
            return InteractionResult::Fail;
        }

        let entity = match self.create_hanging_entity(context, block_pos, clicked_face) {
            HangingCreation::Entity(entity) => entity,
            HangingCreation::NoSpace => return InteractionResult::Consume,
            HangingCreation::Unknown => return InteractionResult::Success,
        };

        apply_implicit_item_stack_components(&entity, &stack);

        if !hanging_entity_survives(&entity) {
            return InteractionResult::Consume;
        }

        play_placement_sound(&entity);
        context.world.game_event_at(
            &vanilla_game_events::ENTITY_PLACE,
            entity.position(),
            &GameEventContext::new(Some(&**context.player), None),
        );
        if let Err(error) = add_spawned_entity(context.world, Arc::clone(&entity)) {
            log::warn!(
                "Failed to add hanging entity {} to the world: {error:?}",
                self.entity_type.key
            );
        }

        context.inv.with_item(|item| item.shrink(1));
        InteractionResult::Success
    }

    /// Constructs the hanging entity for the clicked block and face.
    ///
    /// Mirrors vanilla's direct `new ItemFrame` / `new GlowItemFrame`
    /// construction and `Painting.create`'s variant selection.
    fn create_hanging_entity(
        &self,
        context: &UseOnContext,
        block_pos: BlockPos,
        clicked_face: Direction,
    ) -> HangingCreation {
        let world = context.world;

        if self.entity_type == &vanilla_entities::PAINTING {
            // Vanilla `Painting.create(level, blockPos, clickedFace)`: the
            // largest placeable variant that fits is chosen, and an empty result
            // means the item consumes the action without placing.
            return match PaintingEntity::create_attached(world, block_pos, clicked_face) {
                Some(painting) => HangingCreation::Entity(Arc::new(painting)),
                None => HangingCreation::NoSpace,
            };
        }

        if self.entity_type == &vanilla_entities::ITEM_FRAME {
            let frame: SharedEntity = Arc::new(ItemFrameEntity::new_attached(
                self.entity_type,
                next_entity_id(),
                block_pos,
                clicked_face,
                Arc::downgrade(world),
            ));
            return HangingCreation::Entity(frame);
        }

        if self.entity_type == &vanilla_entities::GLOW_ITEM_FRAME {
            let frame: SharedEntity = Arc::new(GlowItemFrameEntity::new_attached(
                self.entity_type,
                next_entity_id(),
                block_pos,
                clicked_face,
                Arc::downgrade(world),
            ));
            return HangingCreation::Entity(frame);
        }

        // Vanilla returns `SUCCESS` for hanging item types it does not know.
        HangingCreation::Unknown
    }
}

impl ItemBehavior for HangingEntityItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        self.use_on_with(context, Self::may_place)
    }
}

/// Vanilla `HangingEntity.survives` gate.
///
/// Paintings only arrive here after
/// [`PaintingEntity::create_attached`] accepted a variant whose bounding box
/// survives, so their gate is already satisfied; the frames are checked
/// directly.
fn hanging_entity_survives(entity: &SharedEntity) -> bool {
    if let Some(frame) = entity.as_ref().downcast_ref::<ItemFrameEntity>() {
        return frame.survives();
    }
    if let Some(frame) = entity.as_ref().downcast_ref::<GlowItemFrameEntity>() {
        return frame.survives();
    }
    true
}

/// Vanilla `HangingEntity.playPlacementSound`: the concrete hanging entity plays
/// its own placement sound at its position with volume `1.0` and pitch `1.0`.
fn play_placement_sound(entity: &SharedEntity) {
    if let Some(frame) = entity.as_ref().downcast_ref::<ItemFrameEntity>() {
        frame.play_placement_sound();
    } else if let Some(frame) = entity.as_ref().downcast_ref::<GlowItemFrameEntity>() {
        frame.play_placement_sound();
    } else if let Some(painting) = entity.as_ref().downcast_ref::<PaintingEntity>() {
        painting.play_placement_sound();
    }
}
