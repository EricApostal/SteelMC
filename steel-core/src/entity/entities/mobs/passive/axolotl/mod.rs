//! Vanilla `Axolotl`: a water-dwelling passive mob with five color variants,
//! bucket capture, a rare blue variant and the play-dead defense.
//!
//! Scope: the entity shell (variant/playing-dead/from-bucket synced state,
//! bucket capture, breeding variant inheritance, dry-out air handling, sounds),
//! the `PlayDead`/`ValidatePlayDead` behaviors and the `AxolotlAi` brain
//! provider in [`brain`]. The amphibious navigation/move-control, the
//! `travelInWater` override, the hunting/attack goals that vanilla reaches
//! through its melee/attack-target behaviors and the water-pathfinding malus
//! are listed as gaps in the wave report.

pub(crate) mod brain;
pub(crate) mod play_dead;
pub(crate) use play_dead::{PlayDead, ValidatePlayDead};

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::components::CustomData;
use steel_registry::data_components::vanilla_components::{
    AXOLOTL_VARIANT, BUCKET_ENTITY_DATA, CUSTOM_NAME,
};
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::entity_variant::AxolotlVariant;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::AxolotlEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes, vanilla_damage_types,
    vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

use crate::behavior::InteractionResult;
use crate::behavior::advancement_hooks::raise_filled_bucket;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::water::abstract_fish::check_water_animal_spawn_obstruction;
use crate::entity::leash::Leashable;
use crate::entity::{
    AgeableMob, AgeableMobBase, AgeableMobGroupData, Animal, AnimalBase, Entity, EntityBase,
    EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, RemovalReason, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::World;

/// Vanilla `Axolotl.TOTAL_PLAYDEAD_TIME`.
const TOTAL_PLAYDEAD_TIME: i32 = 200;
/// Vanilla `Axolotl.RARE_VARIANT_CHANCE`: 1 in 1200.
const RARE_VARIANT_CHANCE: i32 = 1200;
/// Vanilla `Axolotl.AXOLOTL_TOTAL_AIR_SUPPLY`.
const AXOLOTL_TOTAL_AIR_SUPPLY: i32 = 6000;
/// Vanilla `Axolotl.REHYDRATE_AIR_SUPPLY`.
const REHYDRATE_AIR_SUPPLY: i32 = 1800;
/// Vanilla `Axolotl` dry-out damage per drowning tick.
const DRY_OUT_DAMAGE: f32 = 2.0;
/// Vanilla `Axolotl.createAttributes` step height (registry value fallback).
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

const AXOLOTL_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.1875, 0.0)];
/// Vanilla `Axolotl.BABY_DIMENSIONS`.
const AXOLOTL_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    0.375,
    0.21,
    0.09375,
    EntityAttachments::new(&AXOLOTL_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);

#[entity_behavior(class = "Axolotl")]
/// Vanilla axolotl with variant, play-dead and bucket state.
pub struct AxolotlEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<AxolotlEntityData>,
    /// Vanilla `Mob.brain`; the provider is wired in `axolotl/brain.rs`.
    brain: SyncMutex<Brain>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `AxolotlEntity`.
unsafe impl DowncastType for AxolotlEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/axolotl");
}

