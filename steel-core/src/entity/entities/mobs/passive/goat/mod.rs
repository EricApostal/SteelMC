//! Vanilla `Goat`: a horned climber that rams, long-jumps and can be milked.
//!
//! Mirrors `net.minecraft.world.entity.animal.goat.Goat`. Vanilla registers **no**
//! goals (`Goat.registerGoals()` is empty) and drives goats from `GoatAi`'s
//! `CORE`/`IDLE`/`LONG_JUMP`/`RAM` packages; the brain provider lives in
//! [`brain`] and `Goat.customServerAiStep` is the `brain_tick` hook.
//!
//! ## Deviations
//!
//! - `Goat.handleEntityEvent(58/59)` (lower-head start/stop) has no Steel hook:
//!   `Entity` exposes no entity-event receiver yet, so [`GoatEntity`] keeps the
//!   vanilla `isLoweringHead`/`lowerHeadTick` state and ticks it exactly like
//!   `Goat.aiStep`, but nothing currently raises the flag.
//! - `Goat.calculateFallDamage`'s `GOAT_FALL_DAMAGE_REDUCTION` of 10 damage has no
//!   hook on the fall-damage path, which reads the attributes directly.
//! - `RamTarget` carries no `getKnockbackForce`/impact-sound selectors, so the
//!   goat's screaming/baby variants are resolved once when the brain is built
//!   (vanilla re-evaluates per ram). A goat that grows up keeps the baby knockback
//!   until its brain is rebuilt.
//! - GAP(goat-horn-break): `RamTarget` does not call `Goat.dropHorn` on a
//!   horn-breaking block; [`GoatEntity::drop_horn`] is implemented for the
//!   interaction/consumption path but nothing invokes it yet.

mod brain;

use std::f32::consts::PI;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::components::InstrumentComponent;
use steel_registry::data_components::vanilla_components::INSTRUMENT;
use steel_registry::entity_data::EntityPose;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::registry::RegistryHolder;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::GoatEntityData;
use steel_registry::vanilla_instrument_tags::InstrumentTag;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_entities, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::{LevelReader, World};

/// Vanilla `Goat.LONG_JUMPING_DIMENSION_SCALE_FACTOR`.
const LONG_JUMPING_DIMENSION_SCALE_FACTOR: f32 = 0.7;
/// Vanilla `Goat.BABY_SCALE`.
const BABY_SCALE: f32 = 0.55;
/// Vanilla `Goat.ADULT_ATTACK_DAMAGE`.
const ADULT_ATTACK_DAMAGE: f64 = 2.0;
/// Vanilla `Goat.BABY_ATTACK_DAMAGE`.
const BABY_ATTACK_DAMAGE: f64 = 1.0;
/// Vanilla `Goat.MAX_ADDED_RAMMING_X_HEAD_ROT`.
const MAX_ADDED_RAMMING_X_HEAD_ROT: f32 = 30.0;
/// Vanilla `Goat.BABY_DEFAULT_X_HEAD_ROT`.
const BABY_DEFAULT_X_HEAD_ROT: f32 = 22.5;
/// Vanilla `Goat.GOAT_SCREAMING_CHANCE`.
const GOAT_SCREAMING_CHANCE: f64 = 0.02;
/// Vanilla `Goat.UNIHORN_CHANCE`.
const UNIHORN_CHANCE: f32 = 0.1;
/// Vanilla `Goat.getMaxHeadYRot`.
const MAX_HEAD_Y_ROT: f32 = 15.0;
/// Vanilla `Goat.lowerHeadTick` upper bound.
const MAX_LOWER_HEAD_TICK: i32 = 20;
/// Vanilla `Goat.playStepSound` volume.
const STEP_SOUND_VOLUME: f32 = 0.15;

/// Vanilla `Goat.BABY_DIMENSIONS`.
const GOAT_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.531_25, 0.0)];
const GOAT_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    0.45,
    0.65,
    0.593_75,
    EntityAttachments::new(&GOAT_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);

/// Vanilla `Goat` runtime fields that are not synchronized entity data.
#[derive(Debug, Clone, Copy, Default)]
struct GoatState {
    /// Vanilla `Goat.isLoweringHead`.
    is_lowering_head: bool,
    /// Vanilla `Goat.lowerHeadTick`.
    lower_head_tick: i32,
}

#[entity_behavior(class = "Goat")]
/// Vanilla goat with horns, screaming variant, ram/long-jump state and brain.
pub struct GoatEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<GoatEntityData>,
    brain: SyncMutex<Brain>,
    state: SyncMutex<GoatState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `GoatEntity`.
unsafe impl DowncastType for GoatEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/goat");
}

