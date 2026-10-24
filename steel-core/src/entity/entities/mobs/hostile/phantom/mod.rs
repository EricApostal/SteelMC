//! Phantom implementation (vanilla `net.minecraft.world.entity.monster.Phantom`).
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (phantom); renaming would churn every consumer"
)]
pub mod phantom;
pub use phantom::PhantomEntity;
