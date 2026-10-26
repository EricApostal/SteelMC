//! Vanilla Tadpole entity with growth-into-frog, age lock and bucket parity.
//!
//! Mirrors `net.minecraft.world.entity.animal.frog.Tadpole` (which extends
//! `AbstractFish` extends `WaterAnimal extends PathfinderMob`). Tadpoles are brain
//! mobs: [`brain`] mirrors `TadpoleAi`, the entity owns the growth counter, the
//! synced age lock, the flop behavior and the bucket round trip.
//!
//! Unlike `AgeableMob`s, vanilla tadpoles keep a private `age` counter that counts
//! *up* to [`TICKS_TO_BE_FROG`] and then converts the entity into a frog; the
//! `AgeLocked` synced flag pauses that counter.
//!
//! ## Deviations
//!
//! - Vanilla `Tadpole.TadpoleMoveControl`/`SmoothSwimmingLookControl` and the
//!   `WaterBoundPathNavigation` are not ported; Steel's shared move/look controls and
//!   navigation are used with the vanilla `WATER` malus of 0.
//! - `WaterAnimal.handleAirSupply` is already covered by
//!   `LivingEntity::base_tick_living_entity`'s air-supply tick, so it is not repeated.
//! - Release: `MobBucketItem` runs [`TadpoleEntity::load_from_bucket_data`] and
//!   `setFromBucket(true)` on the spawned mob (vanilla `Bucketable` half of
//!   `MobBucketItem.spawn`).

mod brain;

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::components::CustomData;
use steel_registry::data_components::vanilla_components::{BUCKET_ENTITY_DATA, CUSTOM_NAME};
use steel_registry::entity_data::EntityPose;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::TadpoleEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_entities, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::behavior::advancement_hooks::raise_filled_bucket;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::leash::Leashable;
use crate::entity::{AgeableMobBase, ConversionParams};
use crate::entity::{
    AnimalBase, Entity, EntityBase, EntityBaseLoad, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, RemovalReason, SharedEntity,
};
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::World;

/// Vanilla `Tadpole.ticksToBeFrog`.
pub const TICKS_TO_BE_FROG: i32 = 24_000;
/// Vanilla `Tadpole.HITBOX_WIDTH`/`HITBOX_HEIGHT` live on the entity type.
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `WaterAnimal.AMBIENT_SOUND_INTERVAL`.
const AMBIENT_SOUND_INTERVAL: i32 = 120;
/// Vanilla `Tadpole.ageLockParticleTimer` reset value.
const AGE_LOCK_PARTICLE_TIMER: i32 = 40;
/// Vanilla `AbstractFish.aiStep` flop impulse.
const FLOP_HORIZONTAL_IMPULSE: f64 = 0.05;
const FLOP_VERTICAL_IMPULSE: f64 = 0.4;
/// Vanilla `AbstractFish.travelInWater` input speed.
const WATER_TRAVEL_SPEED: f32 = 0.01;
/// Vanilla `AbstractFish.travelInWater` drag and sink impulse.
const WATER_TRAVEL_DRAG: f64 = 0.9;
const WATER_SINK_IMPULSE: f64 = -0.005;
/// Vanilla `AbstractFish` bucket pickup/empty sounds play at volume/pitch 1.
const BUCKET_SOUND_VOLUME: f32 = 1.0;
const BUCKET_SOUND_PITCH: f32 = 1.0;

/// Runtime state vanilla keeps in `Tadpole` fields rather than synched data.
#[derive(Debug, Clone, Copy, Default)]
struct TadpoleState {
    age: i32,
    age_lock_particle_timer: i32,
}