impl AxolotlEntity {
    /// Creates a new axolotl at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs an axolotl from persisted base entity state.
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
        let mut entity_data = AxolotlEntityData::new();
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
        };
        // Vanilla `Axolotl.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        *entity.brain.lock() = brain::make_brain(&entity);
        entity
    }

    /// Sets the active axolotl variant (vanilla `Axolotl.setVariant`).
    pub fn set_variant(&self, variant: AxolotlVariant) {
        self.entity_data.lock().variant.set(variant.id());
    }

    /// Returns the active axolotl variant (vanilla `Axolotl.getVariant`).
    #[must_use]
    pub fn variant(&self) -> AxolotlVariant {
        AxolotlVariant::by_id(*self.entity_data.lock().variant.get())
    }

    /// Vanilla `Axolotl.setPlayingDead`.
    pub fn set_playing_dead(&self, playing_dead: bool) {
        self.entity_data.lock().playing_dead.set(playing_dead);
    }

    /// Vanilla `Axolotl.isPlayingDead`.
    #[must_use]
    pub fn is_playing_dead(&self) -> bool {
        *self.entity_data.lock().playing_dead.get()
    }

    /// Vanilla `Axolotl.fromBucket`.
    #[must_use]
    #[expect(
        clippy::wrong_self_convention,
        reason = "vanilla `Axolotl.fromBucket` getter; the name is shared with the other bucketable fish entities"
    )]
    pub fn from_bucket(&self) -> bool {
        *self.entity_data.lock().from_bucket.get()
    }

    /// Vanilla `Axolotl.setFromBucket`.
    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.entity_data.lock().from_bucket.set(from_bucket);
    }

    /// Vanilla `Axolotl.rehydrate`: restore up to 1800 air supply.
    pub fn rehydrate(&self) {
        let air_supply = (self.air_supply() + REHYDRATE_AIR_SUPPLY).min(self.max_air_supply());
        self.set_air_supply(air_supply);
    }

    /// Vanilla `Axolotl.useRareVariant`: 1 in 1200.
    fn use_rare_variant() -> bool {
        rand::random_range(0..RARE_VARIANT_CHANCE) == 0
    }

    /// Vanilla `Axolotl.Variant.getCommonSpawnVariant`.
    fn random_common_variant() -> AxolotlVariant {
        const COMMON: [AxolotlVariant; 4] = [
            AxolotlVariant::Lucy,
            AxolotlVariant::Wild,
            AxolotlVariant::Gold,
            AxolotlVariant::Cyan,
        ];
        COMMON[rand::random_range(0..COMMON.len())]
    }

    /// Vanilla `Axolotl.Variant.getRareSpawnVariant`: only `Blue` is rare.
    const fn rare_variant() -> AxolotlVariant {
        AxolotlVariant::Blue
    }

    /// Vanilla `Axolotl.isInWaterOrRain`: in water, or rained on at either the
    /// feet or the top of the hitbox.
    fn is_in_water_or_rain(&self) -> bool {
        if self.is_in_water() {
            return true;
        }
        let Some(world) = self.level() else {
            return false;
        };

        let pos = self.block_position();
        world.is_raining_at(pos)
            || world.is_raining_at(BlockPos::new(
                pos.x(),
                self.bounding_box().max_y().floor() as i32,
                pos.z(),
            ))
    }

    /// Vanilla `Axolotl.handleAirSupply`: breathe out of water, dry out when
    /// the air runs out, and refill instantly while wet.
    fn handle_air_supply(&self) {
        if !LivingEntity::is_alive(self) {
            return;
        }

        if self.is_in_water_or_rain() {
            self.set_air_supply(self.max_air_supply());
            return;
        }

        self.set_air_supply(self.air_supply() - 1);
        if self.should_take_drowning_damage() {
            self.set_air_supply(0);
            if let Some(world) = self.level() {
                self.hurt(
                    &world,
                    &DamageSource::environment(&vanilla_damage_types::DRY_OUT),
                    DRY_OUT_DAMAGE,
                );
            }
        }
    }

    /// Vanilla `Axolotl.saveToBucketTag`: build the axolotl bucket stack with the
    /// variant component and the bucket entity data.
    fn create_bucket_stack(&self) -> ItemStack {
        let mut stack = ItemStack::new(&vanilla_items::AXOLOTL_BUCKET);
        stack.set(AXOLOTL_VARIANT, self.variant());

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
        tag.insert("Age", self.get_age());
        tag.insert("AgeLocked", i8::from(self.is_age_locked()));
        // Vanilla also stores the brain's `HAS_HUNTING_COOLDOWN` expiry; the
        // memory is registered here but only written by the AI provider.
        if let Some(custom_data) = CustomData::try_from_compound(tag) {
            stack.set(BUCKET_ENTITY_DATA, custom_data);
        }
        if let Some(custom_name) = self.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }

        stack
    }

    /// Vanilla `Axolotl.loadFromBucketTag` / `Bucketable.loadDefaultDataFromBucketTag`.
    ///
    /// `MobBucketItem` currently only applies generic item components, so the
    /// bucket's entity data is not fed back here yet (see the wave report).
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
        self.set_age(nbt.int("Age").unwrap_or(0));
        self.set_age_locked(nbt.byte("AgeLocked").is_some_and(|value| value != 0));
    }

    /// Vanilla `Axolotl.bucketMobPickup`: a water bucket captures this axolotl.
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

        self.play_sound(&sound_events::ITEM_BUCKET_FILL_AXOLOTL, 1.0, 1.0);
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

    /// Vanilla `Axolotl.checkAxolotlSpawnRules`, the `SpawnPlacements` predicate for
    /// `EntityType.AXOLOTL`.
    ///
    /// Vanilla ignores `entity_type`, `spawnReason` and `random`; the block below
    /// must carry `#minecraft:axolotls_spawnable_on`.
    #[must_use]
    pub fn check_axolotl_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut RandomSource,
    ) -> bool {
        level
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&BlockTag::AXOLOTLS_SPAWNABLE_ON)
    }
}

