//! Vanilla `Salmon` (`net.minecraft.world.entity.animal.fish.Salmon`).
//!
//! A schooling `AbstractFish` with three synced size variants; the shared water
//! behaviour lives in [`super::abstract_fish`].

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::SALMON_SIZE;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SalmonEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::SchoolingFish;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

use super::abstract_fish::{self, SchoolingState};

/// Vanilla `Salmon.Variant`, shared with the `salmon/size` item component.
///
/// Deviation: Steel keeps the closed variant enum in `steel_registry` so the synced
/// value and the bucket component use one representation; `id()` is vanilla
/// `Variant.id()`.
pub use steel_registry::entity_variant::SalmonVariant as Variant;

/// Vanilla `Salmon.getMaxSchoolSize`.
pub(super) const MAX_SCHOOL_SIZE: i32 = 5;
/// Vanilla `Salmon.TAG_TYPE`.
const TAG_TYPE: &str = "type";
/// Vanilla `Mob` default step height used when the attribute is missing.
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Salmon.finalizeSpawn` weighted variant table (`WeightedList.Builder`):
/// 30 small / 50 medium / 15 large, i.e. a `nextInt(totalWeight)` over 95 slots.
const VARIANT_ROLL_BOUND: i32 = 95;
const SMALL_WEIGHT: i32 = 30;
const MEDIUM_WEIGHT: i32 = 50;
const LARGE_WEIGHT: i32 = 15;

#[entity_behavior(class = "Salmon")]
/// Vanilla salmon: a schooling bucketable fish with three sizes.
pub struct SalmonEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<SalmonEntityData>,
    schooling: SyncMutex<SchoolingState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SalmonEntity`.
unsafe impl DowncastType for SalmonEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/salmon");
}

impl SalmonEntity {
    /// Creates a new salmon at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a salmon from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        abstract_fish::initialize_water_pathfinding_malus(&mob_base);
        let mut entity_data = SalmonEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        abstract_fish::register_abstract_fish_goals(&mob_base);
        abstract_fish::register_schooling_goals(&mob_base);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            schooling: SyncMutex::new(SchoolingState::new()),
        };
        // Vanilla `Salmon` constructor: `refreshDimensions()`.
        entity.refresh_dimensions();
        entity
    }

    /// Vanilla `Salmon.getVariant`.
    #[must_use]
    pub fn variant(&self) -> Variant {
        Variant::by_id(*self.entity_data.lock().variant_type.get())
    }

    /// Vanilla `Salmon.setVariant`.
    ///
    /// Vanilla refreshes dimensions from `onSyncedDataUpdated`; Steel has no synced-data
    /// callback, so the setter refreshes them directly.
    pub fn set_variant(&self, variant: Variant) {
        self.entity_data.lock().variant_type.set(variant.id());
        self.refresh_dimensions();
    }

    /// Vanilla `Salmon.getSalmonScale`.
    #[must_use]
    pub fn get_salmon_scale(&self) -> f32 {
        self.variant().bounding_box_scale()
    }

    /// Vanilla `AbstractFish.fromBucket`.
    #[must_use]
    #[expect(
        clippy::wrong_self_convention,
        reason = "vanilla `AbstractFish.fromBucket` getter; the name is shared with the other bucketable fish entities"
    )]
    pub fn from_bucket(&self) -> bool {
        *self.entity_data.lock().abstract_fish.from_bucket.get()
    }

    /// Vanilla `AbstractFish.setFromBucket`.
    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.entity_data
            .lock()
            .abstract_fish
            .from_bucket
            .set(from_bucket);
    }

    /// Vanilla `AbstractSchoolingFish` flock state.
    pub(super) const fn schooling_state(&self) -> &SyncMutex<SchoolingState> {
        &self.schooling
    }

    /// Vanilla `Salmon.getBucketItemStack`.
    #[must_use]
    pub fn bucket_item_stack() -> ItemStack {
        ItemStack::new(&vanilla_items::SALMON_BUCKET)
    }

    /// Vanilla `AbstractFish.getPickupSound`.
    #[must_use]
    pub fn pickup_sound() -> SoundEventRef {
        &sound_events::ITEM_BUCKET_FILL_FISH
    }

    /// Vanilla `Salmon.saveToBucketTag`: the default bucket data plus `salmon/size`.
    fn create_bucket_stack(&self) -> ItemStack {
        let mut stack = Self::bucket_item_stack();
        abstract_fish::save_default_bucket_data(self, &mut stack);
        stack.set(SALMON_SIZE, self.variant());
        stack
    }

    /// Vanilla `Salmon.loadFromBucketTag` (`Bucketable.loadDefaultDataFromBucketTag`).
    pub fn load_from_bucket_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        abstract_fish::load_default_bucket_data(self, nbt);
    }

    /// Vanilla `AbstractFish.mobInteract`'s bucket half (`Bucketable.bucketMobPickup`).
    fn try_bucket_pickup(&self, player: &Player, hand: InteractionHand) -> bool {
        abstract_fish::try_bucket_pickup(self, player, hand, Self::pickup_sound(), || {
            self.create_bucket_stack()
        })
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }

        let display = self.living_base.mob_effect_display_state();

        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }

        self.entity_data.set_base_invisible_flag(display.invisible);
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }
}

