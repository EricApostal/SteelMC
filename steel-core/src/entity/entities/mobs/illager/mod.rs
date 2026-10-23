//! Illager (raid) entity implementations.
//!
//! `Pillager` is implemented; `Vindicator`, `Evoker`, `Illusioner`, `Ravager` and `Witch`
//! are the remaining work of the raid slice (see `local://raid-checkpoint.md`).
//!
//! Vanilla sources for the remaining five:
//! `/tmp/vanilla/src/world/entity/monster/illager/{Vindicator,Evoker,Illusioner}.java`,
//! `/tmp/vanilla/src/world/entity/monster/{Ravager,Witch}.java`, with goal lists in the spec
//! at `local://village-behaviour-spec.md`.

pub mod abstract_illager;
mod pillager;
mod raider_goals;
mod vindicator;

pub use abstract_illager::{IllagerArmPose, IllagerSpell, RaiderKeys};
pub use pillager::PillagerEntity;
pub use vindicator::VindicatorEntity;
