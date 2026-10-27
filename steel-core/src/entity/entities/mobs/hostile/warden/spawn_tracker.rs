//! Vanilla `WardenSpawnTracker`: shrieker escalation and the `spawn_wardens` gate.
//!
//! Vanilla keeps one tracker per `ServerPlayer` and persists three counters
//! (`ticks_since_last_warning`, `warning_level`, `cooldown_ticks`). Steel stores
//! the tracker as a plain value type; the per-player home is
//! `ServerPlayer.wardenSpawnTracker` in vanilla, which Steel does not have yet
//! (reported dependency: `Player` field + player-data persistence).

use std::sync::Arc;

use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_rules::SPAWN_WARDENS;
use steel_utils::WorldAabb;
use steel_utils::types::Difficulty;

use super::warden::WardenEntity;
use crate::entity::spawn::create_entity_instance;
use crate::entity::{Entity, EntitySpawnReason, SharedEntity};
use crate::player::Player;
use crate::world::World;
use steel_utils::Downcast as _;

/// Vanilla `WardenSpawnTracker.MAX_WARNING_LEVEL`.
pub const MAX_WARNING_LEVEL: i32 = 4;
/// Vanilla `WardenSpawnTracker.PLAYER_SEARCH_RADIUS`.
pub const PLAYER_SEARCH_RADIUS: f64 = 16.0;
/// Vanilla `WardenSpawnTracker.WARNING_CHECK_DIAMETER`.
pub const WARNING_CHECK_DIAMETER: f64 = 48.0;
/// Vanilla `WardenSpawnTracker.DECREASE_WARNING_LEVEL_EVERY_INTERVAL`.
pub const DECREASE_WARNING_LEVEL_EVERY_INTERVAL: i32 = 12_000;
/// Vanilla `WardenSpawnTracker.WARNING_LEVEL_INCREASE_COOLDOWN`.
pub const WARNING_LEVEL_INCREASE_COOLDOWN: i32 = 200;

/// Vanilla `WardenSpawnTracker`: the per-player shrieker warning escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WardenSpawnTracker {
    ticks_since_last_warning: i32,
    warning_level: i32,
    cooldown_ticks: i32,
}

impl WardenSpawnTracker {
    /// Vanilla `WardenSpawnTracker(int, int, int)`.
    #[must_use]
    pub const fn new(
        ticks_since_last_warning: i32,
        warning_level: i32,
        cooldown_ticks: i32,
    ) -> Self {
        Self {
            ticks_since_last_warning,
            warning_level,
            cooldown_ticks,
        }
    }

    /// Vanilla `WardenSpawnTracker()`.
    #[must_use]
    pub const fn fresh() -> Self {
        Self::new(0, 0, 0)
    }

    /// Vanilla `ticks_since_last_warning`.
    #[must_use]
    pub const fn ticks_since_last_warning(&self) -> i32 {
        self.ticks_since_last_warning
    }

    /// Vanilla `warning_level`.
    #[must_use]
    pub const fn warning_level(&self) -> i32 {
        self.warning_level
    }

    /// Vanilla `cooldown_ticks`.
    #[must_use]
    pub const fn cooldown_ticks(&self) -> i32 {
        self.cooldown_ticks
    }

    /// Vanilla `WardenSpawnTracker.tick`.
    pub fn tick(&mut self) {
        if self.ticks_since_last_warning >= DECREASE_WARNING_LEVEL_EVERY_INTERVAL {
            self.decrease_warning_level();
            self.ticks_since_last_warning = 0;
        } else {
            self.ticks_since_last_warning += 1;
        }

        if self.cooldown_ticks > 0 {
            self.cooldown_ticks -= 1;
        }
    }

    /// Vanilla `WardenSpawnTracker.reset`.
    pub const fn reset(&mut self) {
        self.ticks_since_last_warning = 0;
        self.warning_level = 0;
        self.cooldown_ticks = 0;
    }

    /// Vanilla `WardenSpawnTracker.onCooldown`.
    #[must_use]
    pub const fn on_cooldown(&self) -> bool {
        self.cooldown_ticks > 0
    }

    /// Vanilla `WardenSpawnTracker.setWarningLevel`: clamps into `0..=4`.
    pub fn set_warning_level(&mut self, warning_level: i32) {
        self.warning_level = warning_level.clamp(0, MAX_WARNING_LEVEL);
    }

    /// Vanilla `WardenSpawnTracker.increaseWarningLevel`.
    fn increase_warning_level(&mut self) {
        if !self.on_cooldown() {
            self.ticks_since_last_warning = 0;
            self.cooldown_ticks = WARNING_LEVEL_INCREASE_COOLDOWN;
            self.set_warning_level(self.warning_level + 1);
        }
    }

    /// Vanilla `WardenSpawnTracker.decreaseWarningLevel`.
    fn decrease_warning_level(&mut self) {
        self.set_warning_level(self.warning_level - 1);
    }

    /// Vanilla `WardenSpawnTracker.copyData`.
    pub const fn copy_data(&mut self, from: Self) {
        self.warning_level = from.warning_level;
        self.cooldown_ticks = from.cooldown_ticks;
        self.ticks_since_last_warning = from.ticks_since_last_warning;
    }

