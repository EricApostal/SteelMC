//! Bee implementation (vanilla `animal/bee/Bee`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (bee); renaming would churn every consumer"
)]
pub mod bee;

pub use bee::BeeEntity;
