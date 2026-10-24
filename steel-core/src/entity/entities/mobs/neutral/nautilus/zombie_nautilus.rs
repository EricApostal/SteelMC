//! Vanilla `ZombieNautilus` (`world/entity/animal/nautilus/ZombieNautilus.java`):
//! the hostile nautilus variant with a synced biome variant, body-slot sun
//! protection and `ZombieNautilusAi`.
//!
//! ## Deviations
//!
//! - Vanilla `AgeableMob.canBeABaby()` has no Steel hook; `canBeABaby() == false` is
//!   expressed by [`ZombieNautilusEntity`]'s `is_baby`/`set_baby`/`get_baby_start_age`
//!   overrides plus skipping the `AgeableMob` NBT block, which is what
//!   `AgeableMob.addAdditionalSaveData`'s `canBeABaby()` gate effectively does.
//! - Vanilla `Entity.get(DataComponentType)` (`DataComponents.ZOMBIE_NAUTILUS_VARIANT`)
//!   has no Steel surface; only the inbound
//!   [`Entity::apply_implicit_item_components`] direction exists, matching `Parrot`.
//! - `VariantUtils.writeVariant`/`readVariant` store the registry key under
//!   `variant`; Steel reproduces that key form directly.

use std::str::FromStr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::ZOMBIE_NAUTILUS_VARIANT;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::shared_structs::pick_spawn_conditioned_entry;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::ZombieNautilusEntityData;
use steel_registry::zombie_nautilus_variant::{ZombieNautilusVariant, ZombieNautilusVariantRef};
use steel_registry::{REGISTRY, RegistryExt as _, RegistryReference, sound_events};
use steel_utils::Identifier;
use steel_utils::locks::SyncMutex;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey};
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::goal::Tameable;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::neutral::nautilus::abstract_nautilus::{
    self, AbstractNautilusParts, Nautilus,
};
use crate::entity::entities::mobs::neutral::nautilus::brain;
use crate::entity::leash::Leashable;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    MobEffectInstance, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `TamableAnimal` synced flag bits.
const TAMABLE_SITTING_FLAG: i8 = 1;
const TAMABLE_TAME_FLAG: i8 = 4;
/// Vanilla `TamableAnimal.TELEPORT_WHEN_DISTANCE_IS_SQ`.
const TELEPORT_WHEN_DISTANCE_SQ: f64 = 144.0;

