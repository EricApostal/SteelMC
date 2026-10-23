//! Breeze wind charge projectile entity (`BreezeWindCharge`).
//!
//! Mirrors vanilla `BreezeWindCharge`: identical to [`WindChargeEntity`] except
//! for a 3-block burst radius, the breeze burst sound, and the shared
//! `AbstractWindCharge` damage calculator (1.0 knockback instead of 1.22).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_entity_data::BreezeWindChargeEntityData;
use steel_registry::{
    sound_events, vanilla_damage_types, vanilla_entities, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey, WorldAabb};

use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::hurting::{
    ExplosionInteraction, ExplosionSpec, HurtingProjectile, HurtingProjectileState, explode,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource as _, EntitySyncedData,
    LivingEntity as _, Projectile, ProjectileBase, ProjectileHit, RemovalReason, SharedEntity,
};
use crate::world::{ClipHitResult, World};

/// Vanilla `BreezeWindCharge.RADIUS`.
const EXPLOSION_RADIUS: f32 = 3.0;

/// Vanilla `AbstractWindCharge.onHitEntity` impact damage.
const IMPACT_DAMAGE: f32 = 1.0;

/// Vanilla `AbstractWindCharge.makeBoundingBox` vertical offset.
const BOX_OFFSET: f64 = 0.15;

/// Vanilla `AbstractWindCharge` upper bound above the world height before bursting.
const MAX_Y_OFFSET: i32 = 30;

