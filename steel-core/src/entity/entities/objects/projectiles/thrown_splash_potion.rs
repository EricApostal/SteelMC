//! Thrown splash potion (`ThrownSplashPotion`).
//!
//! Mirrors vanilla `ThrownSplashPotion`: the shared
//! [`AbstractThrownPotion`] half runs on impact, then `onHitAsPotion` applies
//! the potion's effects to every living entity within four blocks, scaled by the
//! distance to the impact.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::SplashPotionEntityData;
use steel_registry::{vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use super::thrown_potion::{
    AbstractThrownPotion, SPLASH_RANGE, SPLASH_RANGE_SQ, THROWN_ITEM_SPAWN_EYE_OFFSET,
    potion_contents, potion_duration_scale, splash_effect_box,
};
use crate::behavior::MOB_EFFECT_BEHAVIORS;
use crate::entity::potion_contents::to_runtime_instance;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource as _, EntitySyncedData, Projectile,
    ProjectileBase, ProjectileHit, SharedEntity, ThrowableItemProjectile, ThrowableProjectile,
    next_entity_id,
};
use crate::world::{ClipHitResult, World};

/// Vanilla `MobEffectInstance.endsWithin` threshold used by splash potions.
const MIN_EFFECT_DURATION_TICKS: i32 = 20;

/// A thrown splash potion.
#[entity_behavior(class = "ThrownSplashPotion")]
pub struct ThrownSplashPotionEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data carrying the rendered item stack.
    entity_data: SyncMutex<SplashPotionEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies
// `ThrownSplashPotionEntity`.
unsafe impl DowncastType for ThrownSplashPotionEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/thrown_splash_potion");
}

impl ThrownSplashPotionEntity {
    /// Creates a new thrown splash potion with no owner and the default item.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(SplashPotionEntityData::new()),
            projectile_base: ProjectileBase::new(),
        }
    }

    /// Creates a thrown splash potion from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(SplashPotionEntityData::new()),
            projectile_base: ProjectileBase::new(),
        }
    }

    /// Creates a splash potion thrown by `owner`, mirroring vanilla
    /// `ThrownSplashPotion(Level, LivingEntity owner, ItemStack)`.
    ///
    /// The projectile spawns at the owner's eye minus 0.1 and carries the
    /// potion stack (count clamped to one).
    #[must_use]
    pub fn new_from_owner(world: &Arc<World>, owner: &SharedEntity, stack: ItemStack) -> Self {
        let owner_position = owner.position();
        let entity = Self::new(
            &vanilla_entities::SPLASH_POTION,
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
}

impl Entity for ThrownSplashPotionEntity {
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
        self.thrown_potion_default_gravity()
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

impl Projectile for ThrownSplashPotionEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        self.on_hit_block_thrown_potion(hit);
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        self.on_hit_thrown_potion(hit);
    }
}

impl ThrowableProjectile for ThrownSplashPotionEntity {}

impl ThrowableItemProjectile for ThrownSplashPotionEntity {
    fn get_default_item(&self) -> ItemRef {
        &vanilla_items::SPLASH_POTION
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

impl AbstractThrownPotion for ThrownSplashPotionEntity {
    /// Vanilla `ThrownSplashPotion.onHitAsPotion`: every living entity in the
    /// splash box gets each effect scaled by `1 - distance / 4`.
    fn on_hit_as_potion(
        self: Arc<Self>,
        world: &Arc<World>,
        potion_item: &ItemStack,
        hit: &ProjectileHit,
    ) {
        let contents = potion_contents(potion_item);
        let duration_scale = f64::from(potion_duration_scale(potion_item));
        let effects = contents.all_effects();
        if effects.is_empty() {
            return;
        }

        // Vanilla `ThrownSplashPotion`: the projectile box moved to the hit
        // location, inflated by the splash range.
        let hit_location = hit.location();
        let projectile = self.as_entity_event_source();
        let potion_box = self
            .bounding_box()
            .translate(hit_location - self.position());
        let owner = self.get_owner();
        let projectile_entity: SharedEntity = Arc::clone(&self) as SharedEntity;

        let entities = world.get_entities_in_aabb_matching(
            &splash_effect_box(projectile, hit_location),
            |entity| entity.as_living_entity().is_some(),
        );

        for entity in entities {
            let Some(living) = entity.as_living_entity() else {
                continue;
            };
            if !living.is_affected_by_potions() {
                continue;
            }

            let distance_sqr = potion_box.distance_to_sqr(entity.position());
            if distance_sqr >= SPLASH_RANGE_SQ {
                continue;
            }
            let falloff = 1.0 - distance_sqr.sqrt() / SPLASH_RANGE;

            for effect in &effects {
                let behavior = MOB_EFFECT_BEHAVIORS.get_behavior(effect.effect());
                if let Some(instantaneous) = behavior.as_instantaneous() {
                    instantaneous.apply_instantaneous(
                        world,
                        living,
                        effect.amplifier(),
                        Some(&projectile_entity),
                        owner.as_ref(),
                        falloff as f32,
                    );
                    continue;
                }

                // Vanilla `mapDuration(d -> (int)(scale * d * durationScale + 0.5))`:
                // unlike the cloud's `withScaledDuration`, splash rounds half up
                // and leaves sub-second results to the `endsWithin` filter below.
                let duration = if effect.duration() == -1 || effect.duration() == 0 {
                    effect.duration()
                } else {
                    (falloff * f64::from(effect.duration()) * duration_scale + 0.5) as i32
                };
                let new_effect = to_runtime_instance(effect, duration);
                if !new_effect.is_infinite_duration()
                    && new_effect.duration() <= MIN_EFFECT_DURATION_TICKS
                {
                    continue;
                }
                living.add_mob_effect(new_effect);
            }
        }
    }
}
