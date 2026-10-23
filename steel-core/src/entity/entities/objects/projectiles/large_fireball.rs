//! Large fireball projectile entity (`LargeFireball`, registered as `minecraft:fireball`).
//!
//! Mirrors vanilla `LargeFireball` on the Steel
//! `Projectile → AbstractHurtingProjectile` trait stack: ghast fireballs deal
//! 6 impact damage and detonate with an explosion whose power is stored in NBT
//! and whose fire placement follows the mob-griefing rule.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_entity_data::FireballEntityData;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::{
    sound_events, vanilla_damage_types, vanilla_entities, vanilla_items, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

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

/// Vanilla `LargeFireball.DEFAULT_EXPLOSION_POWER`.
const DEFAULT_EXPLOSION_POWER: i32 = 1;

/// Vanilla `LargeFireball.onHitEntity` impact damage.
const IMPACT_DAMAGE: f32 = 6.0;

/// A ghast fireball.
#[entity_behavior(class = "LargeFireball")]
pub struct LargeFireballEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data carrying the rendered fire-charge item stack.
    entity_data: SyncMutex<FireballEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Shared `AbstractHurtingProjectile` accelerated-movement state.
    hurting: HurtingProjectileState,
    /// Vanilla `LargeFireball.explosionPower`.
    explosion_power: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `LargeFireballEntity`.
unsafe impl DowncastType for LargeFireballEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/large_fireball");
}

impl LargeFireballEntity {
    /// Creates a fireball with no owner and the default explosion power.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(FireballEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
            explosion_power: SyncMutex::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    /// Creates a fireball moving along `direction` (vanilla
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

    /// Creates a fireball from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(FireballEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
            explosion_power: SyncMutex::new(DEFAULT_EXPLOSION_POWER),
        }
    }

    /// Returns the explosion power used on impact.
    #[must_use]
    pub fn explosion_power(&self) -> i32 {
        *self.explosion_power.lock()
    }

    /// Replaces the explosion power used on impact.
    pub fn set_explosion_power(&self, explosion_power: i32) {
        *self.explosion_power.lock() = explosion_power;
    }

    /// Returns the synced fire-charge item stack (vanilla `Fireball.getItem`).
    #[must_use]
    pub fn get_item(&self) -> ItemStack {
        self.entity_data.lock().item_stack.get().clone()
    }

    /// Replaces the synced item stack, clamping the count to one like vanilla
    /// `Fireball.setItem` (empty stacks fall back to the default fire charge).
    pub fn set_item(&self, item: ItemStack) {
        let clamped = if item.is_empty() {
            ItemStack::new(&vanilla_items::FIRE_CHARGE)
        } else {
            item.copy_with_count(1)
        };
        self.entity_data.lock().item_stack.set(clamped);
    }

    /// Vanilla `LargeFireball.onHit`: base hit handling, then the explosion.
    fn detonate(self: &Arc<Self>, world: &Arc<World>) {
        let fire = world.get_game_rule(&MOB_GRIEFING);
        let spec = ExplosionSpec {
            source: Arc::clone(self) as SharedEntity,
            damage_source: None,
            center: self.position(),
            radius: self.explosion_power() as f32,
            fire,
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

impl Entity for LargeFireballEntity {
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
        nbt.insert("ExplosionPower", self.explosion_power() as i8);
        nbt.insert("Item", self.get_item().to_nbt_tag_ref());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_hurting_projectile(nbt);
        self.set_explosion_power(i32::from(
            nbt.byte("ExplosionPower")
                .unwrap_or(DEFAULT_EXPLOSION_POWER as i8),
        ));
        if let Some(item) = nbt
            .compound("Item")
            .and_then(|item| ItemStack::from_borrowed_compound(&item))
        {
            self.set_item(item);
        }
    }
}

impl Projectile for LargeFireballEntity {
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
        // Vanilla `LargeFireball.onHitEntity`: 6 fireball damage plus the
        // attacker's post-attack enchantment effects.
        let mut damage = DamageSource::environment(&vanilla_damage_types::FIREBALL)
            .with_direct_entity(Arc::clone(&self) as SharedEntity);
        if let Some(owner) = self.get_owner() {
            damage = damage.with_causing_entity(owner);
        }
        let Some(world) = entity.level() else {
            return;
        };
        entity.hurt(&world, &damage, IMPACT_DAMAGE);
        do_post_attack_effects(&world, entity, &damage);
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);
        if let Some(world) = self.level() {
            self.detonate(&world);
        }
    }
}

impl HurtingProjectile for LargeFireballEntity {
    fn hurting_state(&self) -> &HurtingProjectileState {
        &self.hurting
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};

    use crate::entity::Entity;
    use crate::entity::entities::objects::projectiles::hurting::INITIAL_ACCELERATION_POWER;
    use crate::world::World;

    use super::LargeFireballEntity;

    #[test]
    fn directional_constructor_moves_along_the_direction() {
        init_vanilla_registry();

        let fireball = LargeFireballEntity::with_direction(
            &vanilla_entities::FIREBALL,
            1,
            DVec3::ZERO,
            DVec3::new(0.0, 0.0, 4.0),
            Weak::<World>::new(),
        );

        let velocity = fireball.velocity();
        assert!((velocity.length() - INITIAL_ACCELERATION_POWER).abs() < 1.0e-9);
        assert!((velocity.z - INITIAL_ACCELERATION_POWER).abs() < 1.0e-9);
    }

    #[test]
    fn empty_item_falls_back_to_fire_charge() {
        init_vanilla_registry();

        let fireball = LargeFireballEntity::new(
            &vanilla_entities::FIREBALL,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        fireball.set_item(ItemStack::empty());

        assert_eq!(fireball.get_item().item, &*vanilla_items::FIRE_CHARGE);
        assert_eq!(fireball.get_item().count, 1);
    }
}
