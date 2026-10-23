//! Vanilla raid system (`net.minecraft.world.entity.raid`): the per-level manager, one
//! raid's wave state machine, and the `Raider` capability shared by illagers, ravagers
//! and witches.
//!
//! Steel keeps the manager inside [`World`](crate::world::World) behind a lock, so raid
//! mutation happens on the world tick and read-only consumers use [`RaidSnapshot`].

pub mod behavior;
pub mod patrol;
pub mod raid;
pub mod raider;
pub mod raids;

pub use patrol::PatrolSpawner;

pub use raid::{
    DEFAULT_MAX_RAID_OMEN_LEVEL, DEFAULT_PRE_RAID_TICKS, MAX_CELEBRATION_TICKS, MAX_NO_ACTION_TIME,
    RAID_REMOVAL_THRESHOLD_SQR, Raid, RaidBar, RaidStatus, RaiderType, VALID_RAID_RADIUS_SQR,
    VILLAGE_RADIUS_BUFFER, ravager_spawn_position_ok, village_raid_center,
};
pub use raider::{
    Raider, RaiderBase, RaiderState, pillager_raid_enchant_provider,
    vindicator_raid_enchant_provider,
};
pub use raids::{RaidRecord, RaidSnapshot, Raids, RaidsData, can_join_raid};

#[cfg(test)]
mod tests;