/// Vanilla `Axolotl.Variant` identifiers, used for Steel's breed-variant hook.
fn variant_key(variant: AxolotlVariant) -> &'static Identifier {
    static LUCY: Identifier = Identifier::vanilla_static("lucy");
    static WILD: Identifier = Identifier::vanilla_static("wild");
    static GOLD: Identifier = Identifier::vanilla_static("gold");
    static CYAN: Identifier = Identifier::vanilla_static("cyan");
    static BLUE: Identifier = Identifier::vanilla_static("blue");

    match variant {
        AxolotlVariant::Lucy => &LUCY,
        AxolotlVariant::Wild => &WILD,
        AxolotlVariant::Gold => &GOLD,
        AxolotlVariant::Cyan => &CYAN,
        AxolotlVariant::Blue => &BLUE,
    }
}

impl Entity for AxolotlEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    /// Vanilla `Axolotl.get(DataComponents.AXOLOTL_VARIANT)`: the bucket's
    /// variant component is applied implicitly when an axolotl bucket spawns.
    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(variant) = item_stack.get(AXOLOTL_VARIANT) {
            self.set_variant(*variant);
        }
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            AXOLOTL_BABY_DIMENSIONS.scale(scale)
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

    fn max_air_supply(&self) -> i32 {
        AXOLOTL_TOTAL_AIR_SUPPLY
    }

    /// Vanilla `Axolotl` keeps the generic block step sound.
    fn play_step_sound(&self, _pos: BlockPos, block_state: BlockStateId) {
        self.play_block_step_sound(block_state);
    }

    /// Vanilla `Axolotl.getSwimSound`.
    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_AXOLOTL_SWIM
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    /// Vanilla `Axolotl.isPushedByFluid`: axolotls are not pushed by currents.
    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);

        // Vanilla `Axolotl.Variant.LEGACY_CODEC` stores the variant id as an int.
        nbt.insert("Variant", self.variant().id());
        nbt.insert("FromBucket", i8::from(self.from_bucket()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);

        self.set_variant(AxolotlVariant::by_id(nbt.int("Variant").unwrap_or(0)));
        self.set_from_bucket(nbt.byte("FromBucket").is_some_and(|value| value != 0));
    }

    /// Vanilla `Axolotl.hurtServer`: a hit in water can start the play-dead
    /// defense, then the damage is applied normally.
    ///
    /// Steel hooks this on `Entity::hurt` rather than
    /// `LivingEntity::hurt_server`: that method has no `default_*` companion,
    /// and `LivingEntity::hurt_server(self, …)` from inside the override would
    /// resolve to itself and recurse forever.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let current_health = self.get_health();
        if !self.is_no_ai()
            && rand::random_range(0..3) == 0
            && ((rand::random_range(0..3) as f32) < amount
                || current_health / self.get_max_health() < 0.5)
            && amount < current_health
            && self.is_in_water()
            && (source.causing_entity().is_some() || source.direct_entity().is_some())
            && !self.is_playing_dead()
        {
            self.brain
                .lock()
                .set_memory(&mem::PLAY_DEAD_TICKS, Some(TOTAL_PLAYDEAD_TIME));
        }

        LivingEntity::hurt_server(self, world, source, amount)
    }
}

