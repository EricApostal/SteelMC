//! Vanilla `net.minecraft.world.entity.monster.warden.Warden`.
//!
//! The warden is a brain mob: [`brain`] owns the `WardenAi` activities and this
//! module owns the entity state those activities read — the anger table, the
//! client anger level, the darkness pulse, melee/sonic-boom cooldowns and the
//! digging/emerging invulnerability.
//!
//! ## Subsystems with no Steel counterpart
//!
//! - `VibrationSystem`: Steel has no game-event vibration plumbing (no
//!   `SculkSensor`/`SculkShrieker` behaviours and no `VibrationSystem.Data`), so
//!   the warden cannot hear vibrations and `listener` is not saved.
//!   [`WardenEntity::on_receive_vibration`] is vanilla's
//!   `VibrationUser.onReceiveVibration` kept whole for the day that subsystem
//!   lands; nothing dispatches to it yet.
//! - `WardenSpawnTracker` / `spawn_wardens`: the shrieker escalation that gates
//!   warden spawns is not part of this slice, so the game rule has no call site.
//! - Client animation state (`roarAnimationState`, tendril/heart animation
//!   counters, digging particles) and the heartbeat sound only exist on the
//!   vanilla client. [`WardenEntity::heartbeat_delay`] keeps the interval math.
//! - `getSecondsToDisableBlocking` and `dampensVibrations` have no Steel hook.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::WardenEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_mob_effects};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::GameType;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use super::anger::{AngerLevel, AngerManagement};
use super::brain;
use crate::entity::ai::behavior::warden as warden_ai;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

/// Vanilla `Warden.xpReward`.
const XP_REWARD: i32 = 5;

/// Vanilla `Warden.VIBRATION_COOLDOWN_TICKS`.
const VIBRATION_COOLDOWN_TICKS: i64 = 40;

/// Vanilla `Warden.TIME_TO_USE_MELEE_UNTIL_SONIC_BOOM`: the sonic-boom cooldown
/// `setAttackTarget` installs so the beam is only used after a melee stretch.
const TIME_TO_USE_MELEE_UNTIL_SONIC_BOOM: i64 = 200;

/// Vanilla `SonicBoom.COOLDOWN`, applied by a melee attack.
const SONIC_BOOM_COOLDOWN: i64 = 40;

/// Vanilla `Warden.DARKNESS_DISPLAY_LIMIT`.
const DARKNESS_DISPLAY_LIMIT: i32 = 200;

/// Vanilla `Warden.DARKNESS_DURATION`.
const DARKNESS_DURATION: i32 = 260;

/// Vanilla `Warden.DARKNESS_RADIUS`.
const DARKNESS_RADIUS: i32 = 20;

/// Vanilla `Warden.DARKNESS_INTERVAL`.
const DARKNESS_INTERVAL: i32 = 120;

/// Vanilla `Warden.ANGERMANAGEMENT_TICK_DELAY`.
const ANGER_MANAGEMENT_TICK_DELAY: i32 = 20;

/// Vanilla `Warden.DEFAULT_ANGER`.
const DEFAULT_ANGER: i32 = 35;

/// Vanilla `Warden.PROJECTILE_ANGER`.
const PROJECTILE_ANGER: i32 = 10;

/// Vanilla `Warden.ON_HURT_ANGER_BOOST`, added to `AngerLevel.ANGRY`'s minimum.
const ON_HURT_ANGER_BOOST: i32 = 20;

/// Vanilla `Warden.RECENT_PROJECTILE_TICK_THRESHOLD`.
const RECENT_PROJECTILE_TICK_THRESHOLD: i64 = 100;

/// Vanilla `Warden.TOUCH_COOLDOWN_TICKS`.
const TOUCH_COOLDOWN_TICKS: i64 = 20;

/// Vanilla `Warden.PROJECTILE_ANGER_DISTANCE`.
const PROJECTILE_ANGER_DISTANCE: f64 = 30.0;

/// Vanilla `Warden`: `closerThan(livingAttacker, 5.0)` in `hurtServer`.
const HURT_ANGER_TARGET_DISTANCE: f64 = 5.0;

/// Vanilla `Warden.getDefaultDimensions` while digging or emerging: the height
/// collapses to one block.
const DIGGING_HEIGHT: f32 = 1.0;

/// Vanilla `Warden.nextStep`: `moveDist + 0.55`.
const NEXT_STEP_OFFSET: f32 = 0.55;

/// Vanilla `Warden.getSoundVolume`.
const SOUND_VOLUME: f32 = 4.0;

