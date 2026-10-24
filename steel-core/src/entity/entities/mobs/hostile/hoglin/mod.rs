//! Hoglin family (vanilla `monster/hoglin/Hoglin` and `monster/Zoglin`).
//!
//! Both are brain-driven in vanilla (`HoglinAi`); [`brain`] is owned by the brain slice.
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

pub mod brain;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (hoglin); renaming would churn every consumer"
)]
pub mod hoglin;
pub use hoglin::HoglinEntity;
pub mod zoglin;
pub use zoglin::ZoglinEntity;