impl Entity for SalmonEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    /// Vanilla `WaterAnimal.baseTick`: the generic base tick, then the water air supply.
    fn base_tick(&self) {
        let air_supply = self.air_supply();
        Mob::base_tick_mob(self);
        abstract_fish::handle_air_supply(self, air_supply);
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
        // Vanilla `AbstractSchoolingFish.tick` after `super.tick()`.
        abstract_fish::tick_schooling(entity.as_ref());
    }

    /// Vanilla `Salmon.getDefaultDimensions` = the fish dimensions scaled by
    /// `getSalmonScale`.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        let base = if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        };
        base.scale(self.get_salmon_scale())
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    /// Vanilla `AbstractFish.playStepSound` is empty.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn swim_sound(&self) -> SoundEventRef {
        abstract_fish::swim_sound()
    }

    /// Vanilla `WaterAnimal.isPushedByFluid`.
    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        // Vanilla `Salmon.applyImplicitComponent` for `salmon/size`.
        if let Some(variant) = item_stack.get(SALMON_SIZE) {
            self.set_variant(*variant);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("FromBucket", i8::from(self.from_bucket()));
        // Vanilla `Salmon.addAdditionalSaveData`: `Salmon.Variant.CODEC` is the
        // serialized name.
        nbt.insert(TAG_TYPE, self.variant().serialized_name());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_from_bucket(nbt.byte("FromBucket").is_some_and(|value| value != 0));
        self.set_variant(
            nbt.string(TAG_TYPE)
                .and_then(|name| Variant::from_serialized_name(name.to_str().as_ref()))
                .unwrap_or_default(),
        );
    }

    fn as_schooling_fish(&self) -> Option<&dyn SchoolingFish> {
        Some(self)
    }
}

impl LivingEntity for SalmonEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SALMON_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SALMON_DEATH)
    }

    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        _base_gravity: f64,
        _is_falling: bool,
        _old_y: f64,
    ) -> Option<MoveResult> {
        abstract_fish::travel_in_water(entity, input)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        abstract_fish::tick_flop(self, &sound_events::ENTITY_SALMON_FLOP);
        Mob::mob_ai_step(self, entity)
    }
}

impl Mob for SalmonEntity {
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        // Vanilla `AbstractFish.mobInteract`: bucket pickup, else `super.mobInteract`
        // (`Mob` default is `PASS`).
        if self.try_bucket_pickup(player, hand) {
            return InteractionResult::Success;
        }
        InteractionResult::Pass
    }
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `AbstractSchoolingFish.getMaxSpawnClusterSize` = `getMaxSchoolSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        MAX_SCHOOL_SIZE
    }

    /// Vanilla `Salmon.finalizeSpawn`: roll the weighted size, then the schooling spawn
    /// group data.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let roll = rand::random_range(0..VARIANT_ROLL_BOUND);
        if roll < SMALL_WEIGHT {
            self.set_variant(Variant::Small);
        } else if roll < SMALL_WEIGHT + MEDIUM_WEIGHT {
            self.set_variant(Variant::Medium);
        } else if roll < SMALL_WEIGHT + MEDIUM_WEIGHT + LARGE_WEIGHT {
            self.set_variant(Variant::Large);
        }

        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SALMON_AMBIENT)
    }

    fn ambient_sound_interval(&self) -> i32 {
        abstract_fish::AMBIENT_SOUND_INTERVAL
    }

    /// Vanilla `WaterAnimal.canBeLeashed`.
    fn mob_can_be_leashed(&self) -> bool {
        false
    }

    /// Vanilla `AbstractFish.requiresCustomPersistence`.
    fn requires_custom_persistence(&self) -> bool {
        abstract_fish::requires_custom_persistence(self, self.from_bucket())
    }

    /// Vanilla `AbstractFish.removeWhenFarAway`.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        abstract_fish::remove_when_far_away(self, self.from_bucket())
    }

    /// Vanilla `WaterAnimal.checkSpawnObstruction` = `level.isUnobstructed(this)`.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        abstract_fish::check_water_animal_spawn_obstruction(world, self)
    }

    /// Vanilla `WaterAnimal.getBaseExperienceReward`.
    fn base_experience_reward_mob(&self) -> i32 {
        abstract_fish::base_experience_reward()
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl SchoolingFish for SalmonEntity {
    fn has_followers(&self) -> bool {
        self.schooling.lock().has_followers()
    }

    fn is_follower(&self) -> bool {
        self.schooling.lock().is_follower()
    }

    fn can_be_followed(&self) -> bool {
        self.schooling.lock().can_be_followed(MAX_SCHOOL_SIZE)
    }

    fn add_followers(&self, followers: Vec<SharedEntity>) {
        abstract_fish::add_followers(&self.schooling, self, followers, MAX_SCHOOL_SIZE);
    }

    fn in_range_of_leader(&self) -> bool {
        abstract_fish::in_range_of_leader(&self.schooling, self)
    }

    fn path_to_leader(&self, mob: &SharedEntity) {
        abstract_fish::path_to_leader(&self.schooling, mob);
    }

    fn stop_following(&self) {
        abstract_fish::stop_following(&self.schooling);
    }
}

impl PathfinderMob for SalmonEntity {}