#[entity_behavior(class = "ZombieNautilus")]
/// Vanilla zombie nautilus: the hostile, biome-variant nautilus.
pub struct ZombieNautilusEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    parts: AbstractNautilusParts,
    entity_data: SyncMutex<ZombieNautilusEntityData>,
    brain: SyncMutex<Brain>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ZombieNautilusEntity`.
unsafe impl DowncastType for ZombieNautilusEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/zombie_nautilus");
}

impl ZombieNautilusEntity {
    /// Creates a new zombie nautilus at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a zombie nautilus from persisted base entity state.
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
        // Vanilla `AbstractNautilus(AbstractFish)` sets `WATER` malus 0.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, 0.0);
        let mut entity_data = ZombieNautilusEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            parts: AbstractNautilusParts::new(abstract_nautilus::DEFAULT_INVENTORY_COLUMNS),
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(Brain::default()),
        };
        // Vanilla `ZombieNautilus.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        *entity.brain.lock() = brain::make_zombie_brain();
        entity
    }

    /// Vanilla `ZombieNautilus.getVariant`.
    #[must_use]
    pub fn variant(&self) -> RegistryReference<ZombieNautilusVariant> {
        *self.entity_data.lock().variant.get()
    }

    /// Vanilla `ZombieNautilus.setVariant`.
    pub fn set_variant(&self, variant: RegistryReference<ZombieNautilusVariant>) {
        self.entity_data.lock().variant.set(variant);
    }

    /// Vanilla `AbstractNautilus.isDashing`.
    #[must_use]
    pub fn is_dashing(&self) -> bool {
        *self.entity_data.lock().abstract_nautilus().dash.get()
    }

    /// Vanilla `AbstractNautilus.setDashing`.
    pub fn set_dashing(&self, is_dashing: bool) {
        self.entity_data
            .lock()
            .abstract_nautilus_mut()
            .dash
            .set(is_dashing);
    }

    /// Vanilla `TamableAnimal.isTame`.
    #[must_use]
    pub fn is_tame(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & TAMABLE_TAME_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setTame`.
    pub fn set_tame(&self, tame: bool) {
        set_tamable_flag(&self.entity_data, TAMABLE_TAME_FLAG, tame);
    }

    /// Vanilla `TamableAnimal.getOwnerReference`, reduced to the owner UUID.
    #[must_use]
    pub fn owner_uuid(&self) -> Option<Uuid> {
        *self.entity_data.lock().tamable_animal().owneruuid.get()
    }

    /// Vanilla `TamableAnimal.setOwnerReference`.
    pub fn set_owner_uuid(&self, owner: Option<Uuid>) {
        self.entity_data
            .lock()
            .tamable_animal_mut()
            .owneruuid
            .set(owner);
    }

    /// Vanilla `TamableAnimal.isInSittingPose`.
    #[must_use]
    pub fn is_in_sitting_pose(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & TAMABLE_SITTING_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setInSittingPose`.
    pub fn set_in_sitting_pose(&self, sitting: bool) {
        set_tamable_flag(&self.entity_data, TAMABLE_SITTING_FLAG, sitting);
    }

    /// Vanilla `AbstractNautilus.isAggravated`.
    #[must_use]
    pub fn is_aggravated(&self) -> bool {
        abstract_nautilus::is_aggravated(self)
    }

    /// Vanilla `AbstractNautilus.canJump`.
    #[must_use]
    pub fn can_jump(&self) -> bool {
        abstract_nautilus::can_jump(self)
    }

    /// Vanilla `AbstractNautilus.onPlayerJump`.
    pub fn on_player_jump(&self, jump_amount: i32) {
        abstract_nautilus::on_player_jump(self, jump_amount);
    }

    /// Vanilla `AbstractNautilus.handleStartJump`.
    pub fn handle_start_jump(&self) {
        abstract_nautilus::handle_start_jump(self);
    }

    /// Vanilla `AbstractNautilus.handleStopJump` is empty.
    pub const fn handle_stop_jump() {
        abstract_nautilus::handle_stop_jump();
    }

    /// Vanilla `AbstractNautilus.getJumpCooldown`.
    #[must_use]
    pub fn get_jump_cooldown(&self) -> i32 {
        abstract_nautilus::get_jump_cooldown(self)
    }

    /// Vanilla `AbstractNautilus.isMobControlled`.
    #[must_use]
    pub fn is_mob_controlled(&self) -> bool {
        abstract_nautilus::is_mob_controlled(self)
    }

    fn owner(&self) -> Option<SharedEntity> {
        let uuid = self.owner_uuid()?;
        let world = self.level()?;
        world
            .players
            .get_by_uuid(&uuid)
            .map(|player| -> SharedEntity { player })
    }

    fn owner_position(&self) -> Option<DVec3> {
        Some(self.owner()?.position())
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

    /// Vanilla `ZombieNautilus.getAmbientSound`.
    fn ambient_sound_for(&self) -> SoundEventRef {
        if self.is_under_water() {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_AMBIENT
        } else {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_AMBIENT_LAND
        }
    }

    /// Vanilla `ZombieNautilus.getHurtSound`.
    fn hurt_sound_for(&self) -> SoundEventRef {
        if self.is_under_water() {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_HURT
        } else {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_HURT_LAND
        }
    }

    /// Vanilla `ZombieNautilus.getDeathSound`.
    fn death_sound_for(&self) -> SoundEventRef {
        if self.is_under_water() {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_DEATH
        } else {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_DEATH_LAND
        }
    }
}

/// Sets one vanilla `TamableAnimal.DATA_FLAGS_ID` bit.
fn set_tamable_flag(entity_data: &SyncMutex<ZombieNautilusEntityData>, flag: i8, value: bool) {
    let mut entity_data = entity_data.lock();
    let current = *entity_data.tamable_animal().flags.get();
    let next = if value {
        current | flag
    } else {
        current & !flag
    };
    entity_data.tamable_animal_mut().flags.set(next);
}

/// Vanilla `VariantUtils.selectVariantToSpawn` over the zombie-nautilus variants.
fn select_variant_to_spawn(world: &World, pos: BlockPos) -> Option<ZombieNautilusVariantRef> {
    let mut random = LegacyRandom::from_seed(rand::random());
    world.biome_at(pos).and_then(|biome| {
        pick_spawn_conditioned_entry(
            REGISTRY
                .zombie_nautilus_variants
                .iter()
                .map(|(_, variant)| variant),
            |variant| variant.spawn_conditions,
            biome,
            &mut random,
        )
    })
}

impl Nautilus for ZombieNautilusEntity {
    fn parts(&self) -> &AbstractNautilusParts {
        &self.parts
    }

    fn is_dashing(&self) -> bool {
        ZombieNautilusEntity::is_dashing(self)
    }

    fn set_dashing(&self, dashing: bool) {
        ZombieNautilusEntity::set_dashing(self, dashing);
    }

    fn dash_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_under_water() {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_DASH
        } else {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_DASH_LAND
        })
    }

    fn dash_ready_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_under_water() {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_DASH_READY
        } else {
            &sound_events::ENTITY_ZOMBIE_NAUTILUS_DASH_READY_LAND
        })
    }

    fn set_tame_flag(&self, tame: bool) {
        ZombieNautilusEntity::set_tame(self, tame);
    }

    fn set_owner_uuid(&self, owner: Option<Uuid>) {
        ZombieNautilusEntity::set_owner_uuid(self, owner);
    }

    fn owner_uuid(&self) -> Option<Uuid> {
        ZombieNautilusEntity::owner_uuid(self)
    }

    fn brain_mutex(&self) -> &SyncMutex<Brain> {
        &self.brain
    }
}

