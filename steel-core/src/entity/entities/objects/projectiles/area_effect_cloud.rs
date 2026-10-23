//! A lingering area-effect cloud (`AreaEffectCloud`).
//!
//! Mirrors vanilla `AreaEffectCloud`, which is a plain `Entity` and not a
//! projectile: it counts ticks, shrinks its radius, and every five ticks applies
//! the cloud's effects to every potion-affected living entity inside it.
//! Lingering potions and dragon fireballs both configure a freshly spawned
//! cloud through the setters below.
//!
//! The `custom_particle` field has no Steel NBT codec (see the gap note on the
//! setter): it round-trips through synced entity data, but NBT load always
//! clears it.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::FromNbtTag as _;
use simdnbt::ToNbtTag as _;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::behavior::PushReaction;
use steel_registry::data_components::PotionContents;
use steel_registry::data_components::vanilla_components::{POTION_CONTENTS, POTION_DURATION_SCALE};
use steel_registry::entity_data::EntityPose;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::{ColorParticleOption, ParticleData};
use steel_registry::vanilla_entity_data::AreaEffectCloudEntityData;
use steel_registry::{vanilla_entities, vanilla_particle_types};
use steel_utils::locks::SyncMutex;
use steel_utils::{ArgbColor, DowncastType, DowncastTypeKey};

use crate::behavior::MOB_EFFECT_BEHAVIORS;
use crate::entity::damage::DamageSource;
use crate::entity::potion_contents::to_runtime_instance;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityReference, EntitySyncedData, RemovalReason,
    SharedEntity, next_entity_id,
};
use crate::world::World;

use super::thrown_potion::{potion_color, scale_effect_duration};

/// Vanilla `AreaEffectCloud.TIME_BETWEEN_APPLICATIONS`.
const APPLICATION_INTERVAL_TICKS: i32 = 5;

/// Vanilla `AreaEffectCloud.MINIMAL_RADIUS`.
const MINIMAL_RADIUS: f32 = 0.5;

/// Vanilla `AreaEffectCloud.MAX_RADIUS`.
const MAX_RADIUS: f32 = 32.0;

/// Vanilla `AreaEffectCloud.DEFAULT_RADIUS`.
const DEFAULT_RADIUS: f32 = 3.0;

/// Vanilla `AreaEffectCloud.HEIGHT`.
const CLOUD_HEIGHT: f32 = 0.5;

/// Vanilla `AreaEffectCloud.DEFAULT_WAIT_TIME` / `INFINITE_DURATION` default.
const DEFAULT_WAIT_TIME: i32 = 20;

/// Vanilla `AreaEffectCloud.DEFAULT_REAPPLICATION_DELAY`.
const DEFAULT_REAPPLICATION_DELAY: i32 = 20;

/// Vanilla cloud instant-effect scale: cloud-applied instantaneous effects
/// always run at half strength.
const CLOUD_INSTANT_SCALE: f32 = 0.5;

/// Mutable cloud state behind a single lock (vanilla fields are plain).
struct CloudState {
    /// Vanilla `potionContents` with its own duration scale folded in on read.
    contents: PotionContents,
    /// Vanilla `potionDurationScale`.
    potion_duration_scale: f32,
    /// Server-chosen particle override; replaces the potion-derived default.
    custom_particle: Option<ParticleData>,
    /// Vanilla `victims`: entity id to the first tick it may be affected again.
    victims: HashMap<i32, i32>,
    /// Vanilla `duration` (-1 for infinite).
    duration: i32,
    /// Vanilla `waitTime`.
    wait_time: i32,
    /// Vanilla `reapplicationDelay`.
    reapplication_delay: i32,
    /// Vanilla `durationOnUse`.
    duration_on_use: i32,
    /// Vanilla `radiusOnUse`.
    radius_on_use: f32,
    /// Vanilla `radiusPerTick`.
    radius_per_tick: f32,
    /// Vanilla `owner` (`EntityReference<LivingEntity>`).
    owner: Option<EntityReference>,
    /// Loaded vanilla `Age` minus the base tick counter at load time.
    ///
    /// Vanilla persists `AreaEffectCloud` age in `Age` and reads it back into
    /// `tickCount`, which drives the cloud's lifetime. Steel's base entity does
    /// not persist its tick counter, so the loaded age is kept as an offset on
    /// top of [`Entity::tick_count`] instead of shadowing it. Remove this once
    /// the base entity restores `Age` (reported foundation gap).
    age_offset: i32,
}

