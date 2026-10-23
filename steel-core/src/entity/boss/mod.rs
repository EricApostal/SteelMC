//! Boss fights: shared boss-bar state and the ender dragon fight.
//!
//! Mirrors vanilla `net.minecraft.world.BossEvent` /
//! `net.minecraft.server.level.ServerBossEvent` ([`event`]), the persisted
//! `CustomBossEvents` manager backing the `/bossbar` command ([`custom`]), and
//! `net.minecraft.world.level.dimension.end.EnderDragonFight` with its
//! `DragonRespawnStage` saved state ([`dragon_fight`]).

pub mod custom;
pub mod dragon_fight;
pub mod event;

pub use custom::{CustomBossEvent, CustomBossEvents, CustomBossEventsData};
pub use dragon_fight::{DragonRespawnStage, EnderDragonFight};
pub use event::{BossEvent, ServerBossEvent};