#[entity_behavior(class = "Tadpole")]
/// Vanilla tadpole entity with growth, age lock and bucket state.
pub struct TadpoleEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<TadpoleEntityData>,
    state: SyncMutex<TadpoleState>,
    brain: SyncMutex<Brain>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TadpoleEntity`.
unsafe impl DowncastType for TadpoleEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/tadpole");
}

impl TadpoleEntity {
    /// Creates a new tadpole at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a tadpole from persisted base entity state.
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
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        // Vanilla `WaterAnimal(WATER, 0.0F)`.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, 0.0);
        let mut entity_data = TadpoleEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(TadpoleState::default()),
            brain: SyncMutex::new(Brain::default()),
        };
        // Vanilla `Tadpole.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        *entity.brain.lock() = brain::make_brain(&entity);
        entity
    }

    /// Vanilla `Tadpole.getAge` (the growth counter, not `AgeableMob.age`).
    #[must_use]
    pub fn age(&self) -> i32 {
        self.state.lock().age
    }

    /// Vanilla `Tadpole.setAge`: reaching [`TICKS_TO_BE_FROG`] grows the tadpole up.
    pub fn set_age(&self, new_age: i32) {
        {
            self.state.lock().age = new_age;
        }
        if new_age >= TICKS_TO_BE_FROG {
            self.age_up();
        }
    }

    /// Vanilla `Tadpole.ageUp()` (no-argument): convert into a frog.
    fn age_up(&self) {
        let Some(world) = self.level() else {
            return;
        };

        let previous_dimensions = self.dimensions_for_pose(self.pose());
        let converted = self.convert_to(
            &vanilla_entities::FROG,
            ConversionParams::single(false, false),
            Some(&move |mob: &dyn Mob| {
                let _ = mob.finalize_spawn(&world, EntitySpawnReason::Conversion, None);
                mob.set_persistence_required();
                mob.fudge_position_after_size_change(previous_dimensions);
                mob.play_sound(&sound_events::ENTITY_TADPOLE_GROW_UP, 0.15, 1.0);
            }),
        );

        if converted.is_none() {
            log::warn!("tadpole could not grow into a frog");
        }
    }

    /// Vanilla `Tadpole.getTicksLeftUntilAdult`.
    #[must_use]
    pub fn ticks_left_until_adult(&self) -> i32 {
        (TICKS_TO_BE_FROG - self.age()).max(0)
    }

    /// Vanilla `Tadpole.isAgeLocked`.
    #[must_use]
    pub fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().age_locked.get()
    }

    /// Vanilla `Tadpole.setAgeLocked`.
    pub fn set_age_locked(&self, locked: bool) {
        self.entity_data.lock().age_locked.set(locked);
    }

    /// Vanilla `Tadpole.ageLockParticleTimer`.
    #[must_use]
    pub fn age_lock_particle_timer(&self) -> i32 {
        self.state.lock().age_lock_particle_timer
    }

    fn set_age_lock_particle_timer(&self, timer: i32) {
        self.state.lock().age_lock_particle_timer = timer;
    }

    /// Vanilla `Tadpole.setAgeLockedData`: toggle the lock and restart the counter.
    fn set_age_locked_data(&self) {
        self.set_age_locked(!self.is_age_locked());
        self.set_age(0);
        self.set_age_lock_particle_timer(AGE_LOCK_PARTICLE_TIMER);
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

    /// Vanilla `Tadpole.getBucketItemStack`.
    #[must_use]
    pub fn bucket_item_stack() -> ItemStack {
        ItemStack::new(&vanilla_items::TADPOLE_BUCKET)
    }

    /// Vanilla `Tadpole.getPickupSound`.
    #[must_use]
    pub fn pickup_sound() -> SoundEventRef {
        &sound_events::ITEM_BUCKET_FILL_TADPOLE
    }

    /// Vanilla `Tadpole.saveToBucketTag` (`Bucketable.saveDefaultDataToBucketTag` plus
    /// the tadpole's `Age`/`AgeLocked`).
    fn create_bucket_stack(&self) -> ItemStack {
        let mut stack = Self::bucket_item_stack();

        let mut tag = NbtCompound::new();
        if self.is_no_ai() {
            tag.insert("NoAI", 1i8);
        }
        if self.is_silent() {
            tag.insert("Silent", 1i8);
        }
        if self.is_no_gravity() {
            tag.insert("NoGravity", 1i8);
        }
        if self.has_glowing_tag() {
            tag.insert("Glowing", 1i8);
        }
        if self.is_invulnerable() {
            tag.insert("Invulnerable", 1i8);
        }
        if self.is_persistence_required() {
            tag.insert("PersistenceRequired", 1i8);
        }
        tag.insert("Health", self.get_health());
        tag.insert("Age", self.age());
        tag.insert("AgeLocked", i8::from(self.is_age_locked()));

        if let Some(custom_data) = CustomData::try_from_compound(tag) {
            stack.set(BUCKET_ENTITY_DATA, custom_data);
        }
        if let Some(custom_name) = self.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }

        stack
    }

    /// Vanilla `Tadpole.loadFromBucketTag`.
    pub fn load_from_bucket_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        if let Some(no_ai) = nbt.byte("NoAI") {
            self.set_no_ai(no_ai != 0);
        }
        if let Some(silent) = nbt.byte("Silent") {
            self.set_silent(silent != 0);
        }
        if let Some(no_gravity) = nbt.byte("NoGravity") {
            self.set_no_gravity(no_gravity != 0);
        }
        if let Some(glowing) = nbt.byte("Glowing") {
            self.set_glowing_tag(glowing != 0);
        }
        if let Some(invulnerable) = nbt.byte("Invulnerable") {
            self.set_invulnerable(invulnerable != 0);
        }
        if nbt
            .byte("PersistenceRequired")
            .is_some_and(|value| value != 0)
        {
            self.set_persistence_required();
        }
        if let Some(health) = nbt.float("Health") {
            self.set_health(health);
        }
        if let Some(age) = nbt.int("Age") {
            self.set_age(age);
        }
        self.set_age_locked(nbt.byte("AgeLocked").is_some_and(|value| value != 0));
    }

    /// Vanilla `Tadpole.mobInteract`'s bucket half (`Bucketable.bucketMobPickup`).
    fn try_bucket_pickup(&self, player: &Player, hand: InteractionHand) -> bool {
        let holds_water_bucket = {
            let inventory = player.inventory.lock();
            inventory
                .get_item_in_hand(hand)
                .is(&vanilla_items::WATER_BUCKET)
        };
        if !holds_water_bucket || !Entity::is_alive(self) {
            return false;
        }

        self.play_sound(
            Self::pickup_sound(),
            BUCKET_SOUND_VOLUME,
            BUCKET_SOUND_PITCH,
        );
        let bucket = self.create_bucket_stack();
        let filled = bucket.clone();
        let overflow = {
            let mut inventory = player.inventory.lock();
            inventory.apply_filled_result(hand, bucket, player.has_infinite_materials(), false)
        };
        if !overflow.is_empty() {
            let _ = player.drop_item(overflow, false, Prediction::Predicted);
        }

        // Vanilla `Bucketable.bucketMobPickup` raises `filled_bucket` right after
        // `ItemUtils.createFilledResult`.
        if let Some(world) = Entity::level(self) {
            raise_filled_bucket(player, &world, &filled);
        }

        self.set_removed(RemovalReason::Discarded);
        true
    }

    /// Vanilla `Tadpole.isFood`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::FROG_FOOD)
    }

    /// Vanilla `Tadpole.feed`: consume one item and speed up growth.
    fn feed(&self, player: &Player, hand: InteractionHand) {
        Mob::use_player_item(self, player, hand);
        let seconds =
            AgeableMobBase::get_speed_up_seconds_when_feeding(self.ticks_left_until_adult());
        // Vanilla `Tadpole.ageUp(seconds)`: `setAge(age + seconds * 20)`.
        self.set_age(self.age() + seconds * 20);
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

        // Sync base entity flags from resolved effect display state in one place.
        self.entity_data.set_base_invisible_flag(display.invisible);
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }
}

