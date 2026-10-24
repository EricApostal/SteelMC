//! Wolf implementation (vanilla `animal/wolf/Wolf`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (wolf); renaming would churn every consumer"
)]
pub mod wolf;

pub use wolf::WolfEntity;
