//! Guardian family (vanilla `net.minecraft.world.entity.monster.Guardian`).
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

pub mod elder_guardian;
pub use elder_guardian::ElderGuardianEntity;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (guardian); renaming would churn every consumer"
)]
pub mod guardian;
pub use guardian::GuardianEntity;
