//! Vanilla `BreakDoorGoal` and `Vindicator.VindicatorBreakDoorGoal`: a raider smashes a
//! wooden door in its path instead of opening it.
//!
//! The door bookkeeping lives in [`DoorInteractGoal`], whose API is module-private to
//! `entity::ai::goal`; this goal embeds it and drives it through those accessors, exactly
//! like vanilla's `BreakDoorGoal extends DoorInteractGoal`.
//!
//! Deliberate difference: vanilla swings `Mob.getUsedItemHand()`, Steel always swings the
//! main hand because mobs do not track a used-item hand.

use glam::DVec3;
use steel_registry::level_events;
use steel_registry::vanilla_game_rules;
use steel_utils::types::{Difficulty, InteractionHand};

use super::door_interact::DoorInteractGoal;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `BreakDoorGoal.DEFAULT_DOOR_BREAK_TIME`.
const DEFAULT_DOOR_BREAK_TIME: i32 = 240;
/// Vanilla `BreakDoorGoal.tick`: one door hit (sound and swing) every 20 ticks.
const DOOR_HIT_INTERVAL: i32 = 20;
/// Vanilla `BreakDoorGoal.tick` scales break progress to `0..=9` for the client.
const BREAK_PROGRESS_STEPS: f32 = 10.0;
/// Vanilla `DoorInteractGoal` reach: `doorPos.closerToCenterThan(mob.position(), 2.0)`.
const DOOR_REACH_DISTANCE: f64 = 2.0;
/// Vanilla `BreakDoorGoal` clear-progress value (`destroyBlockProgress(..., -1)`).
const CLEAR_BREAK_PROGRESS: i32 = -1;

/// Vanilla `BreakDoorGoal`.
pub struct BreakDoorGoal {
    door_interact: DoorInteractGoal,
    difficulty_predicate: fn(Difficulty) -> bool,
    break_time: i32,
    last_break_progress: i32,
    door_break_time: i32,
}

impl BreakDoorGoal {
    /// Vanilla `new BreakDoorGoal(mob, validDifficulties)`.
    #[must_use]
    pub const fn new(difficulty_predicate: fn(Difficulty) -> bool) -> Self {
        Self {
            door_interact: DoorInteractGoal::new(),
            difficulty_predicate,
            break_time: 0,
            last_break_progress: -1,
            door_break_time: -1,
        }
    }

    /// Vanilla `new BreakDoorGoal(mob, seconds, validDifficulties)`.
    #[must_use]
    pub const fn with_door_break_time(
        door_break_time: i32,
        difficulty_predicate: fn(Difficulty) -> bool,
    ) -> Self {
        Self {
            door_interact: DoorInteractGoal::new(),
            difficulty_predicate,
            break_time: 0,
            last_break_progress: -1,
            door_break_time,
        }
    }

    /// Vanilla `BreakDoorGoal.getDoorBreakTime`.
    #[must_use]
    pub const fn door_break_time(&self) -> i32 {
        if DEFAULT_DOOR_BREAK_TIME > self.door_break_time {
            DEFAULT_DOOR_BREAK_TIME
        } else {
            self.door_break_time
        }
    }

    /// Vanilla `BreakDoorGoal.isValidDifficulty`.
    #[must_use]
    fn is_valid_difficulty(&self, difficulty: Difficulty) -> bool {
        (self.difficulty_predicate)(difficulty)
    }

    /// Vanilla `DoorInteractGoal.doorPos.closerToCenterThan(mob.position(), 2.0)`.
    fn is_within_door_reach(&self, mob: &dyn PathfinderMob) -> bool {
        let door_pos = self.door_interact.door_pos();
        let center = DVec3::new(
            f64::from(door_pos.x()) + 0.5,
            f64::from(door_pos.y()) + 0.5,
            f64::from(door_pos.z()) + 0.5,
        );
        mob.position().distance_squared(center) < DOOR_REACH_DISTANCE * DOOR_REACH_DISTANCE
    }

