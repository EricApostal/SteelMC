//! Server event bus for extension listeners.

#![allow(missing_docs, reason = "event payload fields are self-describing")]
//!
//! Vanilla game logic does not depend on the bus: when no listener is
//! registered, each emission is a single empty-slice check. Listeners run on the
//! thread that emitted the event, in ascending priority order. Returning
//! [`EventOutcome::Cancel`] stops later listeners and cancels the action where
//! the emission point supports cancellation.

use std::sync::{Arc, LazyLock};

use steel_utils::{BlockPos, BlockStateId};

use crate::world::World;
use steel_utils::locks::SyncRwLock;

/// Event categories exposed to extensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    BlockBreak,
    BlockPlace,
    PlayerJoin,
    PlayerLeave,
    Chat,
}

impl EventKind {
    /// ABI discriminator used by `steel-abi`.
    #[must_use]
    pub const fn abi_kind(self) -> u32 {
        match self {
            Self::BlockBreak => 1,
            Self::BlockPlace => 2,
            Self::PlayerJoin => 3,
            Self::PlayerLeave => 4,
            Self::Chat => 5,
        }
    }

    /// Resolves an ABI discriminator.
    #[must_use]
    pub const fn from_abi_kind(kind: u32) -> Option<Self> {
        match kind {
            1 => Some(Self::BlockBreak),
            2 => Some(Self::BlockPlace),
            3 => Some(Self::PlayerJoin),
            4 => Some(Self::PlayerLeave),
            5 => Some(Self::Chat),
            _ => None,
        }
    }
}

/// Payload for block-related events.
pub struct BlockEvent {
    pub world: Arc<World>,
    pub pos: BlockPos,
    pub state: BlockStateId,
    pub player_id: Option<i32>,
    pub cancelled: bool,
}

/// Payload for player lifecycle events.
pub struct PlayerEvent {
    pub world: Arc<World>,
    pub player_id: i32,
    pub cancelled: bool,
}

/// Payload for chat events.
pub struct ChatEvent {
    pub world: Arc<World>,
    pub player_id: i32,
    pub message: String,
    pub cancelled: bool,
}

/// A dispatched event.
pub enum Event {
    BlockBreak(BlockEvent),
    BlockPlace(BlockEvent),
    PlayerJoin(PlayerEvent),
    PlayerLeave(PlayerEvent),
    Chat(ChatEvent),
}

impl Event {
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::BlockBreak(_) => EventKind::BlockBreak,
            Self::BlockPlace(_) => EventKind::BlockPlace,
            Self::PlayerJoin(_) => EventKind::PlayerJoin,
            Self::PlayerLeave(_) => EventKind::PlayerLeave,
            Self::Chat(_) => EventKind::Chat,
        }
    }

    #[must_use]
    pub const fn is_cancelled(&self) -> bool {
        match self {
            Self::BlockBreak(event) | Self::BlockPlace(event) => event.cancelled,
            Self::PlayerJoin(event) | Self::PlayerLeave(event) => event.cancelled,
            Self::Chat(event) => event.cancelled,
        }
    }

    pub fn cancel(&mut self) {
        match self {
            Self::BlockBreak(event) | Self::BlockPlace(event) => event.cancelled = true,
            Self::PlayerJoin(event) | Self::PlayerLeave(event) => event.cancelled = true,
            Self::Chat(event) => event.cancelled = true,
        }
    }
}

/// Result returned by a listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventOutcome {
    Continue,
    Cancel,
}

type Callback = Box<dyn Fn(&mut Event) -> EventOutcome + Send + Sync>;

struct Listener {
    kind: EventKind,
    priority: i32,
    callback: Callback,
}

static LISTENERS: LazyLock<SyncRwLock<Vec<Listener>>> =
    LazyLock::new(|| SyncRwLock::new(Vec::new()));

/// Registers a listener. Lower priorities run first.
///
/// Returns `false` when the event kind is unknown.
pub fn register_listener(
    kind: EventKind,
    priority: i32,
    callback: impl Fn(&mut Event) -> EventOutcome + Send + Sync + 'static,
) -> bool {
    let mut guard = LISTENERS.write();
    let listener = Listener {
        kind,
        priority,
        callback: Box::new(callback),
    };
    let index = guard
        .iter()
        .position(|existing| existing.priority > priority)
        .unwrap_or(guard.len());
    guard.insert(index, listener);
    true
}

/// Dispatches an event to all matching listeners.
pub fn dispatch(event: &mut Event) -> EventOutcome {
    let guard = LISTENERS.read();
    if guard.is_empty() {
        return EventOutcome::Continue;
    }
    let kind = event.kind();
    for listener in guard.iter().filter(|listener| listener.kind == kind) {
        if (listener.callback)(event) == EventOutcome::Cancel {
            event.cancel();
            return EventOutcome::Cancel;
        }
    }
    if event.is_cancelled() {
        EventOutcome::Cancel
    } else {
        EventOutcome::Continue
    }
}

/// Fires a block-break event. Returns `true` when the break may proceed.
pub fn fire_block_break(
    world: &Arc<World>,
    pos: BlockPos,
    state: BlockStateId,
    player_id: Option<i32>,
) -> bool {
    if LISTENERS.read().is_empty() {
        return true;
    }
    let mut event = Event::BlockBreak(BlockEvent {
        world: Arc::clone(world),
        pos,
        state,
        player_id,
        cancelled: false,
    });
    dispatch(&mut event) == EventOutcome::Continue
}

/// Fires a block-place event. Returns `true` when placement may proceed.
pub fn fire_block_place(
    world: &Arc<World>,
    pos: BlockPos,
    state: BlockStateId,
    player_id: Option<i32>,
) -> bool {
    if LISTENERS.read().is_empty() {
        return true;
    }
    let mut event = Event::BlockPlace(BlockEvent {
        world: Arc::clone(world),
        pos,
        state,
        player_id,
        cancelled: false,
    });
    dispatch(&mut event) == EventOutcome::Continue
}

/// Fires a player-join event.
pub fn fire_player_join(world: &Arc<World>, player_id: i32) {
    if LISTENERS.read().is_empty() {
        return;
    }
    let mut event = Event::PlayerJoin(PlayerEvent {
        world: Arc::clone(world),
        player_id,
        cancelled: false,
    });
    let _ = dispatch(&mut event);
}

/// Fires a player-leave event.
pub fn fire_player_leave(world: &Arc<World>, player_id: i32) {
    if LISTENERS.read().is_empty() {
        return;
    }
    let mut event = Event::PlayerLeave(PlayerEvent {
        world: Arc::clone(world),
        player_id,
        cancelled: false,
    });
    let _ = dispatch(&mut event);
}

/// Fires a chat event. Returns `true` when the message may be processed.
pub fn fire_chat(world: &Arc<World>, player_id: i32, message: &str) -> bool {
    if LISTENERS.read().is_empty() {
        return true;
    }
    let mut event = Event::Chat(ChatEvent {
        world: Arc::clone(world),
        player_id,
        message: message.to_owned(),
        cancelled: false,
    });
    dispatch(&mut event) == EventOutcome::Continue
}

/// Returns the number of registered listeners for an event kind.
///
/// Intended for host diagnostics and conformance tests.
#[must_use]
pub fn listener_count(kind: EventKind) -> usize {
    LISTENERS
        .read()
        .iter()
        .filter(|listener| listener.kind == kind)
        .count()
}
