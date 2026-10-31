//! Dragon fireball projectile entity (`DragonFireball`).
//!
//! Mirrors vanilla `DragonFireball`: no direct impact damage and no fire; on any
//! non-owner hit it splashes dragon breath at the impact block and discards.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::MobEffectInstance;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::level_events;
use steel_registry::particle_type::{ParticleData, PowerParticleOption};
use steel_registry::vanilla_entity_data::DragonFireballEntityData;
use steel_registry::{vanilla_mob_effects, vanilla_particle_types};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::hurting::{
    HurtingProjectile, HurtingProjectileState,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, Projectile, ProjectileBase,
    ProjectileHit, RemovalReason, SharedEntity,
};
use crate::world::World;

use super::area_effect_cloud::AreaEffectCloudEntity;

/// Vanilla `DragonFireball.SPLASH_RANGE`: the cloud search inflates 4 blocks
/// horizontally and 2 vertically.
const SPLASH_RANGE_XZ: f64 = 4.0;
const SPLASH_RANGE_Y: f64 = 2.0;
/// Vanilla repositions the cloud onto a living entity closer than 4 blocks.
const SPLASH_RADIUS_SQUARED: f64 = 16.0;

/// Vanilla cloud configuration: radius 3 growing to 7 over 600 ticks.
const CLOUD_RADIUS: f32 = 3.0;
const CLOUD_MAX_RADIUS: f32 = 7.0;
const CLOUD_DURATION: i32 = 600;
const CLOUD_POTION_DURATION_SCALE: f32 = 0.25;

/// A dragon fireball.
#[entity_behavior(class = "DragonFireball")]
pub struct DragonFireballEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data layer declared by this entity (no fields of its own).
    entity_data: SyncMutex<DragonFireballEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Shared `AbstractHurtingProjectile` accelerated-movement state.
    hurting: HurtingProjectileState,
}

// SAFETY: This key is owned by Steel and uniquely identifies `DragonFireballEntity`.
unsafe impl DowncastType for DragonFireballEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/dragon_fireball");
}

impl DragonFireballEntity {
    /// Creates a dragon fireball with no owner.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(DragonFireballEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
        }
    }

    /// Creates a dragon fireball moving along `direction` (vanilla
    /// `AbstractHurtingProjectile(type, x, y, z, direction, level)`).
    #[must_use]
    pub fn with_direction(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        direction: DVec3,
        world: Weak<World>,
    ) -> Self {
        let fireball = Self::new(entity_type, id, position, world);
        fireball.assign_directional_movement(direction, fireball.acceleration_power());
        fireball
    }

    /// Creates a dragon fireball from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(DragonFireballEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
        }
    }
}

impl Entity for DragonFireballEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        Arc::clone(&self).hurting_projectile_tick();
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

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        // Vanilla `AbstractHurtingProjectile.hurtServer` never accepts damage.
        false
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_hurting_projectile(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_hurting_projectile(nbt);
    }
}

impl Projectile for DragonFireballEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        self.hurting_can_hit_entity(entity)
    }

    fn on_deflection(&self, by_attack: bool) {
        self.hurting_on_deflection(by_attack);
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);

        // Vanilla skips the splash when the fireball hit its own owner.
        if let ProjectileHit::Entity(entity_hit) = hit
            && self.owned_by(entity_hit.entity.as_ref())
        {
            return;
        }

        let Some(world) = self.level() else {
            return;
        };
        self.spawn_dragon_breath_cloud(&world);
        world.level_event(
            level_events::PARTICLES_DRAGON_FIREBALL_SPLASH,
            BlockPos::from(self.position()),
            if self.is_silent() { -1 } else { 1 },
            None,
        );
        self.set_removed(RemovalReason::Discarded);
    }
}

impl DragonFireballEntity {
    /// Vanilla `DragonFireball.onHit` cloud: radius 3 growing to 7 over 600 ticks,
    /// dragon-breath particles, instant damage II, and repositioned onto the nearest
    /// living entity within 16 blocks of the impact.
    fn spawn_dragon_breath_cloud(self: &Arc<Self>, world: &Arc<World>) {
        let position = self.position();
        let cloud = AreaEffectCloudEntity::new_at_position(world, position);
        if let Some(owner) = self
            .get_owner()
            .filter(|owner| owner.as_living_entity().is_some())
        {
            cloud.set_owner(&owner);
        }
        cloud.set_custom_particle(Some(ParticleData::new(
            &vanilla_particle_types::DRAGON_BREATH,
            PowerParticleOption::new(1.0),
        )));
        cloud.set_radius(CLOUD_RADIUS);
        cloud.set_duration(CLOUD_DURATION);
        cloud.set_radius_per_tick((CLOUD_MAX_RADIUS - CLOUD_RADIUS) / CLOUD_DURATION as f32);
        cloud.set_potion_duration_scale(CLOUD_POTION_DURATION_SCALE);
        cloud.add_effect(MobEffectInstance::new(
            vanilla_mob_effects::INSTANT_DAMAGE,
            1,
            1,
            false,
            true,
            true,
            None,
        ));

        let search =
            self.bounding_box()
                .inflate_xyz(SPLASH_RANGE_XZ, SPLASH_RANGE_Y, SPLASH_RANGE_XZ);
        for entity in world.get_entities_in_aabb_matching(&search, Entity::is_living_entity) {
            if self.distance_to_sqr(entity.position()) < SPLASH_RADIUS_SQUARED {
                if let Err(error) = cloud.try_set_position(entity.position()) {
                    log::debug!("failed to move dragon breath cloud: {error}");
                }
                break;
            }
        }

        let cloud: SharedEntity = Arc::clone(&cloud) as SharedEntity;
        if let Err(error) = world.try_add_entity(cloud) {
            log::debug!("failed to spawn dragon breath cloud: {error}");
        }
    }
}

impl HurtingProjectile for DragonFireballEntity {
    fn hurting_state(&self) -> &HurtingProjectileState {
        &self.hurting
    }

    fn should_burn(&self) -> bool {
        false
    }

    /// Vanilla `DragonFireball.getTrailParticle` uses a dragon-breath power
    /// particle.
    fn get_trail_particle(&self) -> Option<ParticleData> {
        Some(ParticleData::new(
            &vanilla_particle_types::DRAGON_BREATH,
            PowerParticleOption::new(1.0),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::entity::Entity;
    use crate::world::World;

    use super::DragonFireballEntity;

    #[test]
    fn directional_constructor_uses_the_vanilla_initial_power() {
        init_vanilla_registry();

        let fireball = DragonFireballEntity::with_direction(
            &vanilla_entities::DRAGON_FIREBALL,
            1,
            DVec3::ZERO,
            DVec3::Y,
            Weak::<World>::new(),
        );

        let velocity = fireball.velocity();
        assert!((velocity.y - 0.1).abs() < 1.0e-9);
        assert!(velocity.x.abs() < 1.0e-9 && velocity.z.abs() < 1.0e-9);
    }
}