impl Entity for TadpoleEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
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
        &sound_events::ENTITY_FISH_SWIM
    }

    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("Age", self.age());
        nbt.insert("AgeLocked", i8::from(self.is_age_locked()));
        nbt.insert("FromBucket", i8::from(self.from_bucket()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_age(nbt.int("Age").unwrap_or(0));
        self.set_age_locked(nbt.byte("AgeLocked").is_some_and(|value| value != 0));
        self.set_from_bucket(nbt.byte("FromBucket").is_some_and(|value| value != 0));
    }

    /// Vanilla `Tadpole.loadFromBucketTag`, restored by `MobBucketItem.spawn` on
    /// release.
    fn load_from_bucket_tag(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_from_bucket_data(nbt);
    }

    /// Vanilla `Bucketable.setFromBucket`, flipped for every released mob.
    fn set_from_bucket(&self, from_bucket: bool) {
        TadpoleEntity::set_from_bucket(self, from_bucket);
    }
}

impl LivingEntity for TadpoleEntity {
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

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_TADPOLE_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_TADPOLE_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        // Vanilla `Tadpole.shouldDropExperience`.
        false
    }

    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        _base_gravity: f64,
        _is_falling: bool,
        _old_y: f64,
    ) -> Option<MoveResult> {
        // Vanilla `AbstractFish.travelInWater`.
        self.move_relative(WATER_TRAVEL_SPEED, input);
        let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
        self.set_velocity(self.velocity() * WATER_TRAVEL_DRAG);
        if self.target().is_none() {
            self.set_velocity(self.velocity() + DVec3::new(0.0, WATER_SINK_IMPULSE, 0.0));
        }
        Some(result)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.tick_flop();
        let result = Mob::mob_ai_step(self, entity);
        self.tick_growth();
        result
    }
}

