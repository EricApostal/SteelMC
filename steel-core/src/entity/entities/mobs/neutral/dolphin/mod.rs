//! Dolphin implementation (vanilla `animal/dolphin/Dolphin`).

#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (dolphin); renaming would churn every consumer"
)]
pub mod dolphin;

pub use dolphin::DolphinEntity;
