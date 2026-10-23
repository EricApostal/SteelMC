//! Shared container-viewer accounting.
//!
//! Mirrors vanilla `ContainerOpenersCounter`: block entities that light up,
//! sound off, or emit redstone while a player has their container open keep one
//! of these and drive it from the menu open/close path.
//!
//! Vanilla's counter is single-threaded and calls its subclass hooks inline.
//! Steel's block entities are shared across threads and those hooks write to the
//! world, so every method here only computes the transition under the counter's
//! own lock and returns it; the owning block entity runs the hooks afterwards,
//! with no lock held.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::{BlockPos, BlockStateId};

use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `ContainerOpenersCounter.CHECK_TICK_DELAY`.
const CHECK_TICK_DELAY: i32 = 5;

/// Vanilla `ContainerOpenersCounter`'s four subclass hooks.
///
/// Implemented by the block entity that owns a [`ContainerOpenersCounter`];
/// the counter only tracks the count and ordering, exactly like vanilla.
pub trait ContainerOpenersHost: Send + Sync {
    /// Vanilla `onOpen`.
    fn on_open(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId);

    /// Vanilla `onClose`.
    fn on_close(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId);

    /// Vanilla `openerCountChanged`.
    fn opener_count_changed(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        previous: i32,
        current: i32,
    );

    /// Vanilla `isOwnContainer`: whether `player`'s open menu is backed by this
    /// block entity (directly or as one half of a combined container).
    fn is_own_container(&self, player: &Player) -> bool;
}

/// Container-viewer capability exposed by container block entities.
///
/// Vanilla's `startOpen`/`stopOpen`/`getOpenCount`/`recheckOpeners` surface,
/// reachable from the menu lifecycle without downcasting the block entity.
pub trait ContainerViewers: Send + Sync {
    /// Vanilla `startOpen`.
    fn start_open(&self, player: &Player);

    /// Vanilla `stopOpen`.
    fn stop_open(&self, player: &Player);

    /// Vanilla `ContainerOpenersCounter.getOpenerCount`.
    fn open_count(&self) -> i32;

    /// Vanilla `recheckOpeners`, driven from the block's scheduled tick.
    fn recheck_open(&self);
}

/// What changed in the opener count, so the caller can run the vanilla hooks
/// after releasing the counter lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenerTransition {
    /// Count before the change.
    pub previous: i32,
    /// Count after the change.
    pub current: i32,
    /// Vanilla `onOpen` should run.
    pub opened: bool,
    /// Vanilla `onClose` should run.
    pub closed: bool,
    /// Vanilla `scheduleRecheck` should run.
    pub schedule_recheck: bool,
}

impl OpenerTransition {
    /// Whether vanilla `openerCountChanged` should run.
    #[must_use]
    pub const fn count_changed(&self) -> bool {
        self.previous != self.current
    }
}

/// Counts the players that still have `host`'s container open and are in range.
///
/// Mirrors vanilla `ContainerOpenersCounter.getEntitiesWithContainerOpen` +
/// `recheckOpeners` counting. Callers must not hold the counter lock.
pub fn count_viewers<H: ContainerOpenersHost + ?Sized>(
    world: &Arc<World>,
    pos: BlockPos,
    host: &H,
) -> i32 {
    let mut open_count = 0;
    world.players.iter_players(|_, player| {
        if player.game_mode() == steel_utils::types::GameType::Spectator
            || !player.is_within_block_interaction_range_with_buffer(pos, 4.0)
            || !host.is_own_container(player)
        {
            return true;
        }
        open_count += 1;
        true
    });
    open_count
}

/// Tracks how many players currently have one container open.
#[derive(Debug, Default)]
pub struct ContainerOpenersCounter {
    open_count: i32,
    max_interaction_range: f64,
}

impl ContainerOpenersCounter {
    /// Creates an empty counter.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            open_count: 0,
            max_interaction_range: 0.0,
        }
    }

    /// The number of players currently holding this container open.
    #[must_use]
    pub const fn open_count(&self) -> i32 {
        self.open_count
    }

    /// Vanilla `incrementOpeners` bookkeeping.
    pub fn increment(&mut self, interaction_range: f64) -> OpenerTransition {
        let previous = self.open_count;
        self.open_count += 1;
        self.max_interaction_range = self.max_interaction_range.max(interaction_range);
        OpenerTransition {
            previous,
            current: self.open_count,
            opened: previous == 0,
            closed: false,
            schedule_recheck: previous == 0,
        }
    }

    /// Vanilla `decrementOpeners` bookkeeping.
    pub fn decrement(&mut self) -> OpenerTransition {
        let previous = self.open_count;
        self.open_count -= 1;
        if self.open_count == 0 {
            self.max_interaction_range = 0.0;
        }
        OpenerTransition {
            previous,
            current: self.open_count,
            opened: false,
            closed: self.open_count == 0,
            schedule_recheck: false,
        }
    }

    /// Vanilla `recheckOpeners` bookkeeping for an already-counted viewer list.
    pub fn recheck(&mut self, open_count: i32) -> OpenerTransition {
        let previous = self.open_count;
        let was_open = previous != 0;
        let is_open = open_count != 0;
        if previous != open_count {
            self.open_count = open_count;
            if !is_open {
                self.max_interaction_range = 0.0;
            }
        }
        OpenerTransition {
            previous,
            current: open_count,
            opened: is_open && !was_open,
            closed: !is_open && was_open,
            schedule_recheck: open_count > 0,
        }
    }

    /// Runs the vanilla counter hooks for a transition.
    ///
    /// Callers must not hold the counter lock: the hooks write to the world.
    pub fn apply<H: ContainerOpenersHost + ?Sized>(
        host: &H,
        transition: OpenerTransition,
        player: Option<&Player>,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) {
        let context = GameEventContext::new(player.map(|p| p as &dyn crate::entity::Entity), None);
        if transition.opened {
            host.on_open(world, pos, state);
            world.game_event(
                &steel_registry::vanilla_game_events::CONTAINER_OPEN,
                pos,
                &context,
            );
        } else if transition.closed {
            host.on_close(world, pos, state);
            world.game_event(
                &steel_registry::vanilla_game_events::CONTAINER_CLOSE,
                pos,
                &context,
            );
        }

        host.opener_count_changed(world, pos, state, transition.previous, transition.current);

        if transition.schedule_recheck {
            world.schedule_block_tick_default(pos, state.get_block(), CHECK_TICK_DELAY);
        }
    }
}