/// A wind charge fired by a breeze.
#[entity_behavior(class = "BreezeWindCharge")]
pub struct BreezeWindChargeEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data layer declared by this entity (no fields of its own).
    entity_data: SyncMutex<BreezeWindChargeEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Shared `AbstractHurtingProjectile` accelerated-movement state.
    hurting: HurtingProjectileState,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BreezeWindChargeEntity`.
unsafe impl DowncastType for BreezeWindChargeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/breeze_wind_charge");
}

impl BreezeWindChargeEntity {
    /// Creates a breeze wind charge with no owner.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(BreezeWindChargeEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::with_acceleration_power(0.0),
        }
    }

    /// Creates a breeze wind charge owned by `owner` at `position` (vanilla
    /// `BreezeWindCharge(Breeze, Level)`).
    #[must_use]
    pub fn with_owner(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        owner: &SharedEntity,
        world: Weak<World>,
    ) -> Self {
        let wind_charge = Self::new(entity_type, id, position, world);
        wind_charge.set_owner_entity(Some(owner));
        wind_charge
    }

    /// Creates a breeze wind charge moving along `direction` (vanilla
    /// `AbstractWindCharge(type, x, y, z, direction, level)`).
    #[must_use]
    pub fn with_direction(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        direction: DVec3,
        world: Weak<World>,
    ) -> Self {
        let wind_charge = Self::new(entity_type, id, position, world);
        wind_charge.assign_directional_movement(direction, wind_charge.acceleration_power());
        wind_charge
    }

    /// Creates a breeze wind charge from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(BreezeWindChargeEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::with_acceleration_power(0.0),
        }
    }

    /// Vanilla `BreezeWindCharge.explode`: 3-block trigger explosion, no block
    /// destruction and no explosion damage, only knockback.
    fn burst(self: &Arc<Self>, position: DVec3) {
        let Some(world) = self.level() else {
            return;
        };
        let spec = ExplosionSpec {
            source: Arc::clone(self) as SharedEntity,
            damage_source: None,
            center: position,
            radius: EXPLOSION_RADIUS,
            fire: false,
            interaction: ExplosionInteraction::Trigger,
            damages_entities: false,
            knockback_multiplier: None,
            small_particles: ParticleData::simple(&vanilla_particle_types::GUST_EMITTER_SMALL),
            large_particles: ParticleData::simple(&vanilla_particle_types::GUST_EMITTER_LARGE),
            sound: &sound_events::ENTITY_BREEZE_WIND_BURST,
        };
        explode(&world, &spec);
    }

    /// Vanilla `AbstractWindCharge.onHitBlock`: burst slightly off the hit face.
    fn burst_on_block(self: &Arc<Self>, hit: &ClipHitResult) {
        let (dx, dy, dz) = hit.direction.offset();
        let scaled = DVec3::new(f64::from(dx), f64::from(dy), f64::from(dz)) * 0.25;
        self.burst(hit.location + scaled);
        self.set_removed(RemovalReason::Discarded);
    }
}

impl Entity for BreezeWindChargeEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `AbstractWindCharge.tick`: burst when flying far above the world.
        if let Some(world) = self.level()
            && self.block_position().y() > world.get_max_y() + MAX_Y_OFFSET
        {
            self.burst(self.position());
            self.set_removed(RemovalReason::Discarded);
            return;
        }

        Arc::clone(&self).hurting_projectile_tick();
    }

    fn make_bounding_box_at(&self, position: DVec3) -> WorldAabb {
        // Vanilla `AbstractWindCharge.makeBoundingBox` sits 0.15 below the entity.
        let dimensions = self.base().dimensions();
        let half_width = f64::from(dimensions.width) / 2.0;
        let height = f64::from(dimensions.height);
        WorldAabb::new(
            position.x - half_width,
            position.y - BOX_OFFSET,
            position.z - half_width,
            position.x + half_width,
            position.y - BOX_OFFSET + height,
            position.z + half_width,
        )
    }

    fn can_collide_with(&self, other: &dyn Entity) -> bool {
        // Vanilla `AbstractWindCharge.canCollideWith`: wind charges ignore each other.
        if super::wind_charge::is_wind_charge(other) {
            return false;
        }
        other.can_be_collided_with(Some(self.as_entity_event_source()))
            && !self.is_passenger_of_same_vehicle(other)
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

impl Projectile for BreezeWindChargeEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        // Vanilla `AbstractWindCharge.canHitEntity`: never wind charges or end crystals.
        if super::wind_charge::is_wind_charge(entity)
            || entity.entity_type() == &vanilla_entities::END_CRYSTAL
        {
            return false;
        }
        self.hurting_can_hit_entity(entity)
    }

    fn on_deflection(&self, by_attack: bool) {
        self.hurting_on_deflection(by_attack);
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, _location: DVec3) {
        // Vanilla `AbstractWindCharge.onHitEntity`: 1 wind-charge damage, then burst.
        let owner = self.get_owner();
        if let Some(living_owner) = owner.as_ref().and_then(|owner| owner.as_living_entity()) {
            living_owner.set_last_hurt_mob(Some(entity));
        }

        let mut damage = DamageSource::environment(&vanilla_damage_types::WIND_CHARGE)
            .with_direct_entity(Arc::clone(&self) as SharedEntity);
        if let Some(owner) = owner {
            damage = damage.with_causing_entity(owner);
        }
        if let Some(world) = entity.level() {
            entity.hurt(&world, &damage, IMPACT_DAMAGE);
        }

        self.burst(self.position());
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        Arc::clone(&self).projectile_on_hit_block(hit);
        self.burst_on_block(hit);
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);
        self.set_removed(RemovalReason::Discarded);
    }
}

impl HurtingProjectile for BreezeWindChargeEntity {
    fn hurting_state(&self) -> &HurtingProjectileState {
        &self.hurting
    }

    fn get_inertia(&self) -> f32 {
        1.0
    }

    fn get_liquid_inertia(&self) -> f32 {
        1.0
    }

    fn should_burn(&self) -> bool {
        false
    }

    fn get_trail_particle(&self) -> Option<ParticleData> {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::entity::Entity;
    use crate::world::World;

    use super::BreezeWindChargeEntity;

    #[test]
    fn breeze_wind_charges_have_no_acceleration_power() {
        init_vanilla_registry();

        let wind_charge = BreezeWindChargeEntity::with_direction(
            &vanilla_entities::BREEZE_WIND_CHARGE,
            1,
            DVec3::ZERO,
            DVec3::X,
            Weak::<World>::new(),
        );

        assert!(wind_charge.velocity().length() < 1.0e-12);
    }
}
