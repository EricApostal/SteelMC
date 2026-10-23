//! Creaking implementation (vanilla `monster/creaking/Creaking`).
//!
//! Brain-driven in vanilla (`CreakingAi`); [`brain`] is owned by the brain slice.
// TODO(integrator): entity modules are declared by their owner once the entity files exist.

pub mod brain;
pub mod creaking;
pub use creaking::{CreakingEntity};