    /// Vanilla `WardenSpawnTracker.canSpawn`: the `spawn_wardens` game rule is
    /// the only gate on summoning a warden from a shrieker.
    #[must_use]
    pub fn can_spawn(world: &World) -> bool {
        world.get_game_rule(&SPAWN_WARDENS)
    }

    /// Vanilla `WardenSpawnTracker.tryWarn`: escalates the nearby players' trackers and returns
    /// the new warning level, or `None` when the shriek must not escalate (nearby warden, any
    /// participating player on cooldown, or `spawn_wardens` off).
    ///
    /// Vanilla reads and writes `ServerPlayer.wardenSpawnTracker` directly; Steel's per-player
    /// tracker lives on [`Player`], so no tracker slice has to be threaded through the caller.
    #[must_use]
    pub fn try_warn(
        world: &Arc<World>,
        pos: steel_utils::BlockPos,
        trigger_player: &Arc<Player>,
    ) -> Option<i32> {
        if !Self::can_spawn(world) || Self::has_nearby_warden(world, pos) {
            return None;
        }

        // Vanilla adds the trigger player only when the nearby search missed it.
        let mut participants = Self::nearby_players(world, pos);
        if !participants
            .iter()
            .any(|player| player.uuid() == trigger_player.uuid())
        {
            participants.push(Arc::clone(trigger_player));
        }

        // Vanilla aborts when any participating player is on cooldown.
        if participants
            .iter()
            .any(|player| player.warden_spawn_tracker().on_cooldown())
        {
            return None;
        }

        // Vanilla takes the participant with the highest warning level (ties keep the first) and
        // copies its escalated state onto every participant.
        let (first, rest) = participants.split_first()?;
        let mut best = first;
        for player in rest {
            if player.warden_spawn_tracker().warning_level()
                > best.warden_spawn_tracker().warning_level()
            {
                best = player;
            }
        }
        let mut escalated = best.warden_spawn_tracker();
        escalated.increase_warning_level();
        for player in &participants {
            player.warden_spawn_tracker_mut().copy_data(escalated);
        }
        Some(escalated.warning_level())
    }

    /// Vanilla `WardenSpawnTracker.hasNearbyWarden`.
    #[must_use]
    pub fn has_nearby_warden(world: &Arc<World>, pos: steel_utils::BlockPos) -> bool {
        let center = pos_center(pos);
        let box_ = WorldAabb::of_size(
            center,
            WARNING_CHECK_DIAMETER,
            WARNING_CHECK_DIAMETER,
            WARNING_CHECK_DIAMETER,
        );
        world
            .get_entities_in_aabb_matching(&box_, |entity| {
                entity.entity_type() == &vanilla_entities::WARDEN
            })
            .iter()
            .any(|entity| entity.downcast_ref::<WardenEntity>().is_some())
    }

    /// Vanilla `WardenSpawnTracker.getNearbyPlayers`.
    #[must_use]
    fn nearby_players(world: &Arc<World>, pos: steel_utils::BlockPos) -> Vec<Arc<Player>> {
        let center = pos_center(pos);
        let mut players = Vec::new();
        world.players.iter_players(|_, player| {
            if !player.is_spectator()
                && player.is_alive()
                && player.position().distance(center) <= PLAYER_SEARCH_RADIUS
            {
                players.push(Arc::clone(player));
            }
            true
        });
        players
    }

    /// Vanilla `WardenSpawnTracker` spawn hook: summon a warden at `pos` when
    /// the game rule allows it and the tracker reached the final warning level.
    ///
    /// Vanilla summons from `SculkShriekerBlockEntity.trySummonWarden`.
    pub fn summon_warden(world: &Arc<World>, pos: steel_utils::BlockPos) -> Option<SharedEntity> {
        if !Self::can_spawn(world) || world.difficulty() == Difficulty::Peaceful {
            return None;
        }
        let spawn = glam::DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + 1.0,
            f64::from(pos.z()) + 0.5,
        );
        let warden = create_entity_instance(world, &vanilla_entities::WARDEN, spawn).ok()?;
        warden.snap_to(spawn, rand::random::<f32>() * 360.0, 0.0);
        // Vanilla `SpawnUtil.trySpawnMob` → `EntityType.create` → `Mob.finalizeSpawn` with the
        // `TRIGGERED` reason, before the entity joins the level. `Warden.finalizeSpawn` sets the
        // `EMERGING` pose, remembers `IS_EMERGING` for `WardenAi.EMERGE_DURATION` and plays the
        // agitated emerge sound, so the emerge activity can run.
        if let Some(mob) = warden.as_mob() {
            mob.finalize_spawn(world, EntitySpawnReason::Triggered, None);
        }
        if world.try_add_entity(Arc::clone(&warden)).is_ok() {
            Some(warden)
        } else {
            None
        }
    }
}

fn pos_center(pos: steel_utils::BlockPos) -> glam::DVec3 {
    glam::DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    )
}