/// Vanilla `Warden.playStepSound` volume.
const STEP_SOUND_VOLUME: f32 = 10.0;

/// Vanilla `Warden.doHurtTarget` impact-sound volume.
const ATTACK_IMPACT_VOLUME: f32 = 10.0;

/// Vanilla `Warden.playListeningSound` volume.
const LISTENING_SOUND_VOLUME: f32 = 10.0;

/// Vanilla `Warden.VibrationUser.onReceiveVibration` tendril-clicks volume.
const TENDRIL_CLICKS_VOLUME: f32 = 5.0;

/// Vanilla `Warden.finalizeSpawn` emerge-sound volume.
const EMERGE_SOUND_VOLUME: f32 = 5.0;

/// A warden: 500 health, anger-driven brain, darkness aura and sonic boom.
#[entity_behavior(class = "Warden")]
pub struct WardenEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<WardenEntityData>,
    brain: SyncMutex<Brain>,
    anger: SyncMutex<AngerManagement>,
    /// Sonic-boom cooldown queued by a melee hit.
    ///
    /// Vanilla sets it directly in `doHurtTarget`, which Steel reaches through
    /// `Mob::after_hurt_target` — a hook the brain's `MeleeAttack` behavior calls
    /// while it already holds this entity's brain lock. `parking_lot::Mutex` is
    /// not reentrant, so the value is queued here and written by [`Mob::brain_tick`]
    /// once the brain lock is free, in the same tick.
    pending_sonic_boom_cooldown: SyncMutex<Option<i64>>,
}

// SAFETY: The owner-scoped type key uniquely identifies WardenEntity.
unsafe impl DowncastType for WardenEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/warden");
}

