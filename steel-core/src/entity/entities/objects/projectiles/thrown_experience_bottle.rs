//! Thrown experience bottle (`ThrownExperienceBottle`).
//!
//! Mirrors vanilla `ThrownExperienceBottle`: on impact it plays the bottle break
//! events, awards 3-11 experience as directional orbs, and discards itself.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::level_events::{PARTICLES_SPELL_POTION_SPLASH, SOUND_SPELL_POTION_SPLASH};
use steel_registry::vanilla_entity_data::ExperienceBottleEntityData;
use steel_registry::{vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::entities::ExperienceOrbEntity;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, Projectile, ProjectileBase,
    ProjectileHit, RemovalReason, SharedEntity, ThrowableItemProjectile, ThrowableProjectile,
    next_entity_id,
};
use crate::world::World;

/// Vanilla `ThrownExperienceBottle.getDefaultGravity`.
const EXPERIENCE_BOTTLE_GRAVITY: f64 = 0.07;

/// Vanilla `ThrownExperienceBottle` break event color: the default potion color.
const EXPERIENCE_BOTTLE_BREAK_COLOR: i32 = -13_083_194;

/// Vanilla `ThrownExperienceBottle` base experience before the two rolls.
const EXPERIENCE_BASE: i32 = 3;

/// Vanilla `ThrownExperienceBottle` roll bound for each of the two rolls.
const EXPERIENCE_ROLL_BOUND: i32 = 5;

/// Vanilla `ThrowableItemProjectile` spawn offset below the thrower's eye.
const THROWN_ITEM_SPAWN_EYE_OFFSET: f64 = 0.1;

/// A thrown experience bottle.
#[entity_behavior(class = "ThrownExperienceBottle")]
pub struct ThrownExperienceBottleEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data carrying the rendered item stack.
    entity_data: SyncMutex<ExperienceBottleEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies
// `ThrownExperienceBottleEntity`.
unsafe impl DowncastType for ThrownExperienceBottleEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/thrown_experience_bottle");
}

impl ThrownExperienceBottleEntity {
    /// Creates a new thrown experience bottle with no owner and the default item.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ExperienceBottleEntityData::new()),
            projectile_base: ProjectileBase::new(),
        }
    }

    /// Creates a thrown experience bottle from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(ExperienceBottleEntityData::new()),
            projectile_base: ProjectileBase::new(),
        }
    }

    /// Creates an experience bottle thrown by `owner`, mirroring vanilla
    /// `ThrownExperienceBottle(Level, LivingEntity, ItemStack)`.
    #[must_use]
    pub fn new_from_owner(world: &Arc<World>, owner: &SharedEntity, stack: ItemStack) -> Self {
        let owner_position = owner.position();
        let entity = Self::new(
            &vanilla_entities::EXPERIENCE_BOTTLE,
            next_entity_id(),
            DVec3::new(
                owner_position.x,
                owner.get_eye_y() - THROWN_ITEM_SPAWN_EYE_OFFSET,
                owner_position.z,
            ),
            Arc::downgrade(world),
        );
        entity.set_item_clamped(stack);
        entity.set_owner_entity(Some(owner));
        entity
    }

    /// Vanilla `3 + random.nextInt(5) + random.nextInt(5)`.
    fn roll_experience() -> i32 {
        EXPERIENCE_BASE
            + rand::random_range(0..EXPERIENCE_ROLL_BOUND)
            + rand::random_range(0..EXPERIENCE_ROLL_BOUND)
    }
}

impl Entity for ThrownExperienceBottleEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        self.throwable_projectile_tick();
    }

    fn get_default_gravity(&self) -> f64 {
        EXPERIENCE_BOTTLE_GRAVITY
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
        false
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_throwable_item(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_throwable_item(nbt);
    }
}

impl Projectile for ThrownExperienceBottleEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);

        let Some(world) = self.level() else {
            return;
        };
        let block_pos = self.block_position();
        world.level_event(
            PARTICLES_SPELL_POTION_SPLASH,
            block_pos,
            EXPERIENCE_BOTTLE_BREAK_COLOR,
            None,
        );
        if !self.is_silent() {
            world.level_event(SOUND_SPELL_POTION_SPLASH, block_pos, 0, None);
        }

        // Vanilla `ExperienceOrb.awardWithDirection(level, hitLocation,
        // direction, xpCount)`; Steel's orb spawner only has the
        // direction-less `award` (see the reported gap), so orbs spawn with
        // vanilla randomization but without the surface-facing velocity bias.
        ExperienceOrbEntity::award(&world, hit.location(), Self::roll_experience());

        self.set_removed(RemovalReason::Discarded);
    }
}

impl ThrowableProjectile for ThrownExperienceBottleEntity {}

impl ThrowableItemProjectile for ThrownExperienceBottleEntity {
    fn get_default_item(&self) -> ItemRef {
        &vanilla_items::EXPERIENCE_BOTTLE
    }

    fn set_item(&self, item: ItemStack) {
        self.entity_data
            .lock()
            .throwable_item_projectile
            .item_stack
            .set(item);
    }

    fn get_item(&self) -> ItemStack {
        self.entity_data
            .lock()
            .throwable_item_projectile
            .item_stack
            .get()
            .clone()
    }
}
