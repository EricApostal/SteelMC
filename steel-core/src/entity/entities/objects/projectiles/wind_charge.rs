//! Wind charge projectile entity (`WindCharge`).
//!
//! Mirrors vanilla `WindCharge`: a fast, gravity-free `AbstractHurtingProjectile`
//! with zero acceleration power that deals 1 impact damage and then bursts,
//! knocking entities back without damaging them and never destroying blocks.
//! The first five ticks refuse deflection (vanilla `noDeflectTicks`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_entity_data::WindChargeEntityData;
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
    LivingEntity as _, Projectile, ProjectileBase, ProjectileDeflection, ProjectileHit,
    RemovalReason, SharedEntity,
};
use crate::world::{ClipHitResult, World};

/// Vanilla `WindCharge.RADIUS`.
const EXPLOSION_RADIUS: f32 = 1.2;

/// Vanilla `WindCharge.EXPLOSION_DAMAGE_CALCULATOR` knockback multiplier.
const KNOCKBACK_MULTIPLIER: f32 = 1.22;

/// Vanilla `WindCharge.noDeflectTicks`.
const NO_DEFLECT_TICKS: i32 = 5;

/// Vanilla `AbstractWindCharge.onHitEntity` impact damage.
const IMPACT_DAMAGE: f32 = 1.0;

/// Vanilla `AbstractWindCharge.makeBoundingBox` vertical offset.
const BOX_OFFSET: f64 = 0.15;

/// Vanilla `AbstractWindCharge` upper bound above the world height before bursting.
const MAX_Y_OFFSET: i32 = 30;

/// A thrown wind charge.
#[entity_behavior(class = "WindCharge")]
pub struct WindChargeEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data layer declared by this entity (no fields of its own).
    entity_data: SyncMutex<WindChargeEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Shared `AbstractHurtingProjectile` accelerated-movement state.
    hurting: HurtingProjectileState,
    /// Remaining ticks during which deflection is refused.
    no_deflect_ticks: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `WindChargeEntity`.
unsafe impl DowncastType for WindChargeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/wind_charge");
}

impl WindChargeEntity {
    /// Creates a wind charge with no owner.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(WindChargeEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::with_acceleration_power(0.0),
            no_deflect_ticks: SyncMutex::new(NO_DEFLECT_TICKS),
        }
    }

    /// Creates a wind charge owned by `owner` at `position` (vanilla
    /// `WindCharge(Player, Level, x, y, z)`).
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

    /// Creates a wind charge moving along `direction` (vanilla
    /// `WindCharge(Level, x, y, z, direction)`).
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

    /// Creates a wind charge from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(WindChargeEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::with_acceleration_power(0.0),
            no_deflect_ticks: SyncMutex::new(NO_DEFLECT_TICKS),
        }
    }

    /// Vanilla `WindCharge.explode`: a trigger explosion that knocks entities back
    /// (1.22x) without damaging them and never breaks blocks.
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
            knockback_multiplier: Some(KNOCKBACK_MULTIPLIER),
            small_particles: ParticleData::simple(&vanilla_particle_types::GUST_EMITTER_SMALL),
            large_particles: ParticleData::simple(&vanilla_particle_types::GUST_EMITTER_LARGE),
            sound: &sound_events::ENTITY_WIND_CHARGE_WIND_BURST,
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

impl Entity for WindChargeEntity {
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

        let mut no_deflect_ticks = self.no_deflect_ticks.lock();
        if *no_deflect_ticks > 0 {
            *no_deflect_ticks -= 1;
        }
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
        if is_wind_charge(other) {
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

impl Projectile for WindChargeEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        // Vanilla `AbstractWindCharge.canHitEntity`: never wind charges or end crystals.
        if is_wind_charge(entity) || entity.entity_type() == &vanilla_entities::END_CRYSTAL {
            return false;
        }
        self.hurting_can_hit_entity(entity)
    }

    fn deflect(
        &self,
        deflection: ProjectileDeflection,
        deflecting_entity: Option<&dyn Entity>,
        new_owner: Option<crate::entity::EntityReference>,
        by_attack: bool,
    ) -> bool {
        // Vanilla `WindCharge.deflect`: refuse deflection during the first ticks.
        if *self.no_deflect_ticks.lock() > 0 {
            return false;
        }
        super::hurting::base_deflect(self, deflection, deflecting_entity, new_owner, by_attack)
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

/// Returns whether `entity` is one of the two vanilla `AbstractWindCharge` types.
pub(super) fn is_wind_charge(entity: &dyn Entity) -> bool {
    entity.entity_type() == &vanilla_entities::WIND_CHARGE
        || entity.entity_type() == &vanilla_entities::BREEZE_WIND_CHARGE
}

impl HurtingProjectile for WindChargeEntity {
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

    use super::WindChargeEntity;

    #[test]
    fn wind_charges_have_no_acceleration_power() {
        init_vanilla_registry();

        let wind_charge = WindChargeEntity::with_direction(
            &vanilla_entities::WIND_CHARGE,
            1,
            DVec3::ZERO,
            DVec3::Z,
            Weak::<World>::new(),
        );

        assert!(wind_charge.velocity().length() < 1.0e-12);
    }
}
