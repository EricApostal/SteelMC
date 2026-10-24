//! Wither skull projectile entity (`WitherSkull`).
//!
//! Mirrors vanilla `WitherSkull`: a dangerous variant (blazing blue, fired by the
//! wither on hard mode) that flies slower and a plain variant that explodes on any
//! hit with power 1. Impact damage is 8 with a wither effect whose duration scales
//! with difficulty; a non-living owner deals 5 magic damage instead.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_entity_data::WitherSkullEntityData;
use steel_registry::{
    sound_events, vanilla_damage_types, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::Difficulty;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::MobEffectInstance;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::hurting::{
    ExplosionInteraction, ExplosionSpec, HurtingProjectile, HurtingProjectileState,
    do_post_attack_effects, explode,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, Projectile, ProjectileBase,
    ProjectileHit, RemovalReason, SharedEntity,
};
use crate::world::World;

/// Vanilla `WitherSkull.getInertia` for the dangerous variant.
const DANGEROUS_INERTIA: f32 = 0.73;

/// Vanilla `WitherSkull.onHit`: fixed explosion power for both variants.
const EXPLOSION_POWER: f32 = 1.0;

/// Vanilla `WitherSkull.onHitEntity` damage with a living owner.
const IMPACT_DAMAGE: f32 = 8.0;

/// Vanilla fallback damage when the owner is not a living entity.
const MAGIC_FALLBACK_DAMAGE: f32 = 5.0;

/// Vanilla difficulty-scaled wither durations (easy: none, normal: 10s, hard: 40s).
/// Peaceful despawns all attacking mobs in vanilla, so no duration applies either.
const fn wither_ticks(difficulty: Difficulty) -> i32 {
    match difficulty {
        Difficulty::Normal => 200,
        Difficulty::Hard => 800,
        Difficulty::Peaceful | Difficulty::Easy => 0,
    }
}

/// A wither skull.
#[entity_behavior(class = "WitherSkull")]
pub struct WitherSkullEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data carrying the dangerous variant flag.
    entity_data: SyncMutex<WitherSkullEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Shared `AbstractHurtingProjectile` accelerated-movement state.
    hurting: HurtingProjectileState,
}

// SAFETY: This key is owned by Steel and uniquely identifies `WitherSkullEntity`.
unsafe impl DowncastType for WitherSkullEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/wither_skull");
}

impl WitherSkullEntity {
    /// Creates a wither skull with no owner.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(WitherSkullEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
        }
    }

    /// Creates a wither skull moving along `direction` (vanilla
    /// `AbstractHurtingProjectile(type, x, y, z, direction, level)`).
    #[must_use]
    pub fn with_direction(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        direction: DVec3,
        world: Weak<World>,
    ) -> Self {
        let skull = Self::new(entity_type, id, position, world);
        skull.assign_directional_movement(direction, skull.acceleration_power());
        skull
    }

    /// Creates a wither skull from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(WitherSkullEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
        }
    }

    /// Returns the synced dangerous flag (vanilla `WitherSkull.isDangerous`).
    #[must_use]
    pub fn is_dangerous(&self) -> bool {
        *self.entity_data.lock().dangerous.get()
    }

    /// Sets the synced dangerous flag (vanilla `WitherSkull.setDangerous`).
    pub fn set_dangerous(&self, dangerous: bool) {
        self.entity_data.lock().dangerous.set(dangerous);
    }

    /// Vanilla `WitherSkull.onHit`: no fire placement, fixed-size explosion.
    fn detonate(self: &Arc<Self>, world: &Arc<World>) {
        let spec = ExplosionSpec {
            source: Arc::clone(self) as SharedEntity,
            damage_source: None,
            center: self.position(),
            radius: EXPLOSION_POWER,
            fire: false,
            interaction: ExplosionInteraction::Mob,
            damages_entities: true,
            knockback_multiplier: None,
            small_particles: ParticleData::simple(&vanilla_particle_types::EXPLOSION),
            large_particles: ParticleData::simple(&vanilla_particle_types::EXPLOSION_EMITTER),
            sound: &sound_events::ENTITY_GENERIC_EXPLODE,
        };
        explode(world, &spec);
        self.set_removed(RemovalReason::Discarded);
    }
}

impl Entity for WitherSkullEntity {
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

    fn is_on_fire(&self) -> bool {
        // Vanilla `WitherSkull.isOnFire` never renders fire.
        false
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
        nbt.insert("dangerous", i8::from(self.is_dangerous()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_hurting_projectile(nbt);
        self.set_dangerous(nbt.byte("dangerous").is_some_and(|value| value != 0));
    }
}

impl Projectile for WitherSkullEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        self.hurting_can_hit_entity(entity)
    }

    fn on_deflection(&self, by_attack: bool) {
        self.hurting_on_deflection(by_attack);
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, _location: DVec3) {
        // Vanilla `WitherSkull.onHitEntity`: 8 wither-skull damage from a living
        // owner (healing the owner if the target dies), 5 magic damage without one,
        // plus a difficulty-scaled wither effect on living targets.
        let owner = self
            .get_owner()
            .filter(|owner| owner.as_living_entity().is_some());
        let damage = match &owner {
            Some(owner) => DamageSource::environment(&vanilla_damage_types::WITHER_SKULL)
                .with_direct_entity(Arc::clone(&self) as SharedEntity)
                .with_causing_entity(Arc::clone(owner)),
            None => DamageSource::environment(&vanilla_damage_types::MAGIC)
                .with_direct_entity(Arc::clone(&self) as SharedEntity),
        };
        let Some(world) = entity.level() else {
            return;
        };

        match &owner {
            Some(owner) => {
                let was_hurt = entity.hurt(&world, &damage, IMPACT_DAMAGE);
                if was_hurt {
                    if entity.is_alive() {
                        do_post_attack_effects(&world, entity, &damage);
                    } else if let Some(living) = owner.as_living_entity() {
                        living.heal(5.0);
                    }
                }
            }
            None => {
                entity.hurt(&world, &damage, MAGIC_FALLBACK_DAMAGE);
            }
        }

        if let Some(living) = entity.as_living_entity()
            && wither_ticks(world.difficulty()) > 0
        {
            living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::WITHER,
                wither_ticks(world.difficulty()),
                1,
            ));
        }
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);
        if let Some(world) = self.level() {
            self.detonate(&world);
        }
    }
}

impl HurtingProjectile for WitherSkullEntity {
    fn hurting_state(&self) -> &HurtingProjectileState {
        &self.hurting
    }

    fn get_inertia(&self) -> f32 {
        if self.is_dangerous() {
            DANGEROUS_INERTIA
        } else {
            0.95
        }
    }

    fn should_burn(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::world::World;

    use super::WitherSkullEntity;

    #[test]
    fn dangerous_round_trips_through_synced_data() {
        init_vanilla_registry();

        let skull = WitherSkullEntity::new(
            &vanilla_entities::WITHER_SKULL,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        assert!(!skull.is_dangerous());

        skull.set_dangerous(true);
        assert!(skull.is_dangerous());
    }
}
