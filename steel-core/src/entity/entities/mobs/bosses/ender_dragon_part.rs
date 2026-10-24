//! Vanilla `EnderDragonPart`: the dragon's eight unregistered hitboxes.
//!
//! Mirrors `net.minecraft.world.entity.boss.enderdragon.EnderDragonPart`. Parts
//! are `Entity` subclasses that vanilla keeps out of the level's entity map and
//! exposes through `ServerLevel.dragonParts()` for hit detection; this port keeps
//! them owned by the dragon and positions them from the dragon's tick. Damage
//! received by a part forwards to the parent dragon with its own scaling.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::vanilla_entities;
use steel_utils::locks::SyncMutex;
use steel_utils::{Downcast as _, DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityPose, EntitySyncedData, SharedEntity, next_entity_id,
};
use crate::world::World;

use super::ender_dragon::EnderDragonEntity;

/// One of vanilla `EnderDragon`'s eight sub-entities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnderDragonPartKind {
    /// Vanilla `head`, `1.0 x 1.0`.
    Head,
    /// Vanilla `neck`, `3.0 x 3.0`.
    Neck,
    /// Vanilla `body`, `5.0 x 3.0`.
    Body,
    /// Vanilla first `tail`, `2.0 x 2.0`.
    Tail1,
    /// Vanilla second `tail`, `2.0 x 2.0`.
    Tail2,
    /// Vanilla third `tail`, `2.0 x 2.0`.
    Tail3,
    /// Vanilla `wing1`, `4.0 x 2.0`.
    Wing1,
    /// Vanilla `wing2`, `4.0 x 2.0`.
    Wing2,
}

impl EnderDragonPartKind {
    /// Vanilla `EnderDragonPart.name`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Neck => "neck",
            Self::Body => "body",
            Self::Tail1 | Self::Tail2 | Self::Tail3 => "tail",
            Self::Wing1 | Self::Wing2 => "wing",
        }
    }

    /// Vanilla `EntityDimensions.scalable(width, height)` for this part.
    #[must_use]
    pub const fn dimensions(self) -> EntityDimensions {
        let (width, height) = match self {
            Self::Head => (1.0, 1.0),
            Self::Neck => (3.0, 3.0),
            Self::Body => (5.0, 3.0),
            Self::Tail1 | Self::Tail2 | Self::Tail3 => (2.0, 2.0),
            Self::Wing1 | Self::Wing2 => (4.0, 2.0),
        };
        EntityDimensions::with_default_eye_height(width, height)
    }
}

/// One vanilla `EnderDragonPart` hitbox.
///
/// Vanilla parts are unregistered `Entity` subclasses: they share the dragon's
/// entity type to keep vanilla's type dispatch, but have no entity factory and
/// are never added to the world's entity manager.
pub struct EnderDragonPartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    kind: EnderDragonPartKind,
    parent: SyncMutex<Weak<dyn Entity>>,
}

// SAFETY: This Steel-owned key uniquely identifies `EnderDragonPartEntity`.
unsafe impl DowncastType for EnderDragonPartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ender_dragon_part");
}

impl EnderDragonPartEntity {
    /// Creates a part hitbox for `kind`, positioned at the world origin until
    /// the owning dragon places it.
    ///
    /// Vanilla `EnderDragonPart(EnderDragon parentMob, String name, float w, float h)`;
    /// the parent is bound through [`Self::bind_parent`] once the dragon has a
    /// shared handle because Rust cannot self-reference during construction.
    #[must_use]
    pub fn new(kind: EnderDragonPartKind, world: Weak<World>) -> Self {
        let entity_type: EntityTypeRef = &vanilla_entities::ENDER_DRAGON;
        Self {
            base: EntityBase::new(next_entity_id(), DVec3::ZERO, kind.dimensions(), world),
            entity_type,
            kind,
            parent: SyncMutex::new(Weak::<EnderDragonEntity>::new()),
        }
    }

    /// Binds the owning dragon.
    pub fn bind_parent(&self, parent: &SharedEntity) {
        *self.parent.lock() = Arc::downgrade(parent);
    }

    /// Returns the owning dragon, if it is still alive.
    #[must_use]
    pub fn parent(&self) -> Option<SharedEntity> {
        self.parent.lock().upgrade()
    }

    /// Returns which dragon sub-entity this part is.
    #[must_use]
    pub const fn kind(&self) -> EnderDragonPartKind {
        self.kind
    }

    /// Returns the vanilla part name.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.kind.name()
    }
}

impl Entity for EnderDragonPartEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        self.kind.dimensions()
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        None
    }

    fn is_pickable(&self) -> bool {
        true
    }

    /// Vanilla `EnderDragonPart.hurtServer` forwards to the parent dragon, which
    /// applies the phase hook and the head/neck damage scaling.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_invulnerable_to_base(source) {
            return false;
        }
        let Some(parent) = self.parent() else {
            return false;
        };
        let Some(dragon) = parent.downcast_ref::<EnderDragonEntity>() else {
            return false;
        };
        dragon.hurt_dragon(self.kind, world, source, amount)
    }

    /// Vanilla `EnderDragonPart.shouldBeSaved` is `false`; Steel parts are never
    /// added to the world entity manager, so they are never serialized.
    fn save_additional(&self, _nbt: &mut NbtCompound) {}

    fn load_additional(&self, _nbt: BorrowedNbtCompoundView<'_, '_>) {}

    /// Vanilla parts are positioned by their parent, not by entity ticks.
    fn tick(self: Arc<Self>) {}
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use super::{EnderDragonPartEntity, EnderDragonPartKind};
    use crate::entity::Entity;

    #[test]
    fn part_dimensions_match_vanilla() {
        assert_eq!(EnderDragonPartKind::Head.dimensions().width, 1.0);
        assert_eq!(EnderDragonPartKind::Neck.dimensions().width, 3.0);
        assert_eq!(EnderDragonPartKind::Body.dimensions().width, 5.0);
        assert_eq!(EnderDragonPartKind::Body.dimensions().height, 3.0);
        assert_eq!(EnderDragonPartKind::Tail1.dimensions().width, 2.0);
        assert_eq!(EnderDragonPartKind::Wing1.dimensions().width, 4.0);
        assert_eq!(EnderDragonPartKind::Wing1.dimensions().height, 2.0);
    }

    #[test]
    fn part_names_match_vanilla() {
        assert_eq!(EnderDragonPartKind::Head.name(), "head");
        assert_eq!(EnderDragonPartKind::Body.name(), "body");
        assert_eq!(EnderDragonPartKind::Tail3.name(), "tail");
        assert_eq!(EnderDragonPartKind::Wing2.name(), "wing");
    }

    #[test]
    fn unbound_part_is_not_pickable_by_parent() {
        steel_registry::init_vanilla_registry();
        let part = EnderDragonPartEntity::new(EnderDragonPartKind::Head, Weak::new());
        assert!(part.parent().is_none());
        assert!(part.is_pickable());
    }
}
