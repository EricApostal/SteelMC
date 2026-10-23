//! Evoker fangs entity (`net.minecraft.world.entity.projectile.EvokerFangs`).
//!
//! Vanilla `EvokerFangs` extends plain `Entity` (not `Projectile`) and tracks its owner
//! through `TraceableEntity`, so this is a standalone entity holding an
//! [`EntityReference`] rather than a projectile. The evoker's attack spell places a line
//! or arc of fangs; each fang warms up, snaps shut once (magic damage in a small box),
//! then expires.
//!
//! Deliberate difference: vanilla broadcasts entity event byte 4 and renders the spike
//! animation, its `CRIT` particles and the `EVOKER_FANGS_ATTACK` sound on the client from
//! `handleEntityEvent`/client `tick`. Steel has no client, so only the server half runs and
//! the attack sound is left to the client-side event handler.

use std::mem;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_damage_types;
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityReference, RemovalReason, SharedEntity,
};
use crate::world::World;

/// Vanilla `EvokerFangs.ATTACK_DURATION`: the total spike animation length in ticks.
pub const ATTACK_DURATION: i32 = 20;
/// Vanilla `EvokerFangs.LIFE_OFFSET`: ticks between the attack and the animation start.
pub const LIFE_OFFSET: i32 = 2;
/// Vanilla `EvokerFangs.ATTACK_TRIGGER_TICKS`: tick offset used by the animation.
pub const ATTACK_TRIGGER_TICKS: i32 = 14;

/// Vanilla `EvokerFangs.dealDamageTo` damage.
const FANG_DAMAGE: f32 = 6.0;
/// Vanilla `EvokerFangs.lifeTicks` initial value.
const DEFAULT_LIFE_TICKS: i32 = 22;
/// Vanilla `EvokerFangs.DEFAULT_WARMUP_DELAY`.
const DEFAULT_WARMUP_DELAY: i32 = 0;
/// Vanilla damages every living entity inside `getBoundingBox().inflate(0.2, 0.0, 0.2)`.
const DAMAGE_INFLATE: f64 = 0.2;
/// Vanilla snaps shut once the warmup countdown reaches this value.
const DAMAGE_WARMUP_TICK: i32 = -8;

/// An evoker fang.
#[entity_behavior(class = "EvokerFangs")]
pub struct EvokerFangsEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    /// Vanilla `EvokerFangs.warmupDelayTicks`.
    warmup_delay_ticks: SyncMutex<i32>,
    /// Vanilla `EvokerFangs.lifeTicks`.
    life_ticks: SyncMutex<i32>,
    /// Vanilla `EvokerFangs.sentSpikeEvent`.
    sent_spike_event: SyncMutex<bool>,
    /// Vanilla `TraceableEntity.owner`.
    owner: SyncMutex<Option<EntityReference>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `EvokerFangsEntity`.
unsafe impl DowncastType for EvokerFangsEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/evoker_fangs");
}

