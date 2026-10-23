//! Steel adaptation of vanilla's raid-status behaviors (`SetRaidStatus`,
//! `ResetRaidStatus`).
//!
//! Both are vanilla brain behaviors that mutate the villager's `Brain` from inside a
//! behavior tick. Steel's [`BehaviorContext`](crate::entity::ai::brain::behavior::BehaviorContext)
//! deliberately borrows only the memory store — the brain itself is locked by
//! `Brain::tick` — so a behaviour cannot re-enter it. The mob's brain tick therefore calls
//! these helpers from where it already holds `&mut Brain`, right before the behavior tree
//! runs, which keeps the exact vanilla roll, predicates and activity switches.

use steel_utils::BlockPos;

use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::activity::Activity;
use crate::world::World;

use super::raids::RaidSnapshot;

/// Vanilla `SetRaidStatus.create` body.
///
/// Rolls `1/20` per tick and switches the villager between the raid and pre-raid
/// activities. Returns whether the roll happened, matching the vanilla behaviour's result.
pub fn set_raid_status(brain: &mut Brain, raid: Option<RaidSnapshot>) -> bool {
    if rand::random_range(0..20) != 0 {
        return false;
    }

    let Some(raid) = raid else {
        return true;
    };

    if raid.has_first_wave_spawned() && !raid.is_between_waves() {
        brain.set_default_activity(Activity::Raid);
        brain.set_active_activity_if_possible(Activity::Raid);
    } else {
        brain.set_default_activity(Activity::PreRaid);
        brain.set_active_activity_if_possible(Activity::PreRaid);
    }
    true
}

/// Vanilla `ResetRaidStatus.create` body.
///
/// Rolls `1/20` per tick and returns to the schedule when there is no live raid.
pub fn reset_raid_status(brain: &mut Brain, world: &World, pos: BlockPos) -> bool {
    if rand::random_range(0..20) != 0 {
        return false;
    }

    let raid = world.get_raid_at(pos);
    if raid.is_none_or(|raid| raid.is_stopped() || raid.is_loss()) {
        brain.set_default_activity(Activity::Idle);
        brain.update_activity_from_schedule(world);
    }
    true
}