impl TadpoleEntity {
    /// Vanilla `AbstractFish.aiStep` flop: bounce and play the flop sound on land.
    fn tick_flop(&self) {
        if self.is_in_water() || !self.on_ground() || !self.vertical_collision() {
            return;
        }

        let velocity = self.velocity();
        self.set_velocity(DVec3::new(
            velocity.x + f64::from(rand::random::<f32>() * 2.0 - 1.0) * FLOP_HORIZONTAL_IMPULSE,
            FLOP_VERTICAL_IMPULSE,
            velocity.z + f64::from(rand::random::<f32>() * 2.0 - 1.0) * FLOP_HORIZONTAL_IMPULSE,
        ));
        self.set_on_ground(false);
        self.play_sound(&sound_events::ENTITY_TADPOLE_FLOP, 0.4, 1.0);
    }

    /// Vanilla `Tadpole.aiStep` growth counter and age-lock particle timer.
    fn tick_growth(&self) {
        if self.level().is_none() {
            return;
        }

        // Vanilla `AgeableMob.makeAgeLockedParticle` decrements the timer every tick.
        // GAP: the `HAPPY_VILLAGER` particles it emits while locked have no entity-side
        // particle spawn API in Steel yet (only world explosion/level events), so only
        // the timer half is ported.
        {
            let mut state = self.state.lock();
            if state.age_lock_particle_timer > 0 {
                state.age_lock_particle_timer -= 1;
            }
        }

        if self.is_age_locked() {
            return;
        }

        self.set_age(self.age() + 1);
    }
}

impl Mob for TadpoleEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    /// Vanilla `AbstractFish.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        8
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `Tadpole.customServerAiStep`: tick `TadpoleAi` (which includes
    /// `TadpoleAi.updateActivity`).
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Tadpole.getAmbientSound` returns `null`.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        None
    }

    fn ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn mob_can_be_leashed(&self) -> bool {
        // Vanilla `WaterAnimal.canBeLeashed`.
        false
    }

    fn requires_custom_persistence(&self) -> bool {
        (self.is_passenger() || self.as_leashable().is_some_and(Leashable::is_leashed))
            || self.from_bucket()
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        // Vanilla `AbstractFish.removeWhenFarAway`.
        !self.from_bucket() && self.custom_name().is_none()
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        // Vanilla `Tadpole.mobInteract`: feed, then golden-dandelion age lock, then
        // bucket pickup.
        if Self::is_food(&item_stack) && !self.is_age_locked() {
            self.feed(player, hand);
            return InteractionResult::Success;
        }

        if item_stack.is(&vanilla_items::GOLDEN_DANDELION)
            && self.age_lock_particle_timer() == 0
            && !REGISTRY
                .entity_types
                .is_in_tag(self.entity_type(), &EntityTypeTag::CANNOT_BE_AGE_LOCKED)
        {
            // Vanilla `AgeableMob.setAgeLocked(..., this::setAgeLockedData)`.
            Mob::use_player_item(self, player, hand);
            self.set_age_locked_data();
            self.set_persistence_required();

            if let Some(world) = self.level() {
                let sound = if self.is_age_locked() {
                    &sound_events::ITEM_GOLDEN_DANDELION_USE
                } else {
                    &sound_events::ITEM_GOLDEN_DANDELION_UNUSE
                };
                world.play_sound(
                    sound,
                    SoundSource::Players,
                    self.block_position(),
                    1.0,
                    1.0,
                    None,
                );
            }
            return InteractionResult::Success;
        }

        if self.try_bucket_pickup(player, hand) {
            return InteractionResult::Success;
        }

        InteractionResult::Pass
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for TadpoleEntity {}
