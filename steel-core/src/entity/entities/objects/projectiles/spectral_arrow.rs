//! Spectral arrow projectile entity (`SpectralArrow`).
//!
//! Mirrors vanilla `SpectralArrow`: an arrow that applies `Glowing` to whatever
//! it hits for a duration stored in its own `Duration` NBT field. It has no
//! potion contents.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{AbstractArrowEntityData, SpectralArrowEntityData};
use steel_registry::{sound_events, vanilla_entities, vanilla_items, vanilla_mob_effects};
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::entities::objects::projectiles::abstract_arrow::{
    AbstractArrow, AbstractArrowBase,
};
use crate::entity::potion_contents::to_runtime_instance;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, Projectile, ProjectileBase, SharedEntity,
};
use crate::world::{ClipHitResult, World};

/// Vanilla `AbstractArrow.getDefaultGravity`.
const ARROW_GRAVITY: f64 = 0.05;

/// Vanilla `SpectralArrow.DEFAULT_DURATION`.
const DEFAULT_DURATION: i32 = 200;

/// A spectral arrow.
#[entity_behavior(class = "SpectralArrow")]
pub struct SpectralArrowEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced `AbstractArrow` data.
    entity_data: SyncMutex<SpectralArrowEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Non-synced `AbstractArrow` state (life, pickup, shake, damage, …).
    arrow_base: AbstractArrowBase,
    /// Vanilla `SpectralArrow.duration`: glowing duration applied on hit.
    duration: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SpectralArrowEntity`.
unsafe impl DowncastType for SpectralArrowEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/spectral_arrow");
}

impl SpectralArrowEntity {
    /// Creates a new spectral arrow with no owner and a default pickup stack.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(SpectralArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            arrow_base: AbstractArrowBase::new(
                ItemStack::new(&vanilla_items::SPECTRAL_ARROW),
                &sound_events::ENTITY_ARROW_HIT,
            ),
            duration: SyncMutex::new(DEFAULT_DURATION),
        }
    }

    /// Creates a spectral arrow from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(SpectralArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            arrow_base: AbstractArrowBase::new(
                ItemStack::new(&vanilla_items::SPECTRAL_ARROW),
                &sound_events::ENTITY_ARROW_HIT,
            ),
            duration: SyncMutex::new(DEFAULT_DURATION),
        }
    }

    /// Returns the glowing duration applied on hit (vanilla `duration`).
    #[must_use]
    pub fn duration(&self) -> i32 {
        *self.duration.lock()
    }

    /// Sets the glowing duration applied on hit.
    pub fn set_duration(&self, duration: i32) {
        *self.duration.lock() = duration;
    }
}

impl Entity for SpectralArrowEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        Arc::clone(&self).abstract_arrow_tick();
    }

    fn get_default_gravity(&self) -> f64 {
        ARROW_GRAVITY
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn spawn_data(&self) -> i32 {
        self.get_owner().map_or(0, |owner| owner.id())
    }

    fn restore_owner_reference(&self, owner: &SharedEntity) {
        self.cache_owner_entity(owner);
    }

    fn projectile_owner_uuid(&self) -> Option<uuid::Uuid> {
        self.owner_uuid()
    }

    fn projectile_owner(&self) -> Option<SharedEntity> {
        self.get_owner()
    }

    fn attackable(&self) -> bool {
        self.arrow_is_attackable()
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_arrow(nbt);
        nbt.insert("Duration", self.duration());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_arrow(nbt);
        self.set_duration(nbt.int("Duration").unwrap_or(DEFAULT_DURATION));
    }
}

impl Projectile for SpectralArrowEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, location: DVec3) {
        self.abstract_arrow_on_hit_entity(entity, location);
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        self.abstract_arrow_on_hit_block(hit);
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        self.arrow_can_hit_entity(entity)
    }

    fn should_bounce_on_world_border(&self) -> bool {
        self.arrow_should_bounce_on_world_border()
    }
}

impl AbstractArrow for SpectralArrowEntity {
    fn abstract_arrow_base(&self) -> &AbstractArrowBase {
        &self.arrow_base
    }

    fn with_arrow_data<R>(&self, f: impl FnOnce(&mut AbstractArrowEntityData) -> R) -> R {
        f(&mut self.entity_data.lock().abstract_arrow)
    }

    fn get_default_pickup_item(&self) -> ItemRef {
        &vanilla_items::SPECTRAL_ARROW
    }

    fn do_post_hurt_effects(&self, mob: &SharedEntity) {
        // Vanilla `SpectralArrow.doPostHurtEffects`: glowing for `duration`.
        let Some(living) = mob.as_living_entity() else {
            return;
        };
        let effect = steel_registry::MobEffectInstance::simple(
            &vanilla_mob_effects::GLOWING,
            self.duration(),
            0,
        );
        living.add_mob_effect(to_runtime_instance(&effect, self.duration()));
    }
}
