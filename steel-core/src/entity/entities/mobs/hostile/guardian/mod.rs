//! Guardian family (vanilla `net.minecraft.world.entity.monster.Guardian`).
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

pub mod elder_guardian;
pub use elder_guardian::ElderGuardianEntity;
pub mod guardian;
pub use guardian::GuardianEntity;