impl GoatEntity {
    /// Creates a new goat at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a goat from persisted base entity state.
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
        {
            // Vanilla `Goat` constructor: powder snow is impassable, and goats may
            // walk on top of it.
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::PowderSnow, -1.0);
            malus.set(PathType::OnTopOfPowderSnow, -1.0);
        }
        let mut entity_data = GoatEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(Brain::default()),
            state: SyncMutex::new(GoatState::default()),
        };
        // Vanilla `Goat.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        *entity.brain.lock() = brain::make_brain(&entity);
        entity
    }

    /// Vanilla `Goat.isScreamingGoat`.
    #[must_use]
    pub fn is_screaming_goat(&self) -> bool {
        *self.entity_data.lock().is_screaming_goat.get()
    }

    /// Vanilla `Goat.setScreamingGoat`.
    pub fn set_screaming_goat(&self, is_screaming_goat: bool) {
        self.entity_data
            .lock()
            .is_screaming_goat
            .set(is_screaming_goat);
    }

    /// Vanilla `Goat.hasLeftHorn`.
    #[must_use]
    pub fn has_left_horn(&self) -> bool {
        *self.entity_data.lock().has_left_horn.get()
    }

    /// Vanilla `Goat.hasRightHorn`.
    #[must_use]
    pub fn has_right_horn(&self) -> bool {
        *self.entity_data.lock().has_right_horn.get()
    }

    /// Sets the synchronized left-horn flag.
    pub fn set_has_left_horn(&self, has_left_horn: bool) {
        self.entity_data.lock().has_left_horn.set(has_left_horn);
    }

    /// Sets the synchronized right-horn flag.
    pub fn set_has_right_horn(&self, has_right_horn: bool) {
        self.entity_data.lock().has_right_horn.set(has_right_horn);
    }

    /// Vanilla `Goat.getMilkingSound`.
    #[must_use]
    pub fn milking_sound(&self) -> SoundEventRef {
        if self.is_screaming_goat() {
            &sound_events::ENTITY_GOAT_SCREAMING_MILK
        } else {
            &sound_events::ENTITY_GOAT_MILK
        }
    }

    /// Vanilla `Goat.createHorn`: a `GOAT_HORN` carrying a random instrument from
    /// the screaming or regular horn tag, seeded from the goat's UUID.
    #[must_use]
    pub fn create_horn(&self) -> ItemStack {
        // Vanilla `this.getUUID().hashCode()`: the XOR of the UUID's two halves.
        let uuid = self.uuid().as_u128();
        let mut random = LegacyRandom::from_seed((uuid as u64) ^ ((uuid >> 64) as u64));
        let tag = if self.is_screaming_goat() {
            &InstrumentTag::SCREAMING_GOAT_HORNS
        } else {
            &InstrumentTag::REGULAR_GOAT_HORNS
        };

        let instruments: Vec<_> = REGISTRY.instruments.iter_tag(tag).collect();
        let mut item_stack = ItemStack::new(&vanilla_items::GOAT_HORN);
        if !instruments.is_empty() {
            let index = random.next_i32_bounded(instruments.len() as i32) as usize;
            item_stack.set(
                INSTRUMENT,
                InstrumentComponent::new(RegistryHolder::reference(instruments[index])),
            );
        }

        item_stack
    }

    /// Vanilla `Goat.dropHorn`: consume one horn and spawn the horn item in front
    /// of the goat. Returns whether a horn was dropped.
    pub fn drop_horn(&self) -> bool {
        if AgeableMob::is_baby(self) {
            return false;
        }

        let has_left = self.has_left_horn();
        let has_right = self.has_right_horn();
        if !has_left && !has_right {
            return false;
        }

        let drop_left = if !has_left {
            false
        } else if !has_right {
            true
        } else {
            rand::random::<bool>()
        };

        if drop_left {
            self.set_has_left_horn(false);
        } else {
            self.set_has_right_horn(false);
        }

        let Some(world) = self.level() else {
            return true;
        };

        let item = self.create_horn();
        let velocity = DVec3::new(
            f64::from(random_between(-0.2, 0.2)),
            f64::from(random_between(0.3, 0.7)),
            f64::from(random_between(-0.2, 0.2)),
        );
        let item_entity = ItemEntity::with_item_and_velocity(
            &vanilla_entities::ITEM,
            next_entity_id(),
            self.position(),
            item,
            velocity,
            Arc::downgrade(&world),
        );
        if let Err(error) = world.try_add_entity(Arc::new(item_entity)) {
            log::error!("failed to spawn goat horn item: {error}");
        }

        true
    }

    /// Vanilla `Goat.getRammingXHeadRot`: radians the head is lowered this tick.
    #[must_use]
    pub fn ramming_x_head_rot(&self) -> f32 {
        let max_ramming_x_head_rot = if AgeableMob::is_baby(self) {
            BABY_DEFAULT_X_HEAD_ROT + MAX_ADDED_RAMMING_X_HEAD_ROT
        } else {
            MAX_ADDED_RAMMING_X_HEAD_ROT
        };
        let lower_head_tick =
            f32::from(i16::try_from(self.state.lock().lower_head_tick).unwrap_or(0));
        lower_head_tick / 20.0 * max_ramming_x_head_rot * (PI / 180.0)
    }

    /// Vanilla `Goat.handleEntityEvent(58)`: start lowering the head.
    pub fn set_lowering_head(&self, is_lowering_head: bool) {
        self.state.lock().is_lowering_head = is_lowering_head;
    }

    /// Vanilla `Goat.aiStep`'s `lowerHeadTick` bookkeeping.
    fn tick_lowering_head(&self) {
        let mut state = self.state.lock();
        if state.is_lowering_head {
            state.lower_head_tick += 1;
        } else {
            state.lower_head_tick -= 2;
        }
        state.lower_head_tick = state.lower_head_tick.clamp(0, MAX_LOWER_HEAD_TICK);
    }

    /// Vanilla `GoatAi.initMemories`: seed the long-jump and ram cooldowns.
    ///
    /// Vanilla `UniformInt.sample` is `min + random.nextInt(max - min)`, so the
    /// upper bound is exclusive; Steel's [`steel_utils::value_providers::UniformIntProvider`]
    /// is inclusive, so the same ranges are sampled with exclusive `rand` ranges.
    pub fn init_ai_memories(&self) {
        let long_jump_cooldown = rand::random_range(
            brain::TIME_BETWEEN_LONG_JUMPS_MIN..brain::TIME_BETWEEN_LONG_JUMPS_MAX,
        );
        let ram_cooldown =
            rand::random_range(brain::TIME_BETWEEN_RAMS_MIN..brain::TIME_BETWEEN_RAMS_MAX);

        let mut brain = self.brain.lock();
        brain.set_memory(
            &module_type::LONG_JUMP_COOLDOWN_TICKS,
            Some(long_jump_cooldown),
        );
        brain.set_memory(&module_type::RAM_COOLDOWN_TICKS, Some(ram_cooldown));
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

    /// Returns whether the stack matches vanilla `ItemTags.GOAT_FOOD`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::GOAT_FOOD)
    }

    /// Vanilla `Goat.mobInteract`'s bucket branch: fill a bucket with milk.
    fn try_milk(&self, player: &Player, hand: InteractionHand) -> bool {
        if AgeableMob::is_baby(self) {
            return false;
        }

        let is_bucket = {
            let inventory = player.inventory.lock();
            inventory.get_item_in_hand(hand).is(&vanilla_items::BUCKET)
        };
        if !is_bucket {
            return false;
        }

        player.play_sound(self.milking_sound(), 1.0, 1.0);

        let overflow = {
            let mut inventory = player.inventory.lock();
            inventory.apply_filled_result(
                hand,
                ItemStack::new(&vanilla_items::MILK_BUCKET),
                player.has_infinite_materials(),
                true,
            )
        };

        if !overflow.is_empty() {
            let _ = player.drop_item(overflow, false, Prediction::Predicted);
        }

        true
    }
}