impl WardenEntity {
    /// Creates a new warden entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved warden entity.
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
        mob_base.set_xp_reward(XP_REWARD);
        let mut entity_data = WardenEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `Warden` constructor: `getNavigation().setCanFloat(true)` and the
            // pathfinding maluses the warden overrides.
            mob_base.navigation().lock().set_can_float(true);

            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::UnpassableRail, 0.0);
            malus.set(PathType::Damaging, 8.0);
            malus.set(PathType::PowderSnow, 8.0);
            malus.set(PathType::Lava, 8.0);
            malus.set(PathType::Fire, 0.0);
            malus.set(PathType::FireInNeighbor, 0.0);
        }

        // Vanilla keeps its anger on the entity, but the Steel brain reads the
        // angry-at suspect through the `ANGRY_AT` memory (see `brain::angry_at`),
        // which `brain::make_brain` registers with the rest of the provider's
        // memories.
        let brain = brain::make_brain();

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(brain),
            anger: SyncMutex::new(AngerManagement::new()),
            pending_sonic_boom_cooldown: SyncMutex::new(None),
        }
    }

    /// Vanilla `Warden.getAngerLevel`.
    #[must_use]
    pub fn anger_level(&self) -> AngerLevel {
        AngerLevel::by_anger(self.active_anger())
    }

    /// Vanilla `Warden.canTargetEntity`.
    ///
    /// `EntitySelector.NO_CREATIVE_OR_SPECTATOR` is spelled with
    /// `Player::has_infinite_materials`/`Player::is_spectator`, the same pair
    /// `Mob::is_valid_target` uses.
    #[must_use]
    pub fn can_target_entity(&self, entity: &SharedEntity) -> bool {
        self.can_target_entity_ref(entity.as_ref())
    }

    /// Vanilla `Warden.canTargetEntity` over any entity reference, so both
    /// [`Self::can_target_entity`] and `Mob::can_attack` share one predicate.
    fn can_target_entity_ref(&self, entity: &dyn Entity) -> bool {
        let Some(living) = entity.as_living_entity() else {
            return false;
        };
        let (Some(level), Some(entity_level)) = (self.level(), entity.level()) else {
            return false;
        };

        Arc::ptr_eq(&level, &entity_level)
            && !entity
                .as_player()
                .is_some_and(|player| player.has_infinite_materials() || player.is_spectator())
            && !self.is_allied_to(entity)
            && entity.entity_type() != &vanilla_entities::ARMOR_STAND
            && entity.entity_type() != &vanilla_entities::WARDEN
            && !entity.is_invulnerable()
            && !living.is_dead_or_dying()
            && level
                .world_border_snapshot()
                .is_within_bounds(entity.bounding_box())
    }

    /// Vanilla `Warden.clearAnger`.
    pub fn clear_anger(&self, suspect: &SharedEntity) {
        self.anger.lock().clear_anger(suspect);
        self.sync_brain_anger();
    }

    /// Vanilla `Warden.getEntityAngryAt`: the top suspect while angry.
    #[must_use]
    pub fn get_entity_angry_at(&self) -> Option<SharedEntity> {
        if !self.anger_level().is_angry() {
            return None;
        }
        self.active_suspect()
    }

    /// Vanilla `Warden.increaseAngerAt(entity)`, the `35`/sound variant.
    pub fn increase_anger_at(&self, entity: Option<&SharedEntity>) {
        self.increase_anger_at_by(entity, DEFAULT_ANGER, true);
    }

    /// Vanilla `Warden.setPose(Pose.EMERGING)`/`Pose.STANDING` half of
    /// `finalizeSpawn`, exposed so spawn code can put a summoned warden in the
    /// ground.
    pub fn set_emerging(&self, emerging: bool) {
        if emerging {
            self.set_pose(EntityPose::Emerging);
        } else if self.pose() == EntityPose::Emerging {
            self.set_pose(EntityPose::Standing);
        }
    }

    /// Returns whether this warden is currently emerging from the ground.
    #[must_use]
    pub fn is_emerging(&self) -> bool {
        self.pose() == EntityPose::Emerging
    }

    /// Vanilla `WardenAi.setDigCooldown`: refresh `DIG_COOLDOWN` if remembered.
    pub fn set_dig_cooldown(&self) {
        brain::set_dig_cooldown(&mut self.brain.lock());
    }

    /// Vanilla `WardenAi.setDisturbanceLocation`.
    ///
    /// `entity_angry_at` is `Warden.getEntityAngryAt().isPresent()`; the caller
    /// owns the world-border guard, as in vanilla.
    pub fn set_disturbance_location(&self, position: BlockPos, entity_angry_at: bool) {
        brain::set_disturbance_location(&mut self.brain.lock(), position, entity_angry_at);
    }

    /// Vanilla `Warden.getHeartBeatDelay`.
    ///
    /// The heartbeat is a client sound; the interval is kept here so the
    /// synced client anger level keeps its only consumer.
    #[must_use]
    pub fn heartbeat_delay(&self) -> i32 {
        let anger = *self.entity_data.lock().client_anger_level.get() as f32
            / AngerLevel::Angry.minimum_anger() as f32;
        40 - (anger.clamp(0.0, 1.0) * 30.0).floor() as i32
    }

    /// Vanilla `Warden.VibrationUser.onReceiveVibration`.
    ///
    /// Steel has no vibration system, so nothing calls this yet; it is the
    /// complete anger half of vanilla's callback (`VIBRATION_COOLDOWN`, tendril
    /// clicks, `RECENT_PROJECTILE`, the `+35` source gain and the `+10`
    /// projectile gain) so a future `VibrationSystem` only has to dispatch.
    pub fn on_receive_vibration(
        &self,
        position: BlockPos,
        source_entity: Option<&SharedEntity>,
        projectile_owner: Option<&SharedEntity>,
    ) {
        if self.is_dead_or_dying() {
            return;
        }

        self.brain.lock().set_memory_with_expiry(
            &mem::VIBRATION_COOLDOWN,
            (),
            VIBRATION_COOLDOWN_TICKS,
        );
        self.broadcast_entity_event(EntityStatus::TendrilsShiver);
        self.play_sound(
            &sound_events::ENTITY_WARDEN_TENDRIL_CLICKS,
            TENDRIL_CLICKS_VOLUME,
            self.voice_pitch(),
        );

        let mut suspicious_position = position;
        if let Some(owner) = projectile_owner {
            if self.closer_than(owner, PROJECTILE_ANGER_DISTANCE) {
                let recently_angered_by_projectile =
                    self.brain.lock().has_memory_value(&mem::RECENT_PROJECTILE);
                if recently_angered_by_projectile {
                    if self.can_target_entity(owner) {
                        suspicious_position = owner.block_position();
                    }
                    self.increase_anger_at_by(Some(owner), DEFAULT_ANGER, true);
                } else {
                    self.increase_anger_at_by(Some(owner), PROJECTILE_ANGER, true);
                }
            }

            self.brain.lock().set_memory_with_expiry(
                &mem::RECENT_PROJECTILE,
                (),
                RECENT_PROJECTILE_TICK_THRESHOLD,
            );
        } else {
            self.increase_anger_at_by(source_entity, DEFAULT_ANGER, true);
        }

        if !self.anger_level().is_angry() {
            let active_suspect = self.active_suspect();
            if projectile_owner.is_some()
                || active_suspect.is_none()
                || active_suspect.is_some_and(|active| {
                    source_entity.is_some_and(|source| Arc::ptr_eq(&active, source))
                })
            {
                self.set_disturbance_location(
                    suspicious_position,
                    self.get_entity_angry_at().is_some(),
                );
            }
        }
    }

    /// Vanilla `Warden.increaseAngerAt(entity, amount, playSound)`.
    pub fn increase_anger_at_by(
        &self,
        entity: Option<&SharedEntity>,
        amount: i32,
        play_sound: bool,
    ) {
        let Some(entity) = entity else {
            return;
        };
        if self.is_no_ai() || !self.can_target_entity(entity) {
            return;
        }

        self.set_dig_cooldown();

        let current_target_is_player = self
            .target()
            .is_some_and(|target| target.as_player().is_some());
        let new_anger = self.anger.lock().increase_anger(entity, amount);
        if entity.as_player().is_some()
            && !current_target_is_player
            && AngerLevel::by_anger(new_anger).is_angry()
        {
            self.brain.lock().erase_memory(&mem::ATTACK_TARGET);
        }
        self.sync_brain_anger();

        if play_sound {
            self.play_listening_sound();
        }
    }

    /// Vanilla `Warden.playListeningSound`.
    fn play_listening_sound(&self) {
        if self.pose() != EntityPose::Roaring {
            self.play_sound(
                self.anger_level().listening_sound(),
                LISTENING_SOUND_VOLUME,
                self.voice_pitch(),
            );
        }
    }

    /// Vanilla `Warden.getActiveAnger`.
    fn active_anger(&self) -> i32 {
        let target = self.target();
        self.anger.lock().get_active_anger(target.as_ref())
    }

    /// Vanilla `AngerManagement.getActiveEntity` through the warden's
    /// `canTargetEntity` filter.
    fn active_suspect(&self) -> Option<SharedEntity> {
        self.anger
            .lock()
            .get_active_entity(|entity| self.can_target_entity(entity))
    }

    /// Vanilla `Warden.syncClientAngerLevel`.
    fn sync_client_anger_level(&self) {
        let active_anger = self.active_anger();
        self.entity_data.lock().client_anger_level.set(active_anger);
    }

    /// Mirrors the warden's anger into the brain memories `brain` reads, standing
    /// in for vanilla's live `Warden.getEntityAngryAt`/`getAngerLevel` calls.
    fn sync_brain_anger(&self) {
        let angry_at = self.get_entity_angry_at().map(|entity| entity.uuid());
        let mut brain = self.brain.lock();
        match angry_at {
            Some(uuid) => brain.set_memory(&mem::ANGRY_AT, Some(uuid)),
            None => brain.erase_memory(&mem::ANGRY_AT),
        }
    }

    /// Vanilla `Warden.setAttackTarget`.
    fn set_attack_target(&self, target: &SharedEntity) {
        let mut brain = self.brain.lock();
        brain.erase_memory(&mem::ROAR_TARGET);
        brain.set_memory(&mem::ATTACK_TARGET, Some(target.clone()));
        brain.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
        brain.set_memory_with_expiry(
            &mem::SONIC_BOOM_COOLDOWN,
            (),
            TIME_TO_USE_MELEE_UNTIL_SONIC_BOOM,
        );
    }

    /// Vanilla `SonicBoom.setCooldown`, safe to call while this entity's brain is
    /// locked (see [`Self::pending_sonic_boom_cooldown`]).
    fn queue_sonic_boom_cooldown(&self, ticks: i64) {
        *self.pending_sonic_boom_cooldown.lock() = Some(ticks);
    }

    /// Vanilla `Warden.isDiggingOrEmerging`.
    #[must_use]
    fn is_digging_or_emerging(&self) -> bool {
        matches!(self.pose(), EntityPose::Digging | EntityPose::Emerging)
    }

    /// Vanilla `Entity.closerThan(other, distance)`.
    fn closer_than(&self, other: &SharedEntity, distance: f64) -> bool {
        self.position().distance_squared(other.position()) < distance * distance
    }

    /// Vanilla `Warden.customServerAiStep`'s anger tick.
    fn tick_anger(&self, world: &World) {
        {
            let mut anger = self.anger.lock();
            anger.tick(world, |suspect| self.can_target_entity(suspect));
        }
        self.sync_client_anger_level();
        self.sync_brain_anger();
    }

    /// Vanilla `Warden.applyDarknessAround`.
    fn apply_darkness_around(&self, world: &World, radius: i32) {
        let position = self.position();
        let darkness =
            MobEffectInstance::with_duration(vanilla_mob_effects::DARKNESS, DARKNESS_DURATION, 0)
                .with_ambient(false)
                .with_visible(false);

        world.players.iter_players(|_, player| {
            if player.game_mode() != GameType::Survival
                || self.is_allied_to(player.as_ref())
                || position.distance_squared(player.position())
                    >= f64::from(radius) * f64::from(radius)
            {
                return true;
            }

            // Vanilla `MobEffectUtil.addEffectToPlayersAround`: skip players that
            // already have a stronger or long-running instance.
            let eligible =
                player
                    .mob_effect(vanilla_mob_effects::DARKNESS)
                    .is_none_or(|existing| {
                        existing.amplifier() < darkness.amplifier()
                            || (existing.duration() != -1
                                && existing.duration() <= DARKNESS_DISPLAY_LIMIT - 1)
                    });
            if eligible {
                player.add_mob_effect(darkness.clone());
            }
            true
        });
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
    }
}

