//! Polar bear implementation (vanilla `animal/polarbear/PolarBear`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (polar_bear); renaming would churn every consumer"
)]
pub mod polar_bear;

pub use polar_bear::PolarBearEntity;
