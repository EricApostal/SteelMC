//! Vanilla Frog entity with variant, tongue-target and brain parity.
//!
//! Mirrors `net.minecraft.world.entity.animal.frog.Frog`. Vanilla drives frogs from
//! `FrogAi`; the brain package lives in `frog/brain.rs` (owned by the BrainMobs
//! lane) and is wired here exactly like vanilla `Frog.makeBrain`/
//! `Frog.customServerAiStep`. This module owns the entity, its synched data and the
//! [`ShootTongue`] behavior the brain's `TONGUE` activity needs.
//!
//! GAP: the vanilla `Frog.FrogLookControl` (`resetXRotOnTick` keyed on the tongue
//! target) and `Frog.FrogNodeEvaluator`/`FrogPathNavigation` (`FROG_PREFER_JUMP_TO`,
//! water-border corner cutting) are not ported: Steel's shared look control and path
//! navigation are used instead.
//!
//! GAP: `Frog.calculateFallDamage` subtracts 5 from fall damage; Steel has no
//! overridable fall-damage hook (`LivingEntityBase::calculate_fall_damage` is called
//! internally), so frogs take vanilla-default fall damage.

use std::str::FromStr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::FROG_VARIANT;
use steel_registry::entity_data::EntityPose;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::frog_variant::FrogVariantRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::shared_structs::pick_spawn_conditioned_entry;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::FrogEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, RegistryExt, RegistryReference, TaggedRegistryExt, sound_events, vanilla_attributes,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

mod brain;
mod shoot_tongue;

pub(crate) use shoot_tongue::ShootTongue;

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::world::World;

const FROG_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.125, -0.08)];
const FROG_BABY_WIDTH: f32 = 0.166_666_67;
const FROG_BABY_HEIGHT: f32 = 0.166_666_67;
const FROG_BABY_EYE_HEIGHT: f32 = 0.141_666_67;

const FROG_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    FROG_BABY_WIDTH,
    FROG_BABY_HEIGHT,
    FROG_BABY_EYE_HEIGHT,
    EntityAttachments::new(&FROG_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

// GAP: vanilla `Frog.getHeadRotSpeed() == 35` has no Steel hook; the shared look
// control uses its own yaw step (see the module docs).
/// Vanilla `Frog.getMaxHeadYRot`.
const MAX_HEAD_Y_ROT: f32 = 5.0;
/// Vanilla `Frog.playEatingSound` volume/pitch.
const EAT_SOUND_VOLUME: f32 = 2.0;
const EAT_SOUND_PITCH: f32 = 1.0;

#[entity_behavior(class = "Frog")]
/// Vanilla frog entity with synced variant, tongue target and brain state.
pub struct FrogEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    /// Vanilla `LivingEntity.brain`; `frog/brain.rs` (BrainMobs) builds the real
    /// contents through the `FrogBrainProvider` package and owns the activities.
    brain: SyncMutex<Brain>,
    entity_data: SyncMutex<FrogEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `FrogEntity`.
unsafe impl DowncastType for FrogEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/frog");
}

impl FrogEntity {
    /// Creates a new frog at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a frog from persisted base entity state.
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
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        // Vanilla `Frog(WATER, 4.0F)`; the trapdoor malus is applied by the shared
        // node evaluator defaults.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, 4.0);
        // Vanilla `Frog` uses an amphibious navigation: float-capable paths.
        mob_base.navigation().lock().set_can_float(true);
        let mut entity_data = FrogEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        // Vanilla `Frog.registerGoals` inherits `Mob.registerGoals`, which is empty:
        // all frog behavior comes from the brain activities.

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            brain: SyncMutex::new(Brain::default()),
            entity_data: SyncMutex::new(entity_data),
        };
        // Vanilla `Frog.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        *entity.brain.lock() = brain::make_brain(&entity);
        entity
    }

    /// Sets the active frog variant by registry entry.
    pub fn set_variant(&self, variant: FrogVariantRef) {
        self.entity_data
            .lock()
            .variant
            .set(RegistryReference::new(variant));
    }

    /// Returns the active frog variant, falling back to temperate when invalid.
    #[must_use]
    pub fn variant(&self) -> FrogVariantRef {
        self.entity_data.lock().variant.get().value()
    }

    fn set_variant_by_key(&self, key: &Identifier) -> bool {
        let Some(variant) = REGISTRY.frog_variants.by_key(key) else {
            return false;
        };
        self.set_variant(variant);
        true
    }

    /// Vanilla `Frog.getTongueTarget`: the entity id the tongue is striking.
    ///
    /// Vanilla resolves the id to an entity on the client; the renderer needs the id.
    #[must_use]
    pub fn tongue_target_id(&self) -> Option<i32> {
        self.entity_data
            .lock()
            .tongue_target
            .get()
            .and_then(|id| i32::try_from(id).ok())
    }

    /// Vanilla `Frog.getTongueTarget`, resolved against the world.
    #[must_use]
    pub fn tongue_target(&self) -> Option<SharedEntity> {
        let id = self.tongue_target_id()?;
        self.level()?.get_entity_by_id(id)
    }

    /// Vanilla `Frog.setTongueTarget`.
    pub fn set_tongue_target(&self, target: &SharedEntity) {
        let id = u32::try_from(target.id()).unwrap_or(u32::MAX);
        self.entity_data.lock().tongue_target.set(Some(id));
    }

    /// Vanilla `Frog.eraseTongueTarget`.
    pub fn erase_tongue_target(&self) {
        self.entity_data.lock().tongue_target.set(None);
    }

    /// Vanilla `Frog.DATA_POSE == Pose.CROAKING`, driven by the `Croak` behavior.
    #[must_use]
    pub fn is_croaking(&self) -> bool {
        self.pose() == EntityPose::Croaking
    }

    /// Vanilla `Frog.getTongueTarget`-driven pose used by `ShootTongue`.
    #[must_use]
    pub fn is_using_tongue(&self) -> bool {
        self.pose() == EntityPose::UsingTongue
    }

    /// Vanilla `Frog.canEat`: frog-food entity types may be eaten.
    ///
    /// GAP: vanilla also rejects `AbstractCubeMob`s with `size != 1`; Steel's cube
    /// mobs (`mob/hostile/cube`) expose no size accessor yet, so only the
    /// `FROG_FOOD` entity-type tag is tested.
    #[must_use]
    pub fn can_eat(&self, entity: &dyn LivingEntity) -> bool {
        REGISTRY
            .entity_types
            .is_in_tag(entity.entity_type(), &EntityTypeTag::FROG_FOOD)
    }

    /// Returns whether an item stack matches the vanilla frog food tag.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::FROG_FOOD)
    }

    /// Vanilla `FrogAi.getTemptations`.
    #[must_use]
    pub fn get_temptations(item_stack: &ItemStack) -> bool {
        Self::is_food(item_stack)
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

impl Entity for FrogEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(variant) = item_stack.get(FROG_VARIANT) {
            self.set_variant(variant.value());
        }
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            FROG_BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_FROG_STEP, 0.15, 1.0);
    }

    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        // Vanilla `VariantUtils.writeVariant` stores the variant id under `variant`.
        nbt.insert("variant", self.variant().key.to_string());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);

        if let Some(variant) = nbt.string("variant")
            && let Ok(key) = Identifier::from_str(variant.to_str().as_ref())
        {
            self.set_variant_by_key(&key);
        }
    }
}