    /// Vanilla `BreakDoorGoal.tick`.
    fn tick_break(&mut self, mob: &dyn PathfinderMob) {
        self.door_interact.tick(mob);

        let Some(world) = mob.level() else {
            return;
        };
        let door_pos = self.door_interact.door_pos();

        if rand::random_range(0..DOOR_HIT_INTERVAL) == 0 {
            world.level_event(level_events::SOUND_ZOMBIE_WOODEN_DOOR, door_pos, 0, None);
            if !mob.living_swing_state().swinging() {
                mob.swing_for_attack(InteractionHand::MainHand);
            }
        }

        self.break_time += 1;
        let progress =
            (self.break_time as f32 / self.door_break_time() as f32 * BREAK_PROGRESS_STEPS) as i32;
        if progress != self.last_break_progress {
            world.broadcast_block_destruction(mob.id(), door_pos, progress);
            self.last_break_progress = progress;
        }

        if self.break_time == self.door_break_time() && self.is_valid_difficulty(world.difficulty())
        {
            world.remove_block(door_pos, false);
            world.level_event(level_events::SOUND_ZOMBIE_DOOR_CRASH, door_pos, 0, None);
            let state = world.get_block_state(door_pos);
            world.destroy_block_effect(door_pos, u32::from(state.0), None);
        }
    }
}

impl Goal for BreakDoorGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !self.door_interact.can_use(mob) {
            return false;
        }
        let Some(world) = mob.level() else {
            return false;
        };
        world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING)
            && self.is_valid_difficulty(world.difficulty())
            && !self.door_interact.is_open(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        self.break_time <= self.door_break_time()
            && !self.door_interact.is_open(mob)
            && self.is_within_door_reach(mob)
            && self.is_valid_difficulty(world.difficulty())
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.door_interact.start(mob);
        self.break_time = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let Some(world) = mob.level() else {
            return;
        };
        world.broadcast_block_destruction(
            mob.id(),
            self.door_interact.door_pos(),
            CLEAR_BREAK_PROGRESS,
        );
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        self.tick_break(mob);
    }
}

/// Vanilla `Vindicator.VindicatorBreakDoorGoal`: only a raiding vindicator breaks doors, and
/// only on normal or hard difficulty.
pub struct VindicatorBreakDoorGoal {
    inner: BreakDoorGoal,
}

/// Vanilla `Vindicator.DOOR_BREAKING_PREDICATE`.
const fn vindicator_door_breaking_predicate(difficulty: Difficulty) -> bool {
    matches!(difficulty, Difficulty::Normal | Difficulty::Hard)
}

impl VindicatorBreakDoorGoal {
    /// Vanilla `new VindicatorBreakDoorGoal(mob)`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            inner: BreakDoorGoal::with_door_break_time(6, vindicator_door_breaking_predicate),
        }
    }
}

impl Default for VindicatorBreakDoorGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for VindicatorBreakDoorGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let raiding = mob
            .as_raider()
            .is_some_and(|raider| raider.has_active_raid(&world));
        raiding && rand::random_range(0..reduced_tick_delay(10)) == 0 && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        mob.as_raider()
            .is_some_and(|raider| raider.has_active_raid(&world))
            && self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
        mob.set_no_action_time(0);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn break_door_goal_matches_vanilla_timing_and_flags() {
        let goal = BreakDoorGoal::new(|_| true);

        assert_eq!(goal.controls(), GoalControls::EMPTY);
        assert!(goal.requires_update_every_tick());
        // Vanilla `getDoorBreakTime` = max(240, doorBreakTime).
        assert_eq!(goal.door_break_time(), DEFAULT_DOOR_BREAK_TIME);
    }

    #[test]
    fn door_break_seconds_never_shorten_the_default_break_time() {
        let goal = BreakDoorGoal::with_door_break_time(600, |_| true);
        assert_eq!(goal.door_break_time(), 600);

        let goal = BreakDoorGoal::with_door_break_time(6, |_| true);
        assert_eq!(goal.door_break_time(), DEFAULT_DOOR_BREAK_TIME);
    }

    #[test]
    fn vindicator_break_door_goal_gates_on_raid_and_claims_move() {
        let goal = VindicatorBreakDoorGoal::new();

        assert_eq!(goal.controls(), GoalControls::MOVE);
        assert!(goal.requires_update_every_tick());
        assert_eq!(goal.inner.door_break_time(), DEFAULT_DOOR_BREAK_TIME);
        assert!((goal.inner.difficulty_predicate)(Difficulty::Normal));
        assert!((goal.inner.difficulty_predicate)(Difficulty::Hard));
        assert!(!(goal.inner.difficulty_predicate)(Difficulty::Easy));
        assert!(!(goal.inner.difficulty_predicate)(Difficulty::Peaceful));
    }
}