impl Entity for WardenEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        // Vanilla `Warden.tick`: a persistent warden keeps its dig cooldown alive.
        if self.level().is_some()
            && (self.is_persistence_required() || self.requires_custom_persistence())
        {
            self.set_dig_cooldown();
        }
    }

    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        let dimensions = if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        };

        if matches!(pose, EntityPose::Digging | EntityPose::Emerging) {
            EntityDimensions::with_default_eye_height(dimensions.width, DIGGING_HEIGHT)
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

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(0.6) as f32
    }

    fn next_step(&self) -> f32 {
        self.base().movement_progress().move_dist() + NEXT_STEP_OFFSET
    }

    fn is_pushable(&self) -> bool {
        // Vanilla `Warden.isPushable` over `Entity::is_pushable`, which cannot be
        // called from an override.
        // `Entity::is_alive` is Steel's `LivingEntity.isAlive` (not removed and
        // health above zero), which is what vanilla's `isAlive()` dispatches to.
        !self.is_digging_or_emerging()
            && Entity::is_alive(self)
            && !self.is_spectator()
            && !self.on_climbable()
    }

    fn can_ride(&self, _vehicle: &dyn Entity) -> bool {
        false
    }

    fn sound_source(&self) -> SoundSource {
        Monster::sound_source_monster(self)
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_WARDEN_STEP, STEP_SOUND_VOLUME, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        // Vanilla also stores `listener` (`VibrationSystem.Data`), which Steel has
        // no equivalent for.
        self.anger.lock().save(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        *self.anger.lock() = AngerManagement::load(&nbt);
        self.sync_client_anger_level();
        self.sync_brain_anger();
    }
}

