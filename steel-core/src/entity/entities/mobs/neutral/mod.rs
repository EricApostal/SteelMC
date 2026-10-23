//! Neutral entity implementations.
//!
//! Those mobs are neutral to players, but will attack if provoked.

pub mod bee;
pub mod dolphin;
pub mod fox;
pub mod nautilus;
pub mod panda;
pub mod polar_bear;
pub mod squid;
pub mod wolf;

pub use bee::BeeEntity;
pub use dolphin::DolphinEntity;
pub use fox::FoxEntity;
pub use nautilus::{NautilusEntity, ZombieNautilusEntity};
pub use panda::PandaEntity;
pub use polar_bear::PolarBearEntity;
pub use squid::{GlowSquidEntity, SquidEntity};
pub use wolf::WolfEntity;
