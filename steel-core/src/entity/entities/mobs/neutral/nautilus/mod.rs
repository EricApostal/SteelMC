//! Vanilla nautilus family (`world/entity/animal/nautilus/`).
//!
//! `AbstractNautilus` is a tameable, saddled water mount; `Nautilus` is the
//! peaceful ocean variant and `ZombieNautilus` the hostile, biome-variant one.
//! Steel has no class inheritance, so the shared `AbstractNautilus` state and
//! behaviour live in [`abstract_nautilus`], the two `Ai` classes and their shared
//! sensors in [`brain`], and each concrete entity owns only its own synced data,
//! sounds and trait impls.

pub mod abstract_nautilus;
pub mod brain;
mod nautilus;
mod zombie_nautilus;

pub use abstract_nautilus::{AbstractNautilusParts, check_nautilus_spawn_rules};
pub use nautilus::NautilusEntity;
pub use zombie_nautilus::ZombieNautilusEntity;