impl Tameable for ZombieNautilusEntity {
    fn owner(&self) -> Option<SharedEntity> {
        ZombieNautilusEntity::owner(self)
    }

    fn is_tame(&self) -> bool {
        ZombieNautilusEntity::is_tame(self)
    }

    fn is_ordered_to_sit(&self) -> bool {
        self.parts.ordered_to_sit()
    }

    fn set_in_sitting_pose(&self, sitting: bool) {
        ZombieNautilusEntity::set_in_sitting_pose(self, sitting);
    }

    fn is_in_sitting_pose(&self) -> bool {
        ZombieNautilusEntity::is_in_sitting_pose(self)
    }

    fn unable_to_move_to_owner(&self) -> bool {
        self.is_ordered_to_sit()
            || self.is_passenger()
            || (self.as_leashable().is_some_and(Leashable::can_be_leashed)
                && !self.as_leashable().is_some_and(Leashable::is_leashed))
            || self
                .owner_uuid()
                .and_then(|uuid| self.level()?.players.get_by_uuid(&uuid))
                .is_some_and(|owner| owner.is_spectator())
    }

    fn should_try_teleport_to_owner(&self) -> bool {
        self.owner_position().is_some_and(|owner_pos| {
            self.position().distance_squared(owner_pos) >= TELEPORT_WHEN_DISTANCE_SQ
        })
    }

    fn try_to_teleport_to_owner(&self) {
        let Some(owner_pos) = self.owner_position() else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        for _ in 0..10 {
            let dx = rand::random_range(-3i32..=3);
            let dz = rand::random_range(-3i32..=3);
            if dx.abs() < 2 && dz.abs() < 2 {
                continue;
            }

            let dy = rand::random_range(-1i32..=1);
            if self.random_teleport(
                &world,
                owner_pos.x + f64::from(dx) + 0.5,
                owner_pos.y + f64::from(dy),
                owner_pos.z + f64::from(dz) + 0.5,
                false,
            ) {
                return;
            }
        }
    }

    fn wants_to_attack(&self, _target: &SharedEntity, _owner: &SharedEntity) -> bool {
        // Vanilla `TamableAnimal.wantsToAttack` returns true unconditionally.
        true
    }
}

impl Entity for ZombieNautilusEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
        abstract_nautilus::tick(&*self);
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

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    /// Vanilla `AbstractNautilus.playStepSound` is empty.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: steel_utils::BlockStateId) {}

    /// Vanilla `ZombieNautilus.getSwimSound`.
    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_ZOMBIE_NAUTILUS_SWIM
    }

    /// Vanilla `AbstractNautilus.isPushedByFluid`.
    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn can_add_passenger(&self, _passenger: &dyn Entity) -> bool {
        abstract_nautilus::can_add_passenger(self)
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        abstract_nautilus::controlling_passenger(self)
    }

    /// Vanilla `AbstractNautilus.hurtServer` anger reaction.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = LivingEntity::hurt_server(self, world, source, amount);
        if was_hurt && let Some(attacker) = source.causing_entity() {
            brain::set_anger_target(world, self, attacker);
        }

        was_hurt
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(variant) = item_stack.get(ZOMBIE_NAUTILUS_VARIANT) {
            self.set_variant(*variant);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        abstract_nautilus::save_additional(self, nbt);
        // Vanilla `VariantUtils.writeVariant`.
        nbt.insert("variant", self.variant().value().key.to_string().as_str());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        abstract_nautilus::load_additional(self, nbt);
        // Vanilla `VariantUtils.readVariant`.
        if let Some(variant) = nbt
            .string("variant")
            .and_then(|value| Identifier::from_str(value.to_str().as_ref()).ok())
            .and_then(|key| REGISTRY.zombie_nautilus_variants.by_key(&key))
        {
            self.set_variant(RegistryReference::new(variant));
        }
    }
}

