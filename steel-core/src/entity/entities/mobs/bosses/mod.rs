//! Boss mob implementations.

pub mod ender_dragon;
pub mod ender_dragon_part;
pub mod phases;

pub use ender_dragon::EnderDragonEntity;
pub use ender_dragon_part::{EnderDragonPartEntity, EnderDragonPartKind};
pub use phases::{EnderDragonPhase, EnderDragonPhaseManager};