impl Entity for GoatEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        let dimensions = if AgeableMob::is_baby(self) {
            GOAT_BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        };

        if pose == EntityPose::LongJumping {
            dimensions.scale(LONG_JUMPING_DIMENSION_SCALE_FACTOR)
        } else {
            dimensions
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_GOAT_STEP, STEP_SOUND_VOLUME, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("IsScreamingGoat", i8::from(self.is_screaming_goat()));
        nbt.insert("HasLeftHorn", i8::from(self.has_left_horn()));
        nbt.insert("HasRightHorn", i8::from(self.has_right_horn()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_screaming_goat(nbt.byte("IsScreamingGoat").is_some_and(|value| value != 0));
        self.set_has_left_horn(nbt.byte("HasLeftHorn").is_none_or(|value| value != 0));
        self.set_has_right_horn(nbt.byte("HasRightHorn").is_none_or(|value| value != 0));
    }
}

impl LivingEntity for GoatEntity {
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
        Some(if self.is_screaming_goat() {
            &sound_events::ENTITY_GOAT_SCREAMING_HURT
        } else {
            &sound_events::ENTITY_GOAT_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_screaming_goat() {
            &sound_events::ENTITY_GOAT_SCREAMING_DEATH
        } else {
            &sound_events::ENTITY_GOAT_DEATH
        })
    }

    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            BABY_SCALE
        } else {
            1.0
        }
    }

    fn set_y_head_rot(&self, y_head_rot: f32) {
        // Vanilla `Goat.setYHeadRot`: the head may only yaw 15° from the body.
        let delta_from_body = degrees_difference(self.y_body_rot(), y_head_rot);
        LivingEntityBase::set_y_head_rot(
            &self.living_base,
            self.y_body_rot() + delta_from_body.clamp(-MAX_HEAD_Y_ROT, MAX_HEAD_Y_ROT),
        );
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        // Vanilla `Goat.aiStep` ticks the lower-head animation before `super`.
        self.tick_lowering_head();

        let result = Mob::mob_ai_step(self, entity);

        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }
}

