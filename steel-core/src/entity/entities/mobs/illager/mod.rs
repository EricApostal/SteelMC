//! Illager (raid) entity implementations.
//!
//! Vanilla sources: `/tmp/vanilla/src/world/entity/monster/illager/{Pillager,Vindicator,
//! Evoker,Illusioner}.java` and `/tmp/vanilla/src/world/entity/monster/{Ravager,Witch}.java`
//! (`Witch` lives in `mobs/hostile/witch`). `abstract_illager` holds the shared
//! `AbstractIllager`/`SpellcasterIllager` data, and `raider_goals` wires the goals every
//! raider registers.

pub mod abstract_illager;
mod evoker;
mod illusioner;
mod pillager;
mod raider_goals;
mod ravager;
mod vindicator;

pub use abstract_illager::{IllagerArmPose, IllagerSpell, RaiderKeys};
pub use evoker::EvokerEntity;
pub use illusioner::IllusionerEntity;
pub use pillager::PillagerEntity;
pub use ravager::RavagerEntity;
pub use vindicator::VindicatorEntity;