impl LivingEntity for AxolotlEntity {
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

    /// Vanilla `Axolotl` does not override `LivingEntity.getSoundVolume`.
    fn sound_volume(&self) -> f32 {
        1.0
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_AXOLOTL_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_AXOLOTL_DEATH)
    }

    /// Vanilla `Axolotl.canBeSeenAsEnemy`: a playing-dead axolotl is ignored.
    /// Inlined `LivingEntity` default (`!invulnerable && !spectator && alive`):
    /// `LivingEntity::can_be_seen_as_enemy(self, …)` from inside this override
    /// would resolve to this method and recurse.
    fn can_be_seen_as_enemy(&self) -> bool {
        !self.is_playing_dead() && !self.is_invulnerable() && self.can_be_seen_by_anyone()
    }

    /// Vanilla `Axolotl.baseTick` -> `handleAirSupply` replaces the generic
    /// eye-in-water drowning model with out-of-water drying out.
    fn tick_living_environmental_damage(&self) {
        if !LivingEntity::is_alive(self) {
            return;
        }

        if LivingEntity::is_in_wall(self) {
            self.tick_in_wall_damage();
        }
        // Vanilla's border damage only applies to players.

        self.handle_air_supply();
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

impl AgeableMob for AxolotlEntity {
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
        Some(variant_key(self.variant()))
    }

    fn set_breed_variant_key(&self, key: &Identifier) -> bool {
        let Some(variant) = AxolotlVariant::from_serialized_name(key.path.as_ref()) else {
            return false;
        };
        self.set_variant(variant);
        true
    }

    /// Vanilla `Axolotl.getBreedOffspring` variant roll: 1 in 1200 blue, else
    /// one of the parents' variants.
    fn initialize_breed_offspring(&self, partner: &dyn AgeableMob, offspring: &dyn AgeableMob) {
        let variant_key = if AxolotlEntity::use_rare_variant() {
            variant_key(AxolotlEntity::rare_variant())
        } else if rand::random::<bool>() {
            variant_key(self.variant())
        } else {
            partner
                .breed_variant_key()
                .unwrap_or_else(|| variant_key(self.variant()))
        };

        if !offspring.set_breed_variant_key(variant_key) {
            log::error!("axolotl offspring could not inherit breeding variant {variant_key}");
        }
        // Vanilla `Axolotl.getBreedOffspring` marks the baby persistent.
        offspring.set_persistence_required();
    }
}