impl LivingEntity for WardenEntity {
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
        SOUND_VOLUME
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WARDEN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WARDEN_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }

    fn default_is_invulnerable_to(&self, source: &DamageSource) -> bool {
        // Vanilla `Warden.isInvulnerableTo`: digging and emerging wardens ignore
        // everything that does not bypass invulnerability.
        if self.is_digging_or_emerging() && !source.bypasses_invulnerability() {
            return true;
        }

        self.is_invulnerable_to_base(source)
    }

    fn before_actually_hurt(&self, source: &DamageSource, _amount: f32) {
        // Vanilla `Warden.hurtServer` raises anger after `super.hurtServer`; Steel
        // has no post-hurt hook and the default `hurtServer` body cannot be called
        // from an override, so the anger bump happens here, in the same tick and
        // before the health subtraction.
        if self.is_no_ai() || self.is_digging_or_emerging() {
            return;
        }

        let attacker = source.causing_entity();
        self.increase_anger_at_by(
            attacker,
            AngerLevel::Angry.minimum_anger() + ON_HURT_ANGER_BOOST,
            false,
        );

        let Some(attacker) = attacker else {
            return;
        };
        if self.target().is_none()
            && (source.is_direct() || self.closer_than(&attacker, HURT_ANGER_TARGET_DISTANCE))
        {
            self.set_attack_target(&attacker);
        }
    }

    fn do_push(&self, entity: &SharedEntity) {
        // Vanilla `Warden.doPush`: being touched angers the warden once per
        // `TOUCH_COOLDOWN` and points it at the disturbance.
        let touch_on_cooldown = self.brain.lock().has_memory_value(&mem::TOUCH_COOLDOWN);
        if !self.is_no_ai() && !touch_on_cooldown {
            self.brain.lock().set_memory_with_expiry(
                &mem::TOUCH_COOLDOWN,
                (),
                TOUCH_COOLDOWN_TICKS,
            );
            self.increase_anger_at_by(Some(entity), DEFAULT_ANGER, true);
            self.set_disturbance_location(
                entity.block_position(),
                self.get_entity_angry_at().is_some(),
            );
        }

        entity.push_entity(self.as_entity_event_source());
    }
}

