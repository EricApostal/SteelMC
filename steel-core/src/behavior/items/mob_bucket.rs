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
//! Unknown entity factories fail closed: no entity and no game event, exactly
//! like vanilla when `EntityType.create` yields no mob.

use std::io::Cursor;

use simdnbt::borrow::read_compound;
use simdnbt::owned::NbtCompound;
use steel_macros::item_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::data_components::components::CustomData;
use steel_registry::data_components::vanilla_components::BUCKET_ENTITY_DATA;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_events;
use steel_utils::BlockPos;

use crate::behavior::item_utils::{create_filled_result, get_player_pov_hit_result};
use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};
use crate::entity::{
    Entity as _, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, SharedEntity,
    spawn_entity,
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
        let Ok(entity) = spawn_entity(context.world, request) else {
            return;
        };
        Self::apply_bucket_contents(&entity, stack);
        context.world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            spawn_pos,
            &GameEventContext::new(Some(context.player.as_ref()), None),
        );
    }

    /// Mirrors vanilla `MobBucketItem.spawn`'s `Bucketable` half: feeds the
    /// bucket's `bucket_entity_data` into `loadFromBucketTag` and marks the
    /// released mob as from-bucket.
    ///
    /// Vanilla reads `stack.getOrDefault(BUCKET_ENTITY_DATA, CustomData.EMPTY)`,
    /// so an absent component still runs both calls, over an empty tag. The
    /// owned component is re-borrowed into the saved-NBT view the entity
    /// loaders share with chunk deserialization, matching
    /// [`Entity::load_with_owned_components`].
    fn apply_bucket_contents(entity: &SharedEntity, stack: &ItemStack) {
        let tag = stack
            .get(BUCKET_ENTITY_DATA)
            .map_or_else(NbtCompound::new, CustomData::copy_tag);
        let mut bytes = Vec::new();
        tag.write(&mut bytes);
        if let Ok(borrowed) = read_compound(&mut Cursor::new(bytes.as_slice())) {
            entity.load_from_bucket_tag((&borrowed).into());
        }
        entity.set_from_bucket(true);
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::{
        AxolotlVariant, init_vanilla_registry, items::ItemRef, vanilla_blocks, vanilla_entities,
        vanilla_items,
    };
    use steel_utils::{
        BlockPos, ChunkPos, Downcast as _, WorldAabb,
        types::{InteractionHand, UpdateFlags},
    };

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::entities::mobs::hostile::cube::sulfur_cube::SulfurCubeEntity;
    use crate::entity::entities::mobs::passive::axolotl::AxolotlEntity;
    use crate::entity::entities::mobs::passive::tadpole::TadpoleEntity;
    use crate::entity::entities::mobs::water::abstract_fish;
    use crate::entity::entities::mobs::water::cod::CodEntity;
    use crate::entity::entities::mobs::water::salmon::SalmonEntity;
    use crate::entity::init_entities;
    use crate::entity::{AgeableMob, Entity, LivingEntity, Mob, SharedEntity};
    use crate::player::Player;
    use crate::player::game_mode::use_item;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };
    use crate::world::World;

    /// Stone wall under the player's eye ray, far enough that placed fluids and
    /// released mobs land clear of the player.
    const AIM_TARGET: BlockPos = BlockPos::new(0, 66, 3);

    /// A world with that wall, and a player looking straight at its north face.
    fn release_fixture(name: &'static str) -> (TestWorld, Arc<Player>) {
        init_vanilla_registry();
        init_entities();
        init_behaviors();

        let world_fixture = fresh_test_world(name);
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        assert!(world.set_block(
            AIM_TARGET,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL
        ));

        let player = TestPlayerBuilder::new(Arc::clone(world), "BucketPlayer", 1).build();
        // Eye at y ~66.62 hits the wall's north face two blocks ahead.
        player.base().set_position_local(DVec3::new(0.5, 65.0, 0.5));
        player.set_rotation((0.0, 0.0));
        (world_fixture, player)
    }

    /// Spawns `entity_type` for the pickup half of a bucket round trip.
    fn pickup_subject(world: &Arc<World>, entity_type: EntityTypeRef) -> SharedEntity {
        spawn_entity(
            world,
            EntitySpawnRequest {
                entity_type,
                placement: EntitySpawnPlacement::Block {
                    pos: BlockPos::new(4, 65, 4),
                    try_move_down: false,
                    moved_up: false,
                },
                reason: EntitySpawnReason::Natural,
                finalize_spawn: true,
                play_ambient_sound: false,
                item_stack: None,
                user_is_operator: false,
            },
        )
        .expect("the round-trip subject spawns")
    }

    /// Picks the subject up with `pickup_bucket`, then releases it through the
    /// real `MobBucketItem.use_item` path. Returns the released mob found near
    /// the wall.
    fn pickup_and_release(
        world: &Arc<World>,
        player: &Arc<Player>,
        subject: &SharedEntity,
        pickup_bucket: ItemRef,
    ) -> SharedEntity {
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(pickup_bucket));

        let Some(mob) = subject.as_mob() else {
            panic!("the round-trip subject is a mob");
        };
        assert!(
            Mob::mob_interact(mob, player.as_ref(), InteractionHand::MainHand).consumes_action(),
            "the hand bucket captures the subject"
        );
        assert_eq!(
            use_item(player, world, InteractionHand::MainHand),
            InteractionResult::Success,
            "the mob bucket releases its mob"
        );

        let mut matches = world.get_entities_in_aabb_matching(
            &WorldAabb::new(0.0, 63.0, 0.0, 2.0, 70.0, 4.0),
            |entity| entity.entity_type() == subject.entity_type() && !entity.is_removed(),
        );
        assert_eq!(
            matches.len(),
            1,
            "exactly one live mob of the released type is near the wall"
        );
        matches.pop().expect("one released mob")
    }

    /// `Cod.saveToBucketTag` only writes `BUCKET_ENTITY_DATA`, so a released cod
    /// reads its defaults back with `from_bucket = true`.
    #[test]
    fn released_cod_restores_default_bucket_tag_and_marks_from_bucket() {
        let (world_fixture, player) = release_fixture("mob_bucket_cod_round_trip");
        let world = &world_fixture.world;

        let subject = pickup_subject(world, &vanilla_entities::COD);
        let cod = subject
            .downcast_ref::<CodEntity>()
            .expect("the subject is a cod");
        assert!(!cod.from_bucket(), "bucketed mobs start unmarked");

        let released = pickup_and_release(world, &player, &subject, &vanilla_items::WATER_BUCKET);
        let released = released
            .downcast_ref::<CodEntity>()
            .expect("the released mob is a cod");
        assert!(released.from_bucket(), "release marks the mob from-bucket");
        assert!(
            abstract_fish::requires_custom_persistence(released, released.from_bucket()),
            "from-bucket fish persist like vanilla `AbstractFish`"
        );
        assert!(
            !abstract_fish::remove_when_far_away(released, released.from_bucket()),
            "from-bucket fish do not despawn like vanilla `AbstractFish`"
        );
    }

    /// Salmon's bucket payload is the shared default tag: bucket a salmon with
    /// `NoAI` and half health, release it, and the released salmon keeps both.
    #[test]
    fn released_salmon_restores_bucket_health_and_no_ai() {
        let (world_fixture, player) = release_fixture("mob_bucket_salmon_round_trip");
        let world = &world_fixture.world;

        let subject = pickup_subject(world, &vanilla_entities::SALMON);
        let salmon = subject
            .downcast_ref::<SalmonEntity>()
            .expect("the subject is a salmon");
        Mob::set_no_ai(salmon, true);
        LivingEntity::set_health(salmon, 1.5);

        let released = pickup_and_release(world, &player, &subject, &vanilla_items::WATER_BUCKET);
        let released = released
            .downcast_ref::<SalmonEntity>()
            .expect("the released mob is a salmon");
        assert!(released.from_bucket(), "release marks the mob from-bucket");
        assert!(
            Mob::is_no_ai(released),
            "release restores `NoAI` from `bucket_entity_data`"
        );
        assert_eq!(
            LivingEntity::get_health(released),
            1.5,
            "release restores `Health` from `bucket_entity_data`; a \
             default-only release would heal to full (3.0)"
        );
    }

    /// Axolotl round trip: a rare blue variant through the `AXOLOTL_VARIANT`
    /// component plus the baby age, age lock and health from
    /// `bucket_entity_data`.
    #[test]
    fn released_axolotl_restores_variant_age_and_health() {
        let (world_fixture, player) = release_fixture("mob_bucket_axolotl_round_trip");
        let world = &world_fixture.world;

        let subject = pickup_subject(world, &vanilla_entities::AXOLOTL);
        let axolotl = subject
            .downcast_ref::<AxolotlEntity>()
            .expect("the subject is an axolotl");
        axolotl.set_variant(AxolotlVariant::Blue);
        AgeableMob::set_age(axolotl, -24_000);
        AgeableMob::set_age_locked(axolotl, true);
        LivingEntity::set_health(axolotl, 3.0);

        let released = pickup_and_release(world, &player, &subject, &vanilla_items::WATER_BUCKET);
        let released = released
            .downcast_ref::<AxolotlEntity>()
            .expect("the released mob is an axolotl");
        assert!(released.from_bucket(), "release marks the mob from-bucket");
        assert_eq!(
            released.variant(),
            AxolotlVariant::Blue,
            "release restores the `AXOLOTL_VARIANT` component; a \
             default-only release keeps the component path too, but only when set"
        );
        assert!(
            AgeableMob::is_baby(released),
            "release restores the bucketed `Age`; a default-only release spawns an adult"
        );
        assert!(
            AgeableMob::is_age_locked(released),
            "release restores `AgeLocked` from `bucket_entity_data`"
        );
        assert_eq!(
            LivingEntity::get_health(released),
            3.0,
            "release restores `Health` from `bucket_entity_data`; a \
             default-only release would heal to full (14.0)"
        );
    }

    /// Tadpole round trip: an almost-grown, age-locked tadpole keeps its `Age`,
    /// `AgeLocked` and half health on release.
    #[test]
    fn released_tadpole_restores_age_and_health() {
        let (world_fixture, player) = release_fixture("mob_bucket_tadpole_round_trip");
        let world = &world_fixture.world;

        let subject = pickup_subject(world, &vanilla_entities::TADPOLE);
        let tadpole = subject
            .downcast_ref::<TadpoleEntity>()
            .expect("the subject is a tadpole");
        tadpole.set_age(1_200);
        tadpole.set_age_locked(true);
        LivingEntity::set_health(tadpole, 2.0);

        let released = pickup_and_release(world, &player, &subject, &vanilla_items::WATER_BUCKET);
        let released = released
            .downcast_ref::<TadpoleEntity>()
            .expect("the released mob is a tadpole");
        assert!(released.from_bucket(), "release marks the mob from-bucket");
        assert_eq!(
            released.age(),
            1_200,
            "release restores `Age` from `bucket_entity_data`; a \
             default-only release spawns age zero"
        );
        assert!(
            released.is_age_locked(),
            "release restores `AgeLocked` from `bucket_entity_data`"
        );
        assert_eq!(
            LivingEntity::get_health(released),
            2.0,
            "release restores `Health` from `bucket_entity_data`; a \
             default-only release would heal to full (6.0)"
        );
    }

    /// Sulfur-cube round trip: a plain bucket captures the cube and the release
    /// restores the `age`/`age_locked` payload plus `from_bucket`.
    #[test]
    fn released_sulfur_cube_restores_age_and_marks_from_bucket() {
        let (world_fixture, player) = release_fixture("mob_bucket_sulfur_cube_round_trip");
        let world = &world_fixture.world;

        let subject = pickup_subject(world, &vanilla_entities::SULFUR_CUBE);
        let cube = subject
            .downcast_ref::<SulfurCubeEntity>()
            .expect("the subject is a sulfur cube");
        AgeableMob::set_age(cube, 2_400);
        AgeableMob::set_age_locked(cube, true);
        LivingEntity::set_health(cube, 4.0);

        let released = pickup_and_release(world, &player, &subject, &vanilla_items::BUCKET);
        let released = released
            .downcast_ref::<SulfurCubeEntity>()
            .expect("the released mob is a sulfur cube");
        assert!(released.from_bucket(), "release marks the mob from-bucket");
        assert_eq!(
            AgeableMob::get_age(released),
            2_400,
            "release restores `age` from `bucket_entity_data`; a \
             default-only release spawns age zero"
        );
        assert!(
            released.is_age_locked(),
            "release restores `age_locked` from `bucket_entity_data`"
        );
        assert_eq!(
            LivingEntity::get_health(released),
            4.0,
            "release restores `Health` from `bucket_entity_data`"
        );
    }
}
