//! Squid family (vanilla `animal/squid/Squid` + `GlowSquid`).

pub mod glow_squid;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (squid); renaming would churn every consumer"
)]
pub mod squid;

pub use glow_squid::GlowSquidEntity;
pub use squid::SquidEntity;