impl Mob for WardenEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        // Vanilla `Warden.getAmbientSound`: silent while roaring or underground.
        if self.pose() == EntityPose::Roaring || self.is_digging_or_emerging() {
            return None;
        }
        Some(self.anger_level().ambient_sound())
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

    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        {
            let mut brain = self.brain.lock();
            brain::tick(&mut brain, &world, entity);

            // A melee hit during the brain tick queued its sonic-boom cooldown;
            // apply it now that the lock is ours (vanilla sets it in
            // `doHurtTarget`, i.e. inside the brain tick).
            if let Some(ticks) = self.pending_sonic_boom_cooldown.lock().take() {
                brain.set_memory_with_expiry(&mem::SONIC_BOOM_COOLDOWN, (), ticks);
            }
        }

        // Vanilla `Warden.customServerAiStep`: brain tick, then darkness, then the
        // anger tick, then `WardenAi.updateActivity`. The anger tick and
        // `update_activity` both lock the brain, so the guard above is released
        // first.
        if (self.tick_count() + self.id()) % DARKNESS_INTERVAL == 0 {
            self.apply_darkness_around(&world, DARKNESS_RADIUS);
        }

        if self.tick_count() % ANGER_MANAGEMENT_TICK_DELAY == 0 {
            self.tick_anger(&world);
        }

        brain::update_activity(&mut self.brain.lock());
    }

    fn target(&self) -> Option<SharedEntity> {
        // Vanilla `Warden.getTarget` is `getTargetFromBrain`.
        self.brain.lock().get_memory(&mem::ATTACK_TARGET)
    }

    fn set_target(&self, target: Option<&SharedEntity>) -> bool {
        // Vanilla `Warden.getTarget` is `getTargetFromBrain`, so the plain
        // `Mob.setTarget` write would be invisible; Steel writes the memory
        // instead. The `SonicBoom` cooldown of `Warden.setAttackTarget` is *not*
        // applied here — vanilla only does that from the warden's own method.
        let Some(target) = target else {
            self.brain.lock().erase_memory(&mem::ATTACK_TARGET);
            return true;
        };
        if !target.is_living_entity() {
            return false;
        }

        self.brain
            .lock()
            .set_memory(&mem::ATTACK_TARGET, Some(target.clone()));
        true
    }

    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        // Vanilla `Warden.canAttack` is exactly `canTargetEntity`, with no
        // ghast exclusion from `Mob.canAttack`.
        self.can_target_entity_ref(target)
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        false
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.brain.lock().set_memory_with_expiry(
            &mem::DIG_COOLDOWN,
            (),
            warden_ai::DIGGING_COOLDOWN,
        );

        if spawn_reason == EntitySpawnReason::Triggered {
            self.set_pose(EntityPose::Emerging);
            self.brain.lock().set_memory_with_expiry(
                &mem::IS_EMERGING,
                (),
                i64::from(warden_ai::EMERGE_DURATION),
            );
            self.play_sound(
                &sound_events::ENTITY_WARDEN_AGITATED,
                EMERGE_SOUND_VOLUME,
                1.0,
            );
        }

        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn after_hurt_target(&self, _world: &World, _target: &SharedEntity, _was_hurt: bool) {
        // Vanilla `Warden.doHurtTarget`: entity event 4 (attack animation), the
        // impact sound and a 40-tick sonic-boom cooldown. Vanilla runs these
        // before the damage roll; Steel's post-attack hook runs them in the same
        // tick, right after the base melee resolution.
        self.broadcast_entity_event(EntityStatus::StartAttacking);
        self.play_sound(
            &sound_events::ENTITY_WARDEN_ATTACK_IMPACT,
            ATTACK_IMPACT_VOLUME,
            self.voice_pitch(),
        );
        self.queue_sonic_boom_cooldown(SONIC_BOOM_COOLDOWN);
    }
}

impl Monster for WardenEntity {}

impl PathfinderMob for WardenEntity {}