impl CloudState {
    /// Creates the default state of an empty cloud.
    fn new() -> Self {
        Self {
            contents: PotionContents::empty(),
            potion_duration_scale: 1.0,
            custom_particle: None,
            victims: HashMap::new(),
            duration: -1,
            wait_time: DEFAULT_WAIT_TIME,
            reapplication_delay: DEFAULT_REAPPLICATION_DELAY,
            duration_on_use: 0,
            radius_on_use: 0.0,
            radius_per_tick: 0.0,
            owner: None,
            age_offset: 0,
        }
    }
}

/// An area-effect cloud.
#[entity_behavior(class = "AreaEffectCloud")]
pub struct AreaEffectCloudEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced radius / waiting / particle data.
    entity_data: SyncMutex<AreaEffectCloudEntityData>,
    /// Mutable gameplay state (potion contents, timers, owner).
    state: SyncMutex<CloudState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies
// `AreaEffectCloudEntity`.
unsafe impl DowncastType for AreaEffectCloudEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/area_effect_cloud");
}

impl AreaEffectCloudEntity {
    /// Creates a new area-effect cloud with default state.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        let mut entity_data = AreaEffectCloudEntityData::new();
        entity_data.radius.set(DEFAULT_RADIUS);
        // Vanilla `AreaEffectCloud` constructor sets `noPhysics = true`.
        let base = EntityBase::new(id, position, entity_type.dimensions, world);
        base.set_no_physics(true);
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(CloudState::new()),
        }
    }

    /// Creates an area-effect cloud at `position` in `world`, ready to configure.
    ///
    /// Convenience constructor for spawners (lingering potions, dragon
    /// fireballs) that mirrors vanilla `new AreaEffectCloud(level, x, y, z)`.
    #[must_use]
    pub fn new_at_position(world: &Arc<World>, position: DVec3) -> Arc<Self> {
        Arc::new(Self::new(
            &vanilla_entities::AREA_EFFECT_CLOUD,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        ))
    }

    /// Creates an area-effect cloud from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let base = EntityBase::from_load(load, entity_type.dimensions);
        base.set_no_physics(true);
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(AreaEffectCloudEntityData::new()),
            state: SyncMutex::new(CloudState::new()),
        }
    }

    /// Returns the vanilla cloud age in ticks (`tickCount`, restored from `Age`).
    fn age(&self) -> i32 {
        let offset = self.state.lock().age_offset;
        self.tick_count().wrapping_add(offset)
    }

    /// Returns the cloud radius in blocks.
    #[must_use]
    pub fn radius(&self) -> f32 {
        *self.entity_data.lock().radius.get()
    }

    /// Sets the cloud radius, clamped to vanilla's `[0, 32]`.
    ///
    /// Vanilla only syncs the radius server-side; unloaded clouds keep their
    /// last synced value, matching the early return here.
    pub fn set_radius(&self, radius: f32) {
        if self.level().is_none() {
            return;
        }
        self.entity_data
            .lock()
            .radius
            .set(radius.clamp(0.0, MAX_RADIUS));
        self.refresh_dimensions();
    }

    /// Returns whether the cloud is still in its wait phase.
    #[must_use]
    pub fn is_waiting(&self) -> bool {
        *self.entity_data.lock().waiting.get()
    }

    /// Returns the synced particle payload.
    #[must_use]
    pub fn particle(&self) -> ParticleData {
        self.entity_data.lock().particle.get().clone()
    }

    /// Sets the potion contents and refreshes the synced particle color.
    pub fn set_potion_contents(&self, contents: PotionContents) {
        self.state.lock().contents = contents;
        self.update_particle();
    }

    /// Reads a clone of the stored potion contents.
    #[must_use]
    pub fn potion_contents(&self) -> PotionContents {
        self.state.lock().contents.clone()
    }

    /// Sets the cloud's potion-duration scale.
    pub fn set_potion_duration_scale(&self, scale: f32) {
        self.state.lock().potion_duration_scale = scale;
    }

    /// Reads the cloud's potion-duration scale.
    #[must_use]
    pub fn potion_duration_scale(&self) -> f32 {
        self.state.lock().potion_duration_scale
    }

    /// Adds one effect to the cloud's potion contents.
    pub fn add_effect(&self, effect: steel_registry::MobEffectInstance) {
        let contents = self.state.lock().contents.clone();
        let mut custom = contents.custom_effects().to_vec();
        custom.push(effect);
        self.set_potion_contents(PotionContents::new(
            contents.potion(),
            contents.custom_color(),
            custom,
            None,
        ));
    }

    /// Sets a custom particle override, replacing the potion-derived color.
    ///
    /// GAP: Steel has no NBT codec for `ParticleData` (network `WriteTo` /
    /// `ReadFrom` only), so this field persists through synced entity data but
    /// not through save/load. This needs `ToNbtTag` + `FromNbtTag for
    /// ParticleData` in `steel-registry` (registry lane owns that file).
    pub fn set_custom_particle(&self, particle: Option<ParticleData>) {
        self.state.lock().custom_particle = particle;
        self.update_particle();
    }

    /// Returns the cloud duration in ticks (-1 for infinite).
    #[must_use]
    pub fn duration(&self) -> i32 {
        self.state.lock().duration
    }

    /// Sets the cloud duration in ticks (-1 for infinite).
    pub fn set_duration(&self, duration: i32) {
        self.state.lock().duration = duration;
    }

    /// Returns the wait time before the cloud becomes active.
    #[must_use]
    pub fn wait_time(&self) -> i32 {
        self.state.lock().wait_time
    }

    /// Sets the wait time before the cloud becomes active.
    pub fn set_wait_time(&self, wait_time: i32) {
        self.state.lock().wait_time = wait_time;
    }

    /// Returns the reapplication delay between effects on the same victim.
    #[must_use]
    pub fn reapplication_delay(&self) -> i32 {
        self.state.lock().reapplication_delay
    }

    /// Sets the reapplication delay between effects on the same victim.
    pub fn set_reapplication_delay(&self, delay: i32) {
        self.state.lock().reapplication_delay = delay;
    }

    /// Returns the duration added or removed per entity affected.
    #[must_use]
    pub fn duration_on_use(&self) -> i32 {
        self.state.lock().duration_on_use
    }

    /// Sets the duration added or removed per entity affected.
    pub fn set_duration_on_use(&self, duration: i32) {
        self.state.lock().duration_on_use = duration;
    }

    /// Returns the radius added or removed per entity affected.
    #[must_use]
    pub fn radius_on_use(&self) -> f32 {
        self.state.lock().radius_on_use
    }

    /// Sets the radius added or removed per entity affected.
    pub fn set_radius_on_use(&self, radius: f32) {
        self.state.lock().radius_on_use = radius;
    }

    /// Returns the per-tick radius change.
    #[must_use]
    pub fn radius_per_tick(&self) -> f32 {
        self.state.lock().radius_per_tick
    }

    /// Sets the per-tick radius change.
    pub fn set_radius_per_tick(&self, radius_per_tick: f32) {
        self.state.lock().radius_per_tick = radius_per_tick;
    }

    /// Sets the cloud owner (a living entity).
    pub fn set_owner(&self, owner: &SharedEntity) {
        self.state.lock().owner = Some(EntityReference::from_entity(owner));
    }

    /// Returns the cloud owner, if still present.
    #[must_use]
    pub fn owner(&self) -> Option<SharedEntity> {
        let world = self.level()?;
        let owner = self.state.lock().owner.clone()?;
        owner.get_entity(&world)
    }

    /// Copies the item stack's `potion_contents` and `potion_duration_scale`
    /// components onto the cloud, like vanilla's spawn-from-item path.
    pub fn apply_components_from_item_stack(&self, stack: &ItemStack) {
        let contents = stack.get_or_default(POTION_CONTENTS, PotionContents::empty());
        let scale = stack.get_or_default(POTION_DURATION_SCALE, 1.0);
        {
            let mut state = self.state.lock();
            state.contents = contents;
            state.potion_duration_scale = scale;
        }
        self.update_particle();
    }

    /// Refreshes the synced particle to the custom override or the potion
    /// color, mirroring vanilla `updateParticle`.
    fn update_particle(&self) {
        let particle = {
            let state = self.state.lock();
            if let Some(custom) = &state.custom_particle {
                custom.clone()
            } else {
                ParticleData::new(
                    &vanilla_particle_types::ENTITY_EFFECT,
                    ColorParticleOption::new(ArgbColor::new(potion_color(&state.contents))),
                )
            }
        };
        self.entity_data.lock().particle.set(particle);
    }

    /// Vanilla `AreaEffectCloud` server tick.
    fn server_tick(self: &Arc<Self>, world: &Arc<World>) {
        let age = self.age();
        let (duration, wait_time) = {
            let state = self.state.lock();
            (state.duration, state.wait_time)
        };
        if duration != -1 && age - wait_time >= duration {
            self.set_removed(RemovalReason::Discarded);
            return;
        }

        let should_wait = age < wait_time;
        {
            let mut entity_data = self.entity_data.lock();
            if *entity_data.waiting.get() != should_wait {
                entity_data.waiting.set(should_wait);
            }
        }
        if should_wait {
            return;
        }

        if self.state.lock().radius_per_tick != 0.0 {
            let next = self.radius() + self.state.lock().radius_per_tick;
            if next < MINIMAL_RADIUS {
                self.set_removed(RemovalReason::Discarded);
                return;
            }
            self.entity_data.lock().radius.set(next);
            // Vanilla `refreshDimensions` re-centers the box on the same
            // position; the shared refresh does the same work here.
            self.refresh_dimensions();
        }

        if age % APPLICATION_INTERVAL_TICKS != 0 {
            return;
        }

        {
            let mut state = self.state.lock();
            state.victims.retain(|_, reapply_tick| age < *reapply_tick);
            if state.contents.all_effects().is_empty() {
                // Vanilla clears victims but keeps ticking an effect-less cloud.
                state.victims.clear();
                return;
            }
        }

        let source: SharedEntity = Arc::clone(self) as SharedEntity;
        let owner = self.owner();
        let entities = world.get_entities_in_aabb_matching(&self.bounding_box(), |entity| {
            entity.as_living_entity().is_some()
        });

        for entity in entities {
            let Some(living) = entity.as_living_entity() else {
                continue;
            };
            if !living.is_affected_by_potions() {
                continue;
            }

            let (contents, scale, reapplication_delay) = {
                let state = self.state.lock();
                if state.victims.contains_key(&entity.id()) {
                    continue;
                }
                (
                    state.contents.clone(),
                    state.potion_duration_scale,
                    state.reapplication_delay,
                )
            };

            // Vanilla compares the horizontal distance against the radius.
            let radius = self.radius();
            let dx = entity.position().x - self.position().x;
            let dz = entity.position().z - self.position().z;
            if dx * dx + dz * dz > f64::from(radius * radius) {
                continue;
            }

            // Vanilla skips entities none of whose effects can apply.
            let effects = contents.all_effects();
            if effects.iter().all(|effect| {
                !living.can_be_affected(&to_runtime_instance(
                    effect,
                    scale_effect_duration(effect.duration(), scale),
                ))
            }) {
                continue;
            }

            self.state
                .lock()
                .victims
                .insert(entity.id(), age + reapplication_delay);

            for effect in &effects {
                let behavior = MOB_EFFECT_BEHAVIORS.get_behavior(effect.effect());
                if let Some(instantaneous) = behavior.as_instantaneous() {
                    instantaneous.apply_instantaneous(
                        world,
                        living,
                        effect.amplifier(),
                        Some(&source),
                        owner.as_ref(),
                        CLOUD_INSTANT_SCALE,
                    );
                    continue;
                }
                let effect_duration = scale_effect_duration(effect.duration(), scale);
                living.add_mob_effect(to_runtime_instance(effect, effect_duration));
            }

            if self.state.lock().radius_on_use != 0.0 {
                let next = self.radius() + self.state.lock().radius_on_use;
                if next < MINIMAL_RADIUS {
                    self.set_removed(RemovalReason::Discarded);
                    return;
                }
                self.entity_data.lock().radius.set(next);
                self.refresh_dimensions();
            }

            let (duration_on_use, duration) = {
                let state = self.state.lock();
                (state.duration_on_use, state.duration)
            };
            if duration_on_use != 0 && duration != -1 {
                let mut state = self.state.lock();
                state.duration += duration_on_use;
                if state.duration <= 0 {
                    drop(state);
                    self.set_removed(RemovalReason::Discarded);
                    return;
                }
            }
        }
    }
}

