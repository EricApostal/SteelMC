//! Fox implementation (vanilla `animal/fox/Fox`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (fox); renaming would churn every consumer"
)]
pub mod fox;

pub use fox::FoxEntity;