impl LivingEntity for FrogEntity {
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
        Some(&sound_events::ENTITY_FROG_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_FROG_DEATH)
    }

    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        _base_gravity: f64,
        _is_falling: bool,
        _old_y: f64,
    ) -> Option<MoveResult> {
        // Vanilla `Frog.travelInWater`.
        self.move_relative(self.get_speed(), input);
        let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
        self.set_velocity(self.velocity() * 0.9);
        Some(result)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }
}

impl AgeableMob for FrogEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        &self.ageable_base
    }

    fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().ageable_mob().age_locked.get()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.entity_data
            .lock()
            .ageable_mob_mut()
            .age_locked
            .set(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.entity_data.lock().ageable_mob_mut().baby.set(baby);
    }

    fn age_boundary_changed(&self, _baby: bool) {
        self.refresh_dimensions();
    }

    fn breed_variant_key(&self) -> Option<&Identifier> {
        Some(&self.variant().key)
    }

    fn set_breed_variant_key(&self, key: &Identifier) -> bool {
        self.set_variant_by_key(key)
    }

    fn initialize_breed_offspring(&self, partner: &dyn AgeableMob, offspring: &dyn AgeableMob) {
        // Vanilla frog offspring inherit one parent's variant.
        let use_self_variant = rand::random::<bool>();
        let variant_key = if use_self_variant {
            self.breed_variant_key()
        } else {
            partner.breed_variant_key()
        };
        let Some(variant_key) = variant_key else {
            return;
        };

        if !offspring.set_breed_variant_key(variant_key) {
            log::error!("frog offspring could not inherit breeding variant {variant_key}");
        }
    }
}

impl Animal for FrogEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        FrogEntity::is_food(item_stack)
    }

    fn play_eating_sound(&self) {
        if let Some(world) = self.level() {
            world.play_sound_at(
                &sound_events::ENTITY_FROG_EAT,
                SoundSource::Neutral,
                self.position(),
                EAT_SOUND_VOLUME,
                EAT_SOUND_PITCH,
                None,
            );
        }
    }

    fn spawn_child_from_breeding(&self, world: &Arc<World>, partner: &dyn Animal) {
        // Vanilla `Frog.spawnChildFromBreeding`: the offspring is created without a
        // variant, and the mother becomes pregnant for the `LAY_SPAWN` activity.
        self.finalize_spawn_child_from_breeding(world, partner, None);
        self.brain.lock().set_memory(&mem::IS_PREGNANT, Some(()));
    }
}

impl Mob for FrogEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla `Frog.customServerAiStep`: tick the brain, then `FrogAi.updateActivity`.
        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn custom_server_ai_step(&self) {
        Animal::custom_server_ai_step_animal(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_FROG_AMBIENT)
    }

    fn max_head_y_rot(&self) -> f32 {
        MAX_HEAD_Y_ROT
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `Frog.finalizeSpawn`: pick the biome-conditioned variant, then let
        // `FrogAi.initMemories` seed the long-jump cooldown (BrainMobs owns that).
        let variant = {
            let mut random = LegacyRandom::from_seed(rand::random());
            world.biome_at(self.block_position()).and_then(|biome| {
                pick_spawn_conditioned_entry(
                    REGISTRY.frog_variants.iter().map(|(_, variant)| variant),
                    |variant| variant.spawn_conditions,
                    biome,
                    &mut random,
                )
            })
        };

        if let Some(variant) = variant {
            self.set_variant(variant);
        }

        // Vanilla `Frog.finalizeSpawn` -> `FrogAi.initMemories`.
        brain::init_memories(&mut self.brain.lock());

        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        Animal::mob_interact_animal(self, player, hand)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for FrogEntity {}
