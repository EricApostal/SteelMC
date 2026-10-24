//! Panda implementation (vanilla `animal/panda/Panda`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (panda); renaming would churn every consumer"
)]
pub mod panda;

pub use panda::PandaEntity;
