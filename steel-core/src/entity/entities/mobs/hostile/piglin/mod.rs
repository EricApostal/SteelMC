//! Piglin family (vanilla `net.minecraft.world.entity.monster.piglin`).
//!
//! Vanilla `Piglin`/`PiglinBrute` are brain-driven (`PiglinAi`/`PiglinBruteAi`); Steel has no
//! piglin behavior package or bartering registry yet, so only the goal-driven
//! `ZombifiedPiglin` (vanilla `monster/zombie/ZombifiedPiglin`) is implemented here.
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

pub mod zombified_piglin;
pub use zombified_piglin::{ZombifiedPiglinEntity};