impl LivingEntity for ZombieNautilusEntity {
    fn equip_sound(&self, slot: EquipmentSlot, stack: &ItemStack) -> Option<SoundEventRef> {
        abstract_nautilus::equip_sound(self, slot, stack)
    }

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
        Some(self.hurt_sound_for())
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(self.death_sound_for())
    }

    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        abstract_nautilus::can_use_slot(self, slot)
    }

    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        abstract_nautilus::can_dispenser_equip_into_slot(self, slot)
    }

    fn can_be_affected(&self, effect: &MobEffectInstance) -> bool {
        abstract_nautilus::can_be_affected(self, effect)
    }

    fn ridden_input(&self, controller: &Player, _self_input: DVec3) -> DVec3 {
        abstract_nautilus::ridden_input(controller)
    }

    fn tick_ridden(&self, controller: &Player, _ridden_input: DVec3) {
        abstract_nautilus::tick_ridden(self, controller);
    }

    fn ridden_speed(&self, _controller: &Player) -> f32 {
        abstract_nautilus::ridden_speed(self)
    }

    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        _base_gravity: f64,
        _is_falling: bool,
        _old_y: f64,
    ) -> Option<MoveResult> {
        abstract_nautilus::travel_in_water(entity, input)
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

impl AgeableMob for ZombieNautilusEntity {
    /// Vanilla `ZombieNautilus.getBreedOffspring` returns `null`.
    fn get_breed_offspring(
        &self,
        _world: &Arc<World>,
        _partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        None
    }

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

    /// Vanilla `ZombieNautilus.canBeABaby()` is false, so `isBaby()` is always
    /// false regardless of the stored age.
    fn is_baby(&self) -> bool {
        false
    }

    /// Vanilla `AgeableMob.setBaby` does nothing when `canBeABaby()` is false.
    fn set_baby(&self, _baby: bool) {}

    fn age_boundary_changed(&self, _baby: bool) {
        self.refresh_dimensions();
    }
}

impl Animal for ZombieNautilusEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        abstract_nautilus::is_food(self, item_stack)
    }

    /// Vanilla `ZombieNautilus.playEatingSound`.
    fn play_eating_sound(&self) {
        self.make_sound(Some(&sound_events::ENTITY_ZOMBIE_NAUTILUS_EAT));
    }
}

impl Mob for ZombieNautilusEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `ZombieNautilus.customServerAiStep`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        {
            let mut brain = self.brain.lock();
            brain::tick(&mut brain, &world, entity);
        }
        abstract_nautilus::check_restriction(self);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(self.ambient_sound_for())
    }

    /// Vanilla `ZombieNautilus.sunProtectionSlot`.
    fn sun_protection_slot(&self) -> EquipmentSlot {
        EquipmentSlot::Body
    }

    /// Vanilla `ZombieNautilus.canBeLeashed`.
    fn mob_can_be_leashed(&self) -> bool {
        !self.is_aggravated() && !self.is_mob_controlled()
    }

    fn requires_custom_persistence(&self) -> bool {
        abstract_nautilus::requires_custom_persistence(self)
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        abstract_nautilus::remove_when_far_away()
    }

    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        abstract_nautilus::check_spawn_obstruction(self, world)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `ZombieNautilus.finalizeSpawn` -> `VariantUtils.selectVariantToSpawn`.
        if let Some(variant) = select_variant_to_spawn(world, self.block_position()) {
            self.set_variant(RegistryReference::new(variant));
        }

        // Vanilla `AbstractNautilus.finalizeSpawn` -> `NautilusAi.initMemories`.
        brain::init_memories(&mut self.brain.lock());
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        abstract_nautilus::mob_interact(self, player, hand)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for ZombieNautilusEntity {
    /// Vanilla `AbstractNautilus.getWalkTargetValue` treats every position as neutral.
    fn get_walk_target_value(&self, _pos: BlockPos) -> f32 {
        0.0
    }
}