impl Entity for AreaEffectCloudEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        self.default_tick();
        let Some(world) = self.level() else {
            return;
        };
        // Particles are client-local in vanilla; the server only ticks gameplay.
        self.server_tick(&world);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        // Vanilla `getDimensions` uses `EntityDimensions.scalable`, whose eye
        // height is the shared default for the box height.
        EntityDimensions::with_default_eye_height(self.radius() * 2.0, CLOUD_HEIGHT)
    }

    fn piston_push_reaction(&self) -> PushReaction {
        PushReaction::IgnoreEntity
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn attackable(&self) -> bool {
        false
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        // Vanilla `hurtServer` always returns false for the cloud.
        false
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        // Read the age before taking the state lock: `age` locks it too.
        let age = self.age();
        let state = self.state.lock();
        let entity_data = self.entity_data.lock();

        nbt.insert("Age", age);
        nbt.insert("Duration", state.duration);
        nbt.insert("WaitTime", state.wait_time);
        nbt.insert("ReapplicationDelay", state.reapplication_delay);
        nbt.insert("DurationOnUse", state.duration_on_use);
        nbt.insert("RadiusOnUse", state.radius_on_use);
        nbt.insert("RadiusPerTick", state.radius_per_tick);
        nbt.insert("Radius", *entity_data.radius.get());
        if let Some(owner) = &state.owner {
            owner.store(nbt, "Owner");
        }
        // `custom_particle` has no Steel NBT codec; intentionally not persisted.
        if state.contents != PotionContents::empty() {
            nbt.insert("potion_contents", state.contents.clone().to_nbt_tag());
        }
        if state.potion_duration_scale != 1.0 {
            nbt.insert("potion_duration_scale", state.potion_duration_scale);
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        // Vanilla restores `Age` into `tickCount`; the base entity does not
        // persist its tick counter, so keep the loaded age as an offset on top
        // of it (see `CloudState::age_offset`).
        let contents = nbt
            .get("potion_contents")
            .and_then(PotionContents::from_nbt_tag)
            .unwrap_or_else(PotionContents::empty);
        let scale = nbt.float("potion_duration_scale").unwrap_or(1.0);
        let loaded_age = nbt.int("Age").unwrap_or(0);
        let current_ticks = self.tick_count();
        {
            let mut state = self.state.lock();
            state.age_offset = loaded_age.wrapping_sub(current_ticks);
            state.duration = nbt.int("Duration").unwrap_or(-1);
            state.wait_time = nbt.int("WaitTime").unwrap_or(DEFAULT_WAIT_TIME);
            state.reapplication_delay = nbt
                .int("ReapplicationDelay")
                .unwrap_or(DEFAULT_REAPPLICATION_DELAY);
            state.duration_on_use = nbt.int("DurationOnUse").unwrap_or(0);
            state.radius_on_use = nbt.float("RadiusOnUse").unwrap_or(0.0);
            state.radius_per_tick = nbt.float("RadiusPerTick").unwrap_or(0.0);
            state.owner = EntityReference::read(&nbt, "Owner");
            state.custom_particle = None;
            state.contents = contents;
            state.potion_duration_scale = scale;
        }

        self.entity_data
            .lock()
            .radius
            .set(nbt.float("Radius").unwrap_or(DEFAULT_RADIUS));

        self.update_particle();
    }
}