impl Animal for AxolotlEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Axolotl.isFood` uses `ItemTags.AXOLOTL_FOOD`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::AXOLOTL_FOOD)
    }
}

impl Mob for AxolotlEntity {
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

    /// Vanilla `Axolotl.customServerAiStep`: tick `AxolotlAi` (which includes
    /// `AxolotlAi.updateActivity`) and mirror `PLAY_DEAD_TICKS` into the synced
    /// playing-dead flag.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
        brain::update_playing_dead(&brain, self);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Axolotl.playAmbientSound`: silent while playing dead.
    fn play_ambient_sound(&self) {
        if self.is_playing_dead() {
            return;
        }
        self.make_sound(self.ambient_sound());
    }

    /// Vanilla `Axolotl.getAmbientSound`: underwater vs. on land.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_in_water() {
            &sound_events::ENTITY_AXOLOTL_IDLE_WATER
        } else {
            &sound_events::ENTITY_AXOLOTL_IDLE_AIR
        })
    }

    /// Vanilla `Axolotl.playAttackSound`.
    fn play_attack_sound(&self) {
        self.play_sound(&sound_events::ENTITY_AXOLOTL_ATTACK, 1.0, 1.0);
    }

    /// Vanilla `Axolotl.getMaxHeadXRot`.
    fn max_head_x_rot(&self) -> f32 {
        1.0
    }

    /// Vanilla `Axolotl.getMaxHeadYRot`.
    fn max_head_y_rot(&self) -> f32 {
        1.0
    }

    /// Vanilla `Axolotl.canBeLeashed`.
    fn mob_can_be_leashed(&self) -> bool {
        true
    }

    /// Vanilla `Axolotl.requiresCustomPersistence`.
    fn requires_custom_persistence(&self) -> bool {
        (self.is_passenger() || self.as_leashable().is_some_and(Leashable::is_leashed))
            || self.from_bucket()
    }

    /// Vanilla `Axolotl.removeWhenFarAway`.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        !self.from_bucket() && self.custom_name().is_none()
    }

    /// Vanilla `Axolotl.checkSpawnObstruction` is `level.isUnobstructed(this)`; the
    /// shared `Mob` default would reject the water the axolotl spawns in.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        check_water_animal_spawn_obstruction(world, self)
    }

    /// Vanilla `Axolotl.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if spawn_reason == EntitySpawnReason::Bucket {
            return group_data;
        }

        let is_baby = matches!(
            &group_data,
            Some(SpawnGroupData::AgeableMob(group)) if group.group_size() >= 2
        );

        // GAP(axolotl-group-data): vanilla `AxolotlGroupData` carries the two
        // candidate variants so a spawning group shares them. Steel's
        // `SpawnGroupData` has no variant slot, so each axolotl rolls its own
        // common spawn variant (same simplification as the llama family).
        self.set_variant(AxolotlEntity::random_common_variant());
        if is_baby {
            self.set_age(self.get_baby_start_age());
        }

        // Vanilla creates `AxolotlGroupData`, which disables the generic baby
        // roll (`super(false)`); the baby case is handled above.
        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::AgeableMob(
                AgeableMobGroupData::with_should_spawn_baby(false),
            ))
        });
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    /// Vanilla `Axolotl.mobInteract`: bucket capture, then animal feeding.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        if self.try_bucket_pickup(player, hand) {
            return InteractionResult::Success;
        }

        Animal::mob_interact_animal(self, player, hand)
    }

    /// Vanilla `Axolotl.usePlayerItem`: a tropical fish bucket is emptied into a
    /// water bucket instead of being consumed.
    fn use_player_item(&self, player: &Player, hand: InteractionHand) {
        let is_tropical_fish_bucket = {
            let inventory = player.inventory.lock();
            inventory
                .get_item_in_hand(hand)
                .is(&vanilla_items::TROPICAL_FISH_BUCKET)
        };
        if !is_tropical_fish_bucket {
            player.inventory.lock().shrink_item_in_hand(hand, 1);
            return;
        }

        let overflow = {
            let mut inventory = player.inventory.lock();
            inventory.apply_filled_result(
                hand,
                ItemStack::new(&vanilla_items::WATER_BUCKET),
                player.has_infinite_materials(),
                false,
            )
        };
        if !overflow.is_empty() {
            let _ = player.drop_item(overflow, false, Prediction::Predicted);
        }
    }
}

impl PathfinderMob for AxolotlEntity {
    /// Vanilla `Axolotl.getWalkTargetValue` treats every position as neutral.
    fn get_walk_target_value(&self, _pos: BlockPos) -> f32 {
        0.0
    }
}
