//! Mob bucket item behavior (`MobBucketItem`).
//!
//! Mirrors vanilla `MobBucketItem`: a bucket with a fixed fluid content and a
//! fixed entity type. Using it places the fluid through the shared
//! [`BucketItem`](super::BucketItem) path and then spawns the carried mob at
//! the placement position, applying the bucket's item components (custom name,
//! `bucket_entity_data`, `entity_data`) to it.
//!
//! Vanilla parity notes:
//! - `MobBucketItem.playEmptySound` overrides the bucket default with a
//!   per-item sound, so the shared placement path takes that sound as a
//!   parameter.
//! - `MobBucketItem.emptyContents` returns `true` without touching the world
//!   when the content is `Fluids.EMPTY` (`sulfur_cube_bucket`); that path plays
//!   the empty sound and still spawns the mob.
//!
//! **Vanilla differences:** the per-entity `Bucketable.loadFromBucketTag`
//! restoration (axolotl variant, tropical-fish pattern, ...) belongs to the
//! entity behaviors, which do not exist yet for these types; generic item
//! components are applied and unknown entity factories fail closed.

use steel_macros::item_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_events;
use steel_utils::BlockPos;

use crate::behavior::item_utils::{create_filled_result, get_player_pov_hit_result};
use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};
use crate::entity::{
    Entity as _, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, spawn_entity,
};
use crate::world::ClipFluid;
use crate::world::game_event::GameEventContext;

use super::bucket::{empty_success_stack, use_filled_bucket};

/// Behavior for fish, axolotl, tadpole, and sulfur-cube buckets.
#[item_behavior]
pub struct MobBucketItem {
    /// The mob released when the bucket is emptied.
    /// `classes.json` key: `type` (e.g. `"axolotl"`).
    #[json_arg(vanilla_entities, json = "type")]
    entity_type: EntityTypeRef,
    /// The sound played when this bucket is emptied.
    /// `classes.json` key: `empty_sound` (e.g. `"item.bucket.empty_fish"`).
    #[json_arg(sound_events, json = "empty_sound")]
    empty_sound: SoundEventRef,
    /// The fluid content of this bucket.
    /// `classes.json` key: `content` (`"water"`, or `"empty"` for
    /// `sulfur_cube_bucket`). Vanilla stores a `Fluid`; Steel models bucket
    /// contents as their fluid block, so `None` is `Fluids.EMPTY`.
    #[json_arg(vanilla_blocks, json = "content", optional = "empty")]
    content_block: Option<BlockRef>,
}

impl MobBucketItem {
    /// Creates a mob bucket behavior.
    #[must_use]
    pub const fn new(
        entity_type: EntityTypeRef,
        empty_sound: SoundEventRef,
        content_block: Option<BlockRef>,
    ) -> Self {
        Self {
            entity_type,
            empty_sound,
            content_block,
        }
    }

    /// Mirrors vanilla `MobBucketItem.checkExtraContent`: spawns the carried
    /// mob at `spawn_pos` through the shared spawn coordinator (which applies
    /// the item's entity components and `BUCKET` spawn reason), then emits
    /// `ENTITY_PLACE`.
    ///
    /// A missing entity factory or invalid spawn position fails closed: no
    /// entity and no game event, exactly like vanilla when `create` returns
    /// `null`.
    fn check_extra_content(
        &self,
        stack: &ItemStack,
        spawn_pos: BlockPos,
        context: &UseItemContext,
    ) {
        let request = EntitySpawnRequest {
            entity_type: self.entity_type,
            placement: EntitySpawnPlacement::Block {
                pos: spawn_pos,
                // Vanilla `MobBucketItem.spawn` calls `create(..., true, false)`.
                try_move_down: true,
                moved_up: false,
            },
            reason: EntitySpawnReason::Bucket,
            finalize_spawn: true,
            play_ambient_sound: true,
            item_stack: Some(stack),
            user_is_operator: context.player.is_operator(),
        };
        if spawn_entity(&context.world, request).is_err() {
            return;
        }
        context.world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            spawn_pos,
            &GameEventContext::new(Some(context.player.as_ref()), None),
        );
    }

    /// Mirrors `MobBucketItem.playEmptySound`: only the per-item sound, at
    /// `SoundSource.NEUTRAL` with volume and pitch 1.0.
    ///
    /// The `FLUID_PLACE` game event is *not* part of this override; the filled
    /// path emits it from the shared bucket placement
    /// ([`super::bucket::use_filled_bucket`]).
    fn play_empty_sound(&self, context: &UseItemContext) {
        context.player.play_sound(self.empty_sound, 1.0, 1.0);
    }

    /// Mirrors `MobBucketItem.emptyContents` for `Fluids.EMPTY` content
    /// (`sulfur_cube_bucket`): no block is placed, the empty sound plays, and
    /// the mob is still released.
    ///
    /// Replays vanilla `BucketItem.use`'s head with the bucket's
    /// `ClipContext.Fluid.NONE` context (`MobBucketItem.getFluidContext`),
    /// then awards `ITEM_USED` and swaps the emptied stack for a bucket.
    fn use_empty_content(&self, context: &mut UseItemContext) -> InteractionResult {
        let hit_result = get_player_pov_hit_result(context.world, context.player, ClipFluid::None);
        if hit_result.miss {
            return InteractionResult::Pass;
        }

        let clicked_pos = hit_result.block_pos;
        let direction = hit_result.direction;
        let place_pos = direction.relative(clicked_pos);

        let stack = context.inv.with_item(|item| item.clone());
        if !context.world.may_interact(context.player, clicked_pos)
            || !context.player.may_use_item_at(place_pos, direction, &stack)
        {
            return InteractionResult::Fail;
        }

        self.play_empty_sound(context);
        self.check_extra_content(&stack, place_pos, context);
        context
            .player
            .award_stat(&vanilla_stat_types::ITEM_USED, stack.item());
        create_filled_result(context, empty_success_stack(context), true);
        InteractionResult::Success
    }
}

impl ItemBehavior for MobBucketItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let Some(content_block) = self.content_block else {
            return self.use_empty_content(context);
        };

        // Vanilla passes the pre-consumption stack to `checkExtraContent`, so
        // capture it before the shared path replaces the hand stack.
        let stack = context.inv.with_item(|item| item.clone());
        let (result, placed_pos) =
            use_filled_bucket(content_block, Some(self.empty_sound), context);
        if let Some(pos) = placed_pos {
            self.check_extra_content(&stack, pos, context);
        }
        result
    }
}