impl AgeableMob for GoatEntity {
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
        // Vanilla `Goat.ageBoundaryReached`: reset the attack damage for the new age.
        let attack_damage = if AgeableMob::is_baby(self) {
            BABY_ATTACK_DAMAGE
        } else {
            ADULT_ATTACK_DAMAGE
        };
        self.attributes()
            .lock()
            .set_base_value(vanilla_attributes::ATTACK_DAMAGE, attack_damage);
        self.refresh_dimensions();
    }

    fn get_breed_offspring(
        &self,
        world: &Arc<World>,
        partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        let offspring = self.create_breed_offspring(world)?;
        let partner_screaming = partner
            .as_entity_event_source()
            .downcast_ref::<GoatEntity>()
            .is_some_and(GoatEntity::is_screaming_goat);
        // Vanilla `Goat.getBreedOffspring`: one random parent's screaming flag,
        // plus the 2% spontaneous screamer.
        let parent_screaming = if rand::random::<bool>() {
            self.is_screaming_goat()
        } else {
            partner_screaming
        };
        let baby_is_screaming = parent_screaming || rand::random::<f64>() < GOAT_SCREAMING_CHANCE;

        if let Some(goat) = offspring.downcast_ref::<GoatEntity>() {
            goat.set_screaming_goat(baby_is_screaming);
            goat.init_ai_memories();
        }

        Some(offspring)
    }
}

impl Animal for GoatEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        GoatEntity::is_food(item_stack)
    }

    fn play_eating_sound(&self) {
        // Vanilla `Goat.playEatingSound`: screaming variant, pitch 0.8..1.2.
        let sound = if self.is_screaming_goat() {
            &sound_events::ENTITY_GOAT_SCREAMING_EAT
        } else {
            &sound_events::ENTITY_GOAT_EAT
        };
        let pitch = random_between(0.8, 1.2);
        self.play_sound(sound, 1.0, pitch);
    }

    /// Vanilla `Goat.checkGoatSpawnRules`.
    fn check_animal_spawn_rules(
        level: &dyn LevelReader,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        level
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&BlockTag::GOATS_SPAWNABLE_ON)
            && Self::is_bright_enough_to_spawn(level, pos)
    }
}

impl Mob for GoatEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `Goat.customServerAiStep`: tick `GoatAi`, which ends with
    /// `GoatAi.updateActivity`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
    }

    fn tick_goal_selectors(&self, _entity: &SharedEntity) {
        // Vanilla `Goat.registerGoals` is empty: the brain owns all goat AI.
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn custom_server_ai_step(&self) {
        Animal::custom_server_ai_step_animal(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_screaming_goat() {
            &sound_events::ENTITY_GOAT_SCREAMING_AMBIENT
        } else {
            &sound_events::ENTITY_GOAT_AMBIENT
        })
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
        // Vanilla `Goat.finalizeSpawn`.
        self.init_ai_memories();
        self.set_screaming_goat(rand::random::<f64>() < GOAT_SCREAMING_CHANCE);
        self.age_boundary_changed(AgeableMob::is_baby(self));
        if !AgeableMob::is_baby(self) && rand::random::<f32>() < UNIHORN_CHANCE {
            if rand::random::<bool>() {
                self.set_has_left_horn(false);
            } else {
                self.set_has_right_horn(false);
            }
        }

        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held_item = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if held_item.is(&vanilla_items::BUCKET)
            && !AgeableMob::is_baby(self)
            && self.try_milk(player, hand)
        {
            return InteractionResult::Success;
        }

        let interaction_result = Animal::mob_interact_animal(self, player, hand);
        if interaction_result.consumes_action() && Self::is_food(&held_item) {
            self.play_eating_sound();
        }

        interaction_result
    }

    fn mob_can_be_leashed(&self) -> bool {
        // Vanilla `Animal.canBeLeashed`.
        !AgeableMob::is_baby(self)
    }
}

impl PathfinderMob for GoatEntity {}

/// Vanilla `Mth.degreesDifference`.
fn degrees_difference(a: f32, b: f32) -> f32 {
    let difference = (b - a) % 360.0;
    if difference >= 180.0 {
        difference - 360.0
    } else if difference < -180.0 {
        difference + 360.0
    } else {
        difference
    }
}

/// Vanilla `Mth.randomBetween`.
fn random_between(min: f32, max: f32) -> f32 {
    min + rand::random::<f32>() * (max - min)
}
