//! Small fireball projectile entity (`SmallFireball`).
//!
//! Mirrors vanilla `SmallFireball` (blaze and fire-charge fireballs): 5 impact
//! damage that ignites the target for 5 seconds, and fire placement on the block
//! face it hits when the shooter is not a mob or mob-griefing allows it.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_data::SmallFireballEntityData;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::{vanilla_damage_types, vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::types::UpdateFlags;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::behavior::blocks::FireBlock;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::hurting::{
    HurtingProjectile, HurtingProjectileState, do_post_attack_effects,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, Projectile, ProjectileBase,
    ProjectileHit, RemovalReason, SharedEntity,
};
use crate::world::{ClipHitResult, World};

/// Vanilla `SmallFireball.onHitEntity` impact damage.
const IMPACT_DAMAGE: f32 = 5.0;

/// Vanilla `SmallFireball.onHitEntity` ignite duration (`igniteForSeconds(5.0F)`).
const IGNITE_TICKS: i32 = 100;

/// A blaze or fire-charge fireball.
#[entity_behavior(class = "SmallFireball")]
pub struct SmallFireballEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data carrying the rendered fire-charge item stack.
    entity_data: SyncMutex<SmallFireballEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Shared `AbstractHurtingProjectile` accelerated-movement state.
    hurting: HurtingProjectileState,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SmallFireballEntity`.
unsafe impl DowncastType for SmallFireballEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/small_fireball");
}

impl SmallFireballEntity {
    /// Creates a small fireball with no owner.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(SmallFireballEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
        }
    }

    /// Creates a small fireball moving along `direction` (vanilla
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

    /// Creates a small fireball from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(SmallFireballEntityData::new()),
            projectile_base: ProjectileBase::new(),
            hurting: HurtingProjectileState::new(),
        }
    }

    /// Returns the synced fire-charge item stack (vanilla `Fireball.getItem`).
    #[must_use]
    pub fn get_item(&self) -> ItemStack {
        self.entity_data.lock().fireball.item_stack.get().clone()
    }

    /// Replaces the synced item stack, clamping the count to one like vanilla
    /// `Fireball.setItem` (empty stacks fall back to the default fire charge).
    pub fn set_item(&self, item: ItemStack) {
        let clamped = if item.is_empty() {
            ItemStack::new(&vanilla_items::FIRE_CHARGE)
        } else {
            item.copy_with_count(1)
        };
        self.entity_data.lock().fireball.item_stack.set(clamped);
    }

    /// Vanilla `SmallFireball.onHitBlock` fire placement.
    fn place_fire(&self, world: &Arc<World>, hit: &ClipHitResult) {
        let mob_owner = self.get_owner().is_some_and(|owner| owner.is_mob());
        if mob_owner && !world.get_game_rule(&MOB_GRIEFING) {
            return;
        }

        let fire_pos = hit.block_pos.relative(hit.direction);
        if !world.get_block_state(fire_pos).is_air() {
            return;
        }
        world.set_block(
            fire_pos,
            FireBlock::get_state(world.as_ref(), fire_pos),
            UpdateFlags::UPDATE_ALL,
        );
    }
}

impl Entity for SmallFireballEntity {
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
        nbt.insert("Item", self.get_item().to_nbt_tag_ref());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_hurting_projectile(nbt);
        if let Some(item) = nbt
            .compound("Item")
            .and_then(|item| ItemStack::from_borrowed_compound(&item))
        {
            self.set_item(item);
        }
    }
}

impl Projectile for SmallFireballEntity {
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
        // Vanilla `SmallFireball.onHitEntity`: ignite first, then deal 5 damage;
        // a failed hit restores the target's previous fire ticks.
        let remaining_fire_ticks = entity.remaining_fire_ticks();
        entity.ignite_for_ticks(IGNITE_TICKS);

        let mut damage = DamageSource::environment(&vanilla_damage_types::FIREBALL)
            .with_direct_entity(Arc::clone(&self) as SharedEntity);
        if let Some(owner) = self.get_owner() {
            damage = damage.with_causing_entity(owner);
        }
        let Some(world) = entity.level() else {
            return;
        };
        if entity.hurt(&world, &damage, IMPACT_DAMAGE) {
            do_post_attack_effects(&world, entity, &damage);
        } else {
            entity.set_remaining_fire_ticks(remaining_fire_ticks);
        }
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        Arc::clone(&self).projectile_on_hit_block(hit);
        if let Some(world) = self.level() {
            self.place_fire(&world, hit);
        }
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);
        self.set_removed(RemovalReason::Discarded);
    }
}

impl HurtingProjectile for SmallFireballEntity {
    fn hurting_state(&self) -> &HurtingProjectileState {
        &self.hurting
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::entity::Entity;
    use crate::world::World;

    use super::SmallFireballEntity;

    #[test]
    fn directional_constructor_uses_the_vanilla_initial_power() {
        init_vanilla_registry();

        let fireball = SmallFireballEntity::with_direction(
            &vanilla_entities::SMALL_FIREBALL,
            1,
            DVec3::ZERO,
            DVec3::X,
            Weak::<World>::new(),
        );

        let velocity = fireball.velocity();
        assert!((velocity.x - 0.1).abs() < 1.0e-9);
        assert!(velocity.y.abs() < 1.0e-9 && velocity.z.abs() < 1.0e-9);
    }
}
