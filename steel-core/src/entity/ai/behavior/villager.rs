//! Villager-specific access shared by the villager behavior packages.
//!
//! Vanilla behaviors receive a `Villager` and call its methods directly. Steel's
//! behavior tree only hands out `&SharedEntity`, so this module holds the villager
//! queries that would otherwise be duplicated in every behavior. Entities reach the
//! concrete villager through the [`Npc`] capability hook rather than downcasting.

use steel_registry::villager_profession::VillagerProfessionRef;

use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::entity::{LivingEntity, SharedEntity};
use steel_utils::Downcast as _;

/// The villager behind a behavior body, or `None` for any other entity.
///
/// The [`Npc`] capability check keeps non-NPC bodies out without naming concrete
/// types; the final borrow is the keyed concrete downcast Steel uses for
/// Steel-owned entities (`Npc` itself is a marker trait with no members).
#[must_use]
pub fn as_villager(body: &SharedEntity) -> Option<&VillagerEntity> {
    body.as_npc()?;
    body.downcast_ref::<VillagerEntity>()
}

/// [`as_villager`] for call sites that also have the world at hand.
#[must_use]
pub fn villager<'a>(
    _level: &crate::world::World,
    body: &'a SharedEntity,
) -> Option<&'a VillagerEntity> {
    as_villager(body)
}

/// The villager's profession, or `None` when the body is not a villager.
#[must_use]
pub fn profession_of(body: &SharedEntity) -> Option<VillagerProfessionRef> {
    as_villager(body).map(VillagerEntity::profession)
}

/// Vanilla `Entity.isBaby` for villager bodies; non-villagers are never babies.
#[must_use]
pub fn is_baby_villager(body: &SharedEntity) -> bool {
    as_villager(body).is_some() && body.as_living_entity().is_some_and(LivingEntity::is_baby)
}