impl EvokerFangsEntity {
    /// Creates a new evoker fang with the default warmup delay and no owner.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::with_spell_state(entity_type, id, position, DEFAULT_WARMUP_DELAY, None, world)
    }

    /// Loads a saved evoker fang entity.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            warmup_delay_ticks: SyncMutex::new(DEFAULT_WARMUP_DELAY),
            life_ticks: SyncMutex::new(DEFAULT_LIFE_TICKS),
            sent_spike_event: SyncMutex::new(false),
            owner: SyncMutex::new(None),
        }
    }

    /// Vanilla `new EvokerFangs(level, x, y, z, rotationRadians, warmupDelayTicks, owner)`.
    ///
    /// `rotation_radians` matches vanilla's constructor: it is converted to degrees
    /// before being stored as the entity's yaw.
    #[must_use]
    pub fn new_spell(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        rotation_radians: f32,
        warmup_delay_ticks: i32,
        owner: Option<&SharedEntity>,
        world: Weak<World>,
    ) -> Self {
        let entity =
            Self::with_spell_state(entity_type, id, position, warmup_delay_ticks, owner, world);
        entity
            .base
            .set_rotation((rotation_radians.to_degrees(), 0.0));
        entity
    }

    fn with_spell_state(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        warmup_delay_ticks: i32,
        owner: Option<&SharedEntity>,
        world: Weak<World>,
    ) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            warmup_delay_ticks: SyncMutex::new(warmup_delay_ticks),
            life_ticks: SyncMutex::new(DEFAULT_LIFE_TICKS),
            sent_spike_event: SyncMutex::new(false),
            owner: SyncMutex::new(owner.map(EntityReference::from_entity)),
        }
    }

    /// Vanilla `EvokerFangs.setOwner`.
    pub fn set_owner(&self, owner: Option<&SharedEntity>) {
        *self.owner.lock() = owner.map(EntityReference::from_entity);
    }

    /// Vanilla `EvokerFangs.getOwner`.
    #[must_use]
    pub fn get_owner(&self) -> Option<SharedEntity> {
        let world = self.level()?;
        let owner = self.owner.lock().clone()?;
        owner.get_entity(&world)
    }

    /// Vanilla `EvokerFangs.getAnimationProgress`.
    #[must_use]
    pub fn animation_progress(&self, partial_tick: f32) -> f32 {
        // Vanilla gates on `clientSideAttackStarted`, which the client sets from the
        // broadcast spike event; the server's `sentSpikeEvent` flips at the same tick.
        if !*self.sent_spike_event.lock() {
            return 0.0;
        }
        let remaining_life = *self.life_ticks.lock() - LIFE_OFFSET;
        if remaining_life <= 0 {
            1.0
        } else {
            1.0 - (remaining_life as f32 - partial_tick) / ATTACK_DURATION as f32
        }
    }

    /// Vanilla `EvokerFangs.dealDamageTo`.
    fn deal_damage_to(&self, entity: &SharedEntity, fangs: &SharedEntity) {
        if !entity.is_alive() || entity.is_invulnerable() {
            return;
        }

        let Some(owner) = self.get_owner() else {
            let Some(world) = entity.level() else {
                return;
            };
            entity.hurt(
                &world,
                &DamageSource::environment(&vanilla_damage_types::MAGIC),
                FANG_DAMAGE,
            );
            return;
        };

        if entity.uuid() == owner.uuid() || owner.is_allied_to(entity.as_ref()) {
            return;
        }

        let Some(world) = self.level() else {
            return;
        };
        let source = DamageSource::environment(&vanilla_damage_types::INDIRECT_MAGIC)
            .with_direct_entity(Arc::clone(fangs))
            .with_causing_entity(owner);
        entity.hurt(&world, &source, FANG_DAMAGE);
    }

    /// Vanilla `EvokerFangs.tick` server branch.
    fn server_tick(self: &Arc<Self>) {
        let warmup = {
            let mut warmup = self.warmup_delay_ticks.lock();
            *warmup -= 1;
            *warmup
        };

        if warmup >= 0 {
            return;
        }

        if warmup == DAMAGE_WARMUP_TICK {
            let Some(world) = self.level() else {
                return;
            };
            let fangs: SharedEntity = Arc::clone(self) as SharedEntity;
            let search_box = self.bounding_box().inflate(DAMAGE_INFLATE);
            for entity in world.get_entities_in_aabb_matching(&search_box, |entity| {
                entity.as_living_entity().is_some()
            }) {
                self.deal_damage_to(&entity, &fangs);
            }
        }

        let already_sent = {
            let mut sent = self.sent_spike_event.lock();
            mem::replace(&mut *sent, true)
        };
        if !already_sent {
            // Vanilla `level().broadcastEntityEvent(this, (byte) 4)`; Steel's shared
            // `EntityStatus` byte 4 is `StartAttacking`.
            self.broadcast_entity_event(EntityStatus::StartAttacking);
        }

        let life = {
            let mut life = self.life_ticks.lock();
            *life -= 1;
            *life
        };
        if life < 0 {
            self.set_removed(RemovalReason::Discarded);
        }
    }
}

impl Entity for EvokerFangsEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `EvokerFangs.tick` starts with `super.tick()`.
        self.default_tick();
        self.server_tick();
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        // Vanilla `EvokerFangs.addAdditionalSaveData`: `Warmup` and the `Owner` reference.
        nbt.insert("Warmup", *self.warmup_delay_ticks.lock());
        if let Some(owner) = self.owner.lock().as_ref() {
            owner.store(nbt, "Owner");
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        // Vanilla `EvokerFangs.readAdditionalSaveData`.
        *self.warmup_delay_ticks.lock() = nbt.int("Warmup").unwrap_or(DEFAULT_WARMUP_DELAY);
        *self.owner.lock() = EntityReference::read(&nbt, "Owner");
    }
}

/// `EvokerFangs` has no synchronized entity data in vanilla (`defineSynchedData` is empty).
const _: () = {
    assert!(DEFAULT_LIFE_TICKS == ATTACK_DURATION + LIFE_OFFSET);
    assert!(ATTACK_TRIGGER_TICKS < ATTACK_DURATION);
};
