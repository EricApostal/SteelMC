//! Breeze implementation (vanilla `monster/breeze/Breeze`).
//!
//! Brain-driven in vanilla (`BreezeAi`); [`brain`] is owned by the brain slice.

pub mod brain;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (breeze); renaming would churn every consumer"
)]
pub mod breeze;
pub mod long_jump;
pub mod shoot;
pub mod shoot_when_stuck;
pub mod slide;

pub use breeze::BreezeEntity;
