//! Llama spit projectile entity (`LlamaSpit`).
//!
//! Mirrors vanilla `LlamaSpit`: 1 spit damage from the owning llama, gravity of
//! 0.06 with 0.99 air drag, discarded in water, inside non-air blocks, or on
//! block impact. The spit trail particles are client-local.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_damage_types;
use steel_registry::vanilla_entity_data::LlamaSpitEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey, axis::Axis};

use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::hurting::do_post_attack_effects;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, Projectile, ProjectileBase,
    RemovalReason, SharedEntity,
};
use crate::world::{ClipHitResult, World};

/// Vanilla `LlamaSpit.onHitEntity` impact damage.
const IMPACT_DAMAGE: f32 = 1.0;

/// Vanilla `LlamaSpit.getDefaultGravity`.
const DEFAULT_GRAVITY: f64 = 0.06;

/// Vanilla `LlamaSpit.getAirDrag`.
const AIR_DRAG: f64 = 0.99;

/// Vanilla spawns the spit one body width plus one block in front of the llama.
const SPAWN_FORWARD_OFFSET: f64 = 1.0;

/// A llama spit projectile.
#[entity_behavior(class = "LlamaSpit")]
pub struct LlamaSpitEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data layer declared by this entity (no fields of its own).
    entity_data: SyncMutex<LlamaSpitEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies `LlamaSpitEntity`.
unsafe impl DowncastType for LlamaSpitEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/llama_spit");
}

impl LlamaSpitEntity {
    /// Creates llama spit with no owner.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(LlamaSpitEntityData::new()),
            projectile_base: ProjectileBase::new(),
        }
    }

    /// Creates spit at the llama's mouth offset (vanilla `LlamaSpit(Level, Llama)`).
    #[must_use]
    pub fn from_llama(
        entity_type: EntityTypeRef,
        id: i32,
        owner: &SharedEntity,
        world: Weak<World>,
    ) -> Self {
        let width = f64::from(owner.base().dimensions().width);
        let (yaw, _) = owner.rotation();
        let yaw = f64::from(yaw).to_radians();
        let position = owner.position()
            + DVec3::new(
                -(width + SPAWN_FORWARD_OFFSET) * 0.5 * yaw.sin(),
                owner.get_eye_y() - owner.position().y - 0.1,
                f64::midpoint(width, SPAWN_FORWARD_OFFSET) * yaw.cos(),
            );
        let spit = Self::new(entity_type, id, position, world);
        spit.set_owner_entity(Some(owner));
        spit
    }

    /// Creates llama spit from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(LlamaSpitEntityData::new()),
            projectile_base: ProjectileBase::new(),
        }
    }

    /// Vanilla `LlamaSpit.tick`'s bounding-box check: the whole box is inside
    /// non-air blocks (`findBlocksIn(...).filterState(isAir).noneMatched()`).
    fn is_inside_blocks(&self, world: &World) -> bool {
        let bounds = self.bounding_box();
        let min = BlockPos::containing(
            bounds.min(Axis::X),
            bounds.min(Axis::Y),
            bounds.min(Axis::Z),
        );
        let max = BlockPos::containing(
            bounds.max(Axis::X),
            bounds.max(Axis::Y),
            bounds.max(Axis::Z),
        );
        BlockPos::between_closed(min, max).all(|pos| !world.get_block_state(pos).is_air())
    }
}

impl Entity for LlamaSpitEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `LlamaSpit.tick`: resolve the move-vector hit, move by the current
        // delta, then drag/apply gravity for the next tick, discarding when the box is
        // buried in blocks or the spit is in water.
        self.set_old_position_to_current();
        self.base().set_old_rotation_to_current();

        self.projectile_base_tick();

        let movement = self.velocity();
        let hit = self.get_hit_result_on_move_vector();
        if let Some(result) = hit
            && self.is_alive()
        {
            Arc::clone(&self).hit_target_or_deflect_self(&result);
        }

        if !self.is_alive() {
            return;
        }

        let position = self.position();
        let next = position + movement;
        self.update_rotation();

        let Some(world) = self.level() else {
            return;
        };
        if self.is_inside_blocks(&world) || self.is_in_water() {
            self.set_removed(RemovalReason::Discarded);
            return;
        }

        self.set_velocity(movement * AIR_DRAG);
        self.apply_gravity();
        if self.try_set_position(next).is_err() {
            self.set_removed(RemovalReason::Discarded);
        }
    }

    fn get_default_gravity(&self) -> f64 {
        DEFAULT_GRAVITY
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

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        Projectile::hurt(self, world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
    }
}

impl Projectile for LlamaSpitEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, _location: DVec3) {
        // Vanilla `LlamaSpit.onHitEntity`: 1 spit damage, only with a living owner.
        let owner = self
            .get_owner()
            .filter(|owner| owner.as_living_entity().is_some());
        let Some(owner) = owner else {
            return;
        };
        let damage = DamageSource::environment(&vanilla_damage_types::SPIT)
            .with_direct_entity(Arc::clone(&self) as SharedEntity)
            .with_causing_entity(owner);
        let Some(world) = entity.level() else {
            return;
        };
        if entity.hurt(&world, &damage, IMPACT_DAMAGE) {
            do_post_attack_effects(&world, entity, &damage);
        }
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        Arc::clone(&self).projectile_on_hit_block(hit);
        self.set_removed(RemovalReason::Discarded);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::entity::Entity;
    use crate::world::World;

    use super::LlamaSpitEntity;

    #[test]
    fn spit_uses_the_vanilla_ballistic_constants() {
        init_vanilla_registry();

        let spit = LlamaSpitEntity::new(
            &vanilla_entities::LLAMA_SPIT,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        assert!((spit.get_default_gravity() - 0.06).abs() < 1.0e-12);
    }
}
