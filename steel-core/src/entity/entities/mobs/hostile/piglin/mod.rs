//! Piglin family (vanilla `net.minecraft.world.entity.monster.piglin`).
//!
//! Vanilla `Piglin`/`PiglinBrute` are brain-driven (`PiglinAi`/`PiglinBruteAi`, in
//! [`brain`]) and share the `AbstractPiglin` zombification clock; Steel has no
//! inheritance, so each entity re-implements the shared state it uses.
//! `ZombifiedPiglin` (vanilla `monster/zombie/ZombifiedPiglin`) is goal-driven and
//! lives in its own module.

pub mod brain;
pub mod piglin;
pub mod piglin_brute;
pub mod zombified_piglin;

pub use piglin::{PiglinArmPose, PiglinEntity, check_piglin_spawn_rules};
pub use piglin_brute::PiglinBruteEntity;
pub use zombified_piglin::ZombifiedPiglinEntity;
