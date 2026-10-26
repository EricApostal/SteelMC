//! Vanilla `Pufferfish` (`net.minecraft.world.entity.animal.fish.Pufferfish`).
//!
//! An `AbstractFish` that inflates when scared and stings everything it touches. The
//! shared water behaviour lives in [`super::abstract_fish`].

use std::sync::{Arc, LazyLock, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::{CGameEvent, GameEventType, SoundSource};
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PufferfishEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt, sound_events};
use steel_registry::{
    vanilla_attributes, vanilla_damage_types, vanilla_items, vanilla_mob_effects,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::{GameType, InteractionHand};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{Goal, GoalControls};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, MobEffectInstance, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

use super::abstract_fish;

/// Vanilla `Pufferfish.STATE_SMALL`.
pub const STATE_SMALL: i32 = 0;
/// Vanilla `Pufferfish.STATE_MID`.
pub const STATE_MID: i32 = 1;
/// Vanilla `Pufferfish.STATE_FULL`.
pub const STATE_FULL: i32 = 2;
/// Vanilla `Pufferfish` max `PuffState` read back from disk.
const MAX_PUFF_STATE: i32 = 2;
/// Vanilla `Pufferfish.tick` inflation threshold for the second stage.
const INFLATE_FULL_THRESHOLD: i32 = 40;
/// Vanilla `Pufferfish.tick` deflation thresholds.
const DEFLATE_MID_THRESHOLD: i32 = 60;
const DEFLATE_SMALL_THRESHOLD: i32 = 100;
/// Vanilla `Pufferfish.aiStep` touch range and poison duration multiplier.
const TOUCH_INFLATION: f64 = 0.3;
const POISON_DURATION_PER_STATE: i32 = 60;
/// Vanilla `Mob` default step height used when the attribute is missing.
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Pufferfish.touch` sting sound volume/pitch.
const STING_SOUND_VOLUME: f32 = 1.0;
const STING_SOUND_PITCH: f32 = 1.0;

/// Vanilla `Pufferfish.SCARY_MOB`: creative players and
/// `#minecraft:not_scary_for_pufferfish` entities are ignored.
fn scary_mob(target: &dyn LivingEntity, _world: &World) -> bool {
    if target
        .as_player()
        .is_some_and(|player| player.game_mode() == GameType::Creative)
    {
        return false;
    }
    !REGISTRY.entity_types.is_in_tag(
        target.entity_type(),
        &EntityTypeTag::NOT_SCARY_FOR_PUFFERFISH,
    )
}

/// Vanilla `Pufferfish.TARGETING_CONDITIONS`.
static TARGETING_CONDITIONS: LazyLock<TargetingConditions> = LazyLock::new(|| {
    TargetingConditions::for_non_combat()
        .ignore_invisibility_testing()
        .ignore_line_of_sight()
        .selector(scary_mob)
});

#[entity_behavior(class = "Pufferfish")]
/// Vanilla pufferfish: a bucketable fish that inflates and poisons on contact.
pub struct PufferfishEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<PufferfishEntityData>,
    /// Vanilla `Pufferfish.inflateCounter`.
    inflate_counter: SyncMutex<i32>,
    /// Vanilla `Pufferfish.deflateTimer`.
    deflate_timer: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PufferfishEntity`.
unsafe impl DowncastType for PufferfishEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/pufferfish");
}

impl PufferfishEntity {
    /// Creates a new pufferfish at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a pufferfish from persisted base entity state.
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
        let mut entity_data = PufferfishEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        abstract_fish::register_abstract_fish_goals(&mob_base);
        // Vanilla `Pufferfish.registerGoals`.
        mob_base
            .goal_selector()
            .lock()
            .add_goal(1, PufferfishPuffGoal);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            inflate_counter: SyncMutex::new(0),
            deflate_timer: SyncMutex::new(0),
        };
        // Vanilla `Pufferfish` constructor: `refreshDimensions()`.
        entity.refresh_dimensions();
        entity
    }

    /// Vanilla `Pufferfish.getPuffState`.
    #[must_use]
    pub fn puff_state(&self) -> i32 {
        *self.entity_data.lock().puff_state.get()
    }

    /// Vanilla `Pufferfish.setPuffState`.
    ///
    /// Vanilla refreshes dimensions from `onSyncedDataUpdated`; Steel has no synced-data
    /// callback, so the setter refreshes them directly.
    pub fn set_puff_state(&self, state: i32) {
        self.entity_data.lock().puff_state.set(state);
        self.refresh_dimensions();
    }

    /// Vanilla `Pufferfish.inflateCounter`.
    #[must_use]
    fn inflate_counter(&self) -> i32 {
        *self.inflate_counter.lock()
    }

    fn set_inflate_counter(&self, value: i32) {
        *self.inflate_counter.lock() = value;
    }

    /// Vanilla `Pufferfish.deflateTimer`.
    #[must_use]
    fn deflate_timer(&self) -> i32 {
        *self.deflate_timer.lock()
    }

    fn set_deflate_timer(&self, value: i32) {
        *self.deflate_timer.lock() = value;
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

    /// Vanilla `Pufferfish.getBucketItemStack`.
    #[must_use]
    pub fn bucket_item_stack() -> ItemStack {
        ItemStack::new(&vanilla_items::PUFFERFISH_BUCKET)
    }

    /// Vanilla `AbstractFish.getPickupSound`.
    #[must_use]
    pub fn pickup_sound() -> SoundEventRef {
        &sound_events::ITEM_BUCKET_FILL_FISH
    }

    /// Vanilla `Pufferfish.saveToBucketTag` (`Bucketable.saveDefaultDataToBucketTag`).
    fn create_bucket_stack(&self) -> ItemStack {
        let mut stack = Self::bucket_item_stack();
        abstract_fish::save_default_bucket_data(self, &mut stack);
        stack
    }

    /// Vanilla `Pufferfish.loadFromBucketTag` (`Bucketable.loadDefaultDataFromBucketTag`).
    pub fn load_from_bucket_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        abstract_fish::load_default_bucket_data(self, nbt);
    }

    /// Vanilla `AbstractFish.mobInteract`'s bucket half (`Bucketable.bucketMobPickup`).
    fn try_bucket_pickup(&self, player: &Player, hand: InteractionHand) -> bool {
        abstract_fish::try_bucket_pickup(self, player, hand, Self::pickup_sound(), || {
            self.create_bucket_stack()
        })
    }

    /// Vanilla `Pufferfish.tick`'s inflate/deflate state machine.
    ///
    /// Vanilla gates on `!level.isClientSide`; Steel entities are server-side only.
    fn tick_puff_state(&self) {
        if !Entity::is_alive(self) || !self.is_effective_ai() {
            return;
        }

        if self.inflate_counter() > 0 {
            if self.puff_state() == STATE_SMALL {
                self.make_sound(Some(&sound_events::ENTITY_PUFFER_FISH_BLOW_UP));
                self.set_puff_state(STATE_MID);
            } else if self.inflate_counter() > INFLATE_FULL_THRESHOLD
                && self.puff_state() == STATE_MID
            {
                self.make_sound(Some(&sound_events::ENTITY_PUFFER_FISH_BLOW_UP));
                self.set_puff_state(STATE_FULL);
            }

            self.set_inflate_counter(self.inflate_counter() + 1);
        } else if self.puff_state() != STATE_SMALL {
            if self.deflate_timer() > DEFLATE_MID_THRESHOLD && self.puff_state() == STATE_FULL {
                self.make_sound(Some(&sound_events::ENTITY_PUFFER_FISH_BLOW_OUT));
                self.set_puff_state(STATE_MID);
            } else if self.deflate_timer() > DEFLATE_SMALL_THRESHOLD
                && self.puff_state() == STATE_MID
            {
                self.make_sound(Some(&sound_events::ENTITY_PUFFER_FISH_BLOW_OUT));
                self.set_puff_state(STATE_SMALL);
            }

            self.set_deflate_timer(self.deflate_timer() + 1);
        }
    }

    /// Vanilla `Pufferfish.touch`: sting a mob and poison it.
    ///
    /// Deviation: vanilla gates on `doTeamsAllowDamage`, which needs the scoreboard
    /// teams Steel does not have; `is_allied_to` is the closest equivalent.
    fn touch(&self, world: &World, mob: &SharedEntity, attacker: &SharedEntity) {
        if self.is_allied_to(mob.as_ref()) {
            return;
        }

        let puff_state = self.puff_state();
        let source = DamageSource::environment(&vanilla_damage_types::MOB_ATTACK)
            .with_causing_entity(Arc::clone(attacker))
            .with_direct_entity(Arc::clone(attacker));
        let Some(living) = mob.as_living_entity() else {
            return;
        };
        if living.hurt_server(world, &source, (1 + puff_state) as f32) {
            living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::POISON,
                POISON_DURATION_PER_STATE * puff_state,
                0,
            ));
            self.play_sound(
                &sound_events::ENTITY_PUFFER_FISH_STING,
                STING_SOUND_VOLUME,
                STING_SOUND_PITCH,
            );
        }
    }

    /// Vanilla `Pufferfish.getScale`.
    const fn puff_scale(state: i32) -> f32 {
        match state {
            STATE_SMALL => 0.5,
            STATE_MID => 0.7,
            _ => 1.0,
        }
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

impl Entity for PufferfishEntity {
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

    /// Vanilla `Pufferfish.tick`: the puff machine, then `super.tick()`.
    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_puff_state();
        self.tick_living_entity(&entity);
    }

    /// Vanilla `Pufferfish.getDefaultDimensions`: the fish dimensions scaled by
    /// `getScale(getPuffState())`.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        let base = if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        };
        base.scale(Self::puff_scale(self.puff_state()))
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

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("FromBucket", i8::from(self.from_bucket()));
        nbt.insert("PuffState", self.puff_state());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_from_bucket(nbt.byte("FromBucket").is_some_and(|value| value != 0));
        // Vanilla `readAdditionalSaveData`: `Math.min(input.getIntOr("PuffState", 0), 2)`.
        self.set_puff_state(nbt.int("PuffState").unwrap_or(0).min(MAX_PUFF_STATE));
    }

    /// Vanilla `Pufferfish.loadFromBucketTag`, restored by `MobBucketItem.spawn`
    /// on release.
    fn load_from_bucket_tag(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_from_bucket_data(nbt);
    }

    /// Vanilla `Bucketable.setFromBucket`, flipped for every released mob.
    fn set_from_bucket(&self, from_bucket: bool) {
        PufferfishEntity::set_from_bucket(self, from_bucket);
    }

    /// Vanilla `Pufferfish.playerTouch`.
    ///
    /// Deviation: vanilla only stings a `ServerPlayer`; every Steel player is
    /// server-side. Vanilla gates on `doTeamsAllowDamage`, which needs scoreboard teams
    /// Steel does not have, so `is_allied_to` is used instead.
    fn player_touch(self: Arc<Self>, player: &Arc<Player>) {
        if self.is_allied_to(player.as_ref()) {
            return;
        }

        let puff_state = self.puff_state();
        if puff_state <= STATE_SMALL {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let attacker = Arc::clone(&self);
        let attacker: SharedEntity = attacker;
        let source = DamageSource::environment(&vanilla_damage_types::MOB_ATTACK)
            .with_causing_entity(Arc::clone(&attacker))
            .with_direct_entity(attacker);
        if player.hurt_server(&world, &source, (1 + puff_state) as f32) {
            if !self.is_silent() {
                // Vanilla `ClientboundGameEventPacket.PUFFER_FISH_STING`.
                player.send_packet(CGameEvent {
                    event: GameEventType::PufferFishSting,
                    data: 0.0,
                });
            }
            player.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::POISON,
                POISON_DURATION_PER_STATE * puff_state,
                0,
            ));
        }
    }
}

impl LivingEntity for PufferfishEntity {
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
        Some(&sound_events::ENTITY_PUFFER_FISH_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PUFFER_FISH_DEATH)
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

    /// Vanilla `Pufferfish.aiStep`: the flop, then `super.aiStep()`, then the sting sweep.
    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        abstract_fish::tick_flop(self, &sound_events::ENTITY_PUFFER_FISH_FLOP);
        let result = Mob::mob_ai_step(self, entity);

        if Entity::is_alive(self)
            && self.puff_state() > STATE_SMALL
            && let Some(world) = self.level()
        {
            let self_living = self.as_living_entity();
            let targets = world.get_entities_in_aabb_matching(
                &self.bounding_box().inflate(TOUCH_INFLATION),
                |other| {
                    other.as_mob().is_some()
                        && other.as_living_entity().is_some_and(|living| {
                            TARGETING_CONDITIONS.test(world.as_ref(), self_living, living)
                        })
                },
            );
            for target in targets {
                if target.is_alive() {
                    self.touch(&world, &target, entity);
                }
            }
        }

        result
    }
}

impl Mob for PufferfishEntity {
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

    /// Vanilla `AbstractFish.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        abstract_fish::MAX_SPAWN_CLUSTER_SIZE
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

    fn ambient_sound_interval(&self) -> i32 {
        abstract_fish::AMBIENT_SOUND_INTERVAL
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for PufferfishEntity {}

/// Vanilla `Pufferfish.PufferfishPuffGoal`: inflate while a scary entity is nearby.
struct PufferfishPuffGoal;

impl PufferfishPuffGoal {
    fn fish(mob: &dyn PathfinderMob) -> Option<&PufferfishEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<PufferfishEntity>()
    }
}

impl Goal for PufferfishPuffGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(fish) = Self::fish(mob) else {
            return false;
        };
        let Some(world) = fish.level() else {
            return false;
        };

        let self_living = fish.as_living_entity();
        let entities =
            world.get_entities_in_aabb_matching(&fish.bounding_box().inflate(2.0), |other| {
                other.as_living_entity().is_some_and(|living| {
                    TARGETING_CONDITIONS.test(world.as_ref(), self_living, living)
                })
            });
        !entities.is_empty()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(fish) = Self::fish(mob) else {
            return;
        };
        fish.set_inflate_counter(1);
        fish.set_deflate_timer(0);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let Some(fish) = Self::fish(mob) else {
            return;
        };
        fish.set_inflate_counter(0);
    }
}
