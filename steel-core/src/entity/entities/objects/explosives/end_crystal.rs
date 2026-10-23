//! Minimal End Crystal entity implementation for End spike worldgen.

use std::sync::Arc;
use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_entity_data::EndCrystalEntityData;
use steel_registry::vanilla_game_events::ENTITY_DIE;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, locks::SyncMutex};
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::behavior::blocks::FireBlock;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, RemovalReason, SharedEntity,
};
use crate::world::World;
use crate::world::explosion::ExplosionInteraction;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;

/// End Crystal entity state needed by worldgen and persistence.
///
/// Steel currently implements the synchronized data and saved fields used by generated
/// End spikes. Portal handling, dragon fight callbacks, and explosion behavior are still
/// intentionally left to the broader entity/combat foundations.
#[entity_behavior(class = "EndCrystal")]
pub struct EndCrystalEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<EndCrystalEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `EndCrystalEntity`.
unsafe impl DowncastType for EndCrystalEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/end_crystal");
}

impl EndCrystalEntity {
    /// Creates a new End Crystal entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(EndCrystalEntityData::new()),
        }
    }

    /// Creates an End Crystal entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(EndCrystalEntityData::new()),
        }
    }

    /// Sets the optional beam target.
    pub fn set_beam_target(&self, target: Option<BlockPos>) {
        self.entity_data.lock().beam_target.set(target);
    }

    /// Returns the optional beam target.
    #[must_use]
    pub fn beam_target(&self) -> Option<BlockPos> {
        *self.entity_data.lock().beam_target.get()
    }

    /// Sets whether the crystal renders its bedrock base.
    pub fn set_show_bottom(&self, show_bottom: bool) {
        self.entity_data.lock().show_bottom.set(show_bottom);
    }

    /// Returns whether the crystal renders its bedrock base.
    #[must_use]
    pub fn shows_bottom(&self) -> bool {
        *self.entity_data.lock().show_bottom.get()
    }

    const fn nbt_bool(value: bool) -> i8 {
        if value { 1 } else { 0 }
    }

    /// Vanilla `EndCrystal.onDestroyedBy`: notifies the End dragon fight, when the
    /// dimension has one.
    ///
    /// NOTE: vanilla passes the damage source through to
    /// `EnderDragonFight.onCrystalDestroyed`; Steel's fight does not branch on it, so the
    /// hook takes only the crystal UUID.
    fn on_destroyed_by(&self, world: &Arc<World>) {
        if let Some(fight) = world.dragon_fight() {
            fight.lock().on_crystal_destroyed(world, self.uuid());
        }
    }
}

impl Entity for EndCrystalEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `EndCrystal.tick`: block effects, portal handling, then (server only)
        // lighting a fire at the crystal's own position while the dragon fight is active.
        self.apply_effects_from_blocks();
        self.handle_portal();

        let Some(world) = self.level() else {
            return;
        };
        if world.dragon_fight().is_none() {
            return;
        }
        let pos = self.block_position();
        if world.get_block_state(pos).is_air() {
            world.set_block(
                pos,
                FireBlock::get_state(&*world, pos),
                UpdateFlags::UPDATE_ALL,
            );
        }
    }

    fn is_pickable(&self) -> bool {
        true
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn hurt(&self, _world: &World, source: &DamageSource, _amount: f32) -> bool {
        // Match vanilla's `EndCrystal.hurtServer`.
        if self.is_invulnerable_to_base(source) {
            return false;
        }

        if source
            .causing_entity()
            .is_some_and(|entity| entity.entity_type() == &vanilla_entities::ENDER_DRAGON)
        {
            return false;
        }

        if !self.is_removed() {
            self.set_removed(RemovalReason::Killed);
            let level = self.level();

            // Vanilla detonates the crystal unless the damage type is already an
            // explosion: `level.explode(this, damageSource, null, x, y, z, 6.0F, false,
            // ExplosionInteraction.BLOCK)`, with the explosion damage source
            // `damageSources().explosion(this, source.getEntity())`. Steel derives that
            // default damage source from the source entity inside `World::explode_full`.
            if !source.is(&DamageTypeTag::IS_EXPLOSION)
                && let Some(world) = &level
            {
                let position = self.position();
                world.explode_full(
                    world.get_entity_by_id(self.id()),
                    None,
                    None,
                    position.x,
                    position.y,
                    position.z,
                    6.0,
                    false,
                    ExplosionInteraction::Block,
                );
            }

            if let Some(world) = &level {
                self.on_destroyed_by(world);
            }
        }

        true
    }

    fn kill(&self, _world: &World) {
        // Vanilla `EndCrystal.kill` notifies the dragon fight before the shared
        // non-living kill path (`remove(KILLED)` then `ENTITY_DIE`).
        if let Some(world) = self.level() {
            self.on_destroyed_by(&world);
        }
        self.set_removed(RemovalReason::Killed);
        self.game_event(&ENTITY_DIE);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        if let Some(target) = self.beam_target() {
            nbt.insert(
                "beam_target",
                NbtTag::IntArray(vec![target.x(), target.y(), target.z()]),
            );
        }

        nbt.insert("ShowBottom", Self::nbt_bool(self.shows_bottom()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        if let Some(target) = nbt.int_array("beam_target")
            && target.len() == 3
        {
            self.set_beam_target(Some(BlockPos::new(target[0], target[1], target[2])));
        }

        if let Some(show_bottom) = nbt.byte("ShowBottom") {
            self.set_show_bottom(show_bottom != 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use steel_registry::vanilla_entities;

    #[test]
    fn end_crystal_does_not_duplicate_shared_invulnerable_state() {
        let crystal = EndCrystalEntity::new(
            &vanilla_entities::END_CRYSTAL,
            1,
            DVec3::new(1.5, 2.5, 3.5),
            Weak::new(),
        );
        crystal.set_invulnerable(true);

        let mut nbt = NbtCompound::new();
        crystal.save_additional(&mut nbt);

        assert_eq!(nbt.byte("Invulnerable"), None);
    }

    #[test]
    fn end_crystal_is_pickable_like_vanilla() {
        let crystal = EndCrystalEntity::new(
            &vanilla_entities::END_CRYSTAL,
            1,
            DVec3::new(1.5, 2.5, 3.5),
            Weak::new(),
        );

        assert!(crystal.is_pickable());
    }

    #[test]
    fn end_crystal_blocks_building_like_vanilla() {
        let crystal =
            EndCrystalEntity::new(&vanilla_entities::END_CRYSTAL, 1, DVec3::ZERO, Weak::new());

        assert!(crystal.blocks_building());
    }
}
