//! Vanilla `net.minecraft.world.entity.monster.warden` package.
//!
//! - [`brain`] is the `WardenAi` half: `BRAIN_PROVIDER`, the activities and
//!   `updateActivity`.
//! - [`anger`] is `AngerManagement`/`AngerLevel`.
//! - [`warden`] is the entity itself.
//! - [`spawn_tracker`] is `WardenSpawnTracker`, the shrieker escalation that gates
//!   warden summons on the `spawn_wardens` game rule.

mod anger;
mod brain;
mod spawn_tracker;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (warden); renaming would churn every consumer"
)]
mod warden;

pub use anger::AngerLevel;
pub use spawn_tracker::{MAX_WARNING_LEVEL, WardenSpawnTracker};
pub use warden::WardenEntity;
