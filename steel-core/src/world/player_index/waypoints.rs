//! Locator-bar waypoint tracking (vanilla `ServerWaypointManager` +
//! `WaypointTransmitter`).
//!
//! Vanilla keeps one `ServerWaypointManager` per level. It tracks every
//! `WaypointTransmitter` (a living entity whose `waypoint_transmit_range`
//! attribute is positive), keeps one connection per (receiver, source) pair, and
//! drives the client's locator bar through `ClientboundTrackedWaypointPacket`.
//! The `locator_bar` game rule gates every connection: with it disabled no
//! player ever receives a tracked waypoint.
//!
//! Steel tracks players as the only waypoint sources. In the generated vanilla
//! entity data only `minecraft:player` has a non-zero
//! `waypoint_transmit_range` (`6.0E7`; every other entity type defaults to
//! `0.0`), so the transmitter set is identical to vanilla for vanilla data. Mob
//! sources that a plugin raises above zero are not tracked yet; the manager API
//! already works on `Player` only.
//!
//! State lives next to the other per-world player bookkeeping in
//! [`PlayerMap`](super::PlayerMap), which is per-level exactly like vanilla's
//! manager.

use std::ptr;

use glam::DVec3;
use rustc_hash::{FxHashMap, FxHashSet};
use steel_protocol::packets::game::{
    CWaypoint, TrackedWaypoint, TrackedWaypointData, WaypointIcon, WaypointIdentifier,
    WaypointOperation,
};
use steel_registry::attribute::AttributeRef;
use steel_registry::vanilla_attributes;
use steel_utils::{BlockPos, ChunkPos, Identifier, locks::SyncMutex};

use crate::entity::{Entity, LivingEntity as _};
use crate::player::Player;
use crate::world::World;

/// Vanilla `WaypointTransmitter.REALLY_FAR_DISTANCE`: beyond this distance a
/// connection degrades to an azimuth-only waypoint.
const REALLY_FAR_DISTANCE: f64 = 332.0;

/// Vanilla `EntityAzimuthConnection.update` angle threshold (`0.5°`).
const AZIMUTH_UPDATE_EPSILON: f32 = 0.008_726_646;

/// Vanilla `Waypoint.Icon` used by entities that never override their icon.
fn default_icon() -> WaypointIcon {
    WaypointIcon {
        style: Identifier::vanilla_static("default"),
        color: None,
    }
}

fn attribute_value(player: &Player, attribute: AttributeRef) -> f64 {
    player
        .attributes()
        .lock()
        .get_value(attribute)
        .unwrap_or(0.0)
}

/// Vanilla `LivingEntity.isTransmittingWaypoint`.
fn is_transmitting(player: &Player) -> bool {
    attribute_value(player, vanilla_attributes::WAYPOINT_TRANSMIT_RANGE) > 0.0
}

/// Vanilla `ServerPlayer.isReceivingWaypoints`.
fn is_receiving(player: &Player) -> bool {
    attribute_value(player, vanilla_attributes::WAYPOINT_RECEIVE_RANGE) > 0.0
}

fn distance(source: &Player, receiver: &Player) -> f64 {
    (source.position() - receiver.position()).length()
}

/// Vanilla `WaypointTransmitter.doesSourceIgnoreReceiver`.
fn source_ignores_receiver(source: &Player, receiver: &Player) -> bool {
    if receiver.is_spectator() {
        return false;
    }
    if !source.is_spectator() && !source.has_indirect_passenger(receiver) {
        let broadcast_range =
            attribute_value(source, vanilla_attributes::WAYPOINT_TRANSMIT_RANGE).min(
                attribute_value(receiver, vanilla_attributes::WAYPOINT_RECEIVE_RANGE),
            );
        return distance(source, receiver) >= broadcast_range;
    }
    true
}

/// Vanilla `WaypointTransmitter.isChunkVisible`.
fn is_chunk_visible(chunk: ChunkPos, receiver: &Player) -> bool {
    receiver
        .last_tracking_view
        .lock()
        .is_some_and(|view| view.contains(chunk))
}

/// Vanilla `WaypointTransmitter.isReallyFar`.
fn is_really_far(source: &Player, receiver: &Player) -> bool {
    distance(source, receiver) > REALLY_FAR_DISTANCE
}

fn chunk_of(player: &Player) -> ChunkPos {
    ChunkPos::from_entity_pos(player.position())
}

/// Vanilla `EntityAzimuthConnection`'s initial angle: the receiver-relative
/// direction rotated clockwise by 90 degrees.
fn azimuth(source: &Player, receiver: &Player) -> f32 {
    let delta = receiver.position() - source.position();
    // Vanilla `Vec3.rotateClockwise90` is `(-z, y, x)`.
    let rotated = DVec3::new(-delta.z, delta.y, delta.x);
    rotated.z.atan2(rotated.x) as f32
}

fn block_distance_manhattan(a: BlockPos, b: BlockPos) -> i32 {
    (a.x() - b.x()).abs() + (a.y() - b.y()).abs() + (a.z() - b.z()).abs()
}

fn chunk_distance_chessboard(a: ChunkPos, b: ChunkPos) -> i32 {
    (a.0.x - b.0.x).abs().max((a.0.y - b.0.y).abs())
}

/// Which flavor of tracked waypoint a connection currently sends.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ConnectionKind {
    /// Vanilla `EntityBlockConnection`: the source is nearby and its chunk is
    /// already visible to the receiver.
    Block { last_position: BlockPos },
    /// Vanilla `EntityChunkConnection`: the source is nearby but its chunk is
    /// outside the receiver's view.
    Chunk { last_chunk: ChunkPos },
    /// Vanilla `EntityAzimuthConnection`: the source is farther than
    /// [`REALLY_FAR_DISTANCE`], so only a direction is sent.
    Azimuth { last_angle: f32 },
}

/// One live (receiver, source) connection.
struct WaypointConnection {
    icon: WaypointIcon,
    kind: ConnectionKind,
}

fn connection_data(kind: ConnectionKind) -> TrackedWaypointData {
    match kind {
        ConnectionKind::Block { last_position } => TrackedWaypointData::Position {
            x: last_position.x(),
            y: last_position.y(),
            z: last_position.z(),
        },
        ConnectionKind::Chunk { last_chunk } => TrackedWaypointData::Chunk {
            x: last_chunk.0.x,
            z: last_chunk.0.y,
        },
        ConnectionKind::Azimuth { last_angle } => TrackedWaypointData::Azimuth(last_angle),
    }
}

fn send_waypoint(
    receiver: &Player,
    source: &Player,
    operation: WaypointOperation,
    icon: &WaypointIcon,
    kind: ConnectionKind,
) {
    receiver.send_packet(CWaypoint::new(
        operation,
        TrackedWaypoint {
            identifier: WaypointIdentifier::Uuid(source.gameprofile.id()),
            icon: icon.clone(),
            data: connection_data(kind),
        },
    ));
}

/// Vanilla `WaypointTransmitter.makeWaypointConnectionWith`.
///
/// Returns the connection flavor the pair should use, or `None` when the
/// source must not be tracked by this receiver.
fn make_connection(source: &Player, receiver: &Player) -> Option<ConnectionKind> {
    if source.is_first_tick() || ptr::eq(source, receiver) {
        return None;
    }
    if source_ignores_receiver(source, receiver) {
        return None;
    }
    if is_really_far(source, receiver) {
        return Some(ConnectionKind::Azimuth {
            last_angle: azimuth(source, receiver),
        });
    }
    let source_chunk = chunk_of(source);
    if is_chunk_visible(source_chunk, receiver) {
        Some(ConnectionKind::Block {
            last_position: source.block_position(),
        })
    } else {
        Some(ConnectionKind::Chunk {
            last_chunk: source_chunk,
        })
    }
}

/// Per-world locator-bar connection state (vanilla `ServerWaypointManager`).
///
/// Receivers and sources are keyed by entity id: ids are unique for the lifetime
/// of a server session and a waypoint never outlives the level it belongs to.
#[derive(Default)]
pub(crate) struct LocatorBar {
    /// Vanilla `ServerWaypointManager.waypoints`: transmitting sources.
    waypoints: FxHashSet<i32>,
    /// Vanilla `ServerWaypointManager.players`: receivers currently subscribed.
    players: FxHashSet<i32>,
    /// Vanilla `ServerWaypointManager.connections`: receiver id → source id → connection.
    connections: FxHashMap<i32, FxHashMap<i32, WaypointConnection>>,
}

impl LocatorBar {
    /// Vanilla `ServerWaypointManager.trackWaypoint`.
    fn track_waypoint(&mut self, world: &World, source: &Player) {
        self.waypoints.insert(source.id());
        for receiver_id in self.players.iter().copied().collect::<Vec<_>>() {
            let Some(receiver) = world.players.get_by_entity_id(receiver_id) else {
                continue;
            };
            self.create_connection(world, &receiver, source);
        }
    }

    /// Vanilla `ServerWaypointManager.untrackWaypoint`.
    fn untrack_waypoint(&mut self, world: &World, source: &Player) {
        self.waypoints.remove(&source.id());
        for receiver_id in self.connections.keys().copied().collect::<Vec<_>>() {
            let Some(connections) = self.connections.get_mut(&receiver_id) else {
                continue;
            };
            if connections.remove(&source.id()).is_none() {
                continue;
            }
            if let Some(receiver) = world.players.get_by_entity_id(receiver_id) {
                receiver.send_packet(CWaypoint::remove(source.gameprofile.id()));
            }
        }
    }

    /// Vanilla `ServerWaypointManager.addPlayer`.
    fn add_player(&mut self, world: &World, receiver: &Player) {
        self.players.insert(receiver.id());
        for source_id in self.waypoints.iter().copied().collect::<Vec<_>>() {
            let Some(source) = world.players.get_by_entity_id(source_id) else {
                continue;
            };
            self.create_connection(world, receiver, &source);
        }
        if is_transmitting(receiver) {
            self.track_waypoint(world, receiver);
        }
    }

    /// Vanilla `ServerWaypointManager.removePlayer`.
    fn remove_player(&mut self, world: &World, player: &Player) {
        if let Some(connections) = self.connections.remove(&player.id()) {
            for source_id in connections.keys() {
                if let Some(source) = world.players.get_by_entity_id(*source_id) {
                    player.send_packet(CWaypoint::remove(source.gameprofile.id()));
                }
            }
        }
        self.players.remove(&player.id());
        if self.waypoints.remove(&player.id()) {
            self.untrack_waypoint(world, player);
        }
    }

    /// Vanilla `ServerWaypointManager.updatePlayer`: refresh every connection
    /// the player holds as a receiver.
    fn update_player(&mut self, world: &World, receiver: &Player) {
        if !self.players.contains(&receiver.id()) {
            return;
        }
        for source_id in self.waypoints.iter().copied().collect::<Vec<_>>() {
            let Some(source) = world.players.get_by_entity_id(source_id) else {
                continue;
            };
            self.update_connection(world, receiver, &source);
        }
    }

    /// Vanilla `ServerWaypointManager.updateWaypoint`: refresh every receiver's
    /// connection to a moved source.
    fn update_waypoint(&mut self, world: &World, source: &Player) {
        if !self.waypoints.contains(&source.id()) {
            return;
        }
        for receiver_id in self.players.iter().copied().collect::<Vec<_>>() {
            let Some(receiver) = world.players.get_by_entity_id(receiver_id) else {
                continue;
            };
            self.update_connection(world, &receiver, source);
        }
    }

    /// Vanilla `ServerWaypointManager.createConnection`.
    ///
    /// Vanilla gates every connection on the receiver's `locator_bar` rule, so a
    /// disabled rule behaves exactly like an untrackable source.
    fn create_connection(&mut self, world: &World, receiver: &Player, source: &Player) {
        if receiver.id() == source.id() {
            return;
        }
        let tracked = if locator_bar_enabled(world) {
            make_connection(source, receiver)
        } else {
            None
        };
        match tracked {
            Some(kind) => {
                let icon = default_icon();
                send_waypoint(receiver, source, WaypointOperation::Track, &icon, kind);
                self.connections
                    .entry(receiver.id())
                    .or_default()
                    .insert(source.id(), WaypointConnection { icon, kind });
            }
            None => {
                let removed = self
                    .connections
                    .get_mut(&receiver.id())
                    .and_then(|connections| connections.remove(&source.id()));
                if removed.is_some() {
                    receiver.send_packet(CWaypoint::remove(source.gameprofile.id()));
                }
            }
        }
    }

    /// Vanilla `ServerWaypointManager.updateConnection`.
    fn update_connection(&mut self, world: &World, receiver: &Player, source: &Player) {
        if receiver.id() == source.id() {
            return;
        }
        let Some(mut connection) = self
            .connections
            .get_mut(&receiver.id())
            .and_then(|connections| connections.remove(&source.id()))
        else {
            return;
        };
        if !locator_bar_enabled(world) {
            receiver.send_packet(CWaypoint::remove(source.gameprofile.id()));
            return;
        }

        let ignores = source_ignores_receiver(source, receiver);
        let source_chunk = chunk_of(source);
        let broken = match connection.kind {
            ConnectionKind::Block { last_position } => {
                ignores || block_distance_manhattan(last_position, source.block_position()) > 1
            }
            ConnectionKind::Chunk { last_chunk } => {
                ignores
                    || chunk_distance_chessboard(last_chunk, source_chunk) > 1
                    || is_chunk_visible(last_chunk, receiver)
            }
            ConnectionKind::Azimuth { .. } => {
                ignores
                    || is_chunk_visible(source_chunk, receiver)
                    || !is_really_far(source, receiver)
            }
        };

        if broken {
            match make_connection(source, receiver) {
                Some(kind) => {
                    let icon = default_icon();
                    send_waypoint(receiver, source, WaypointOperation::Track, &icon, kind);
                    connection = WaypointConnection { icon, kind };
                }
                None => {
                    receiver.send_packet(CWaypoint::remove(source.gameprofile.id()));
                    return;
                }
            }
        } else {
            match &mut connection.kind {
                ConnectionKind::Block { last_position } => {
                    let current = source.block_position();
                    if block_distance_manhattan(*last_position, current) > 0 {
                        send_waypoint(
                            receiver,
                            source,
                            WaypointOperation::Update,
                            &connection.icon,
                            ConnectionKind::Block {
                                last_position: current,
                            },
                        );
                        *last_position = current;
                    }
                }
                ConnectionKind::Chunk { last_chunk } => {
                    if chunk_distance_chessboard(*last_chunk, source_chunk) > 0 {
                        send_waypoint(
                            receiver,
                            source,
                            WaypointOperation::Update,
                            &connection.icon,
                            ConnectionKind::Chunk {
                                last_chunk: source_chunk,
                            },
                        );
                        *last_chunk = source_chunk;
                    }
                }
                ConnectionKind::Azimuth { last_angle } => {
                    let current = azimuth(source, receiver);
                    if (current - *last_angle).abs() > AZIMUTH_UPDATE_EPSILON {
                        send_waypoint(
                            receiver,
                            source,
                            WaypointOperation::Update,
                            &connection.icon,
                            ConnectionKind::Azimuth {
                                last_angle: current,
                            },
                        );
                        *last_angle = current;
                    }
                }
            }
        }

        self.connections
            .entry(receiver.id())
            .or_default()
            .insert(source.id(), connection);
    }
}

impl World {
    /// Registers a player as a locator-bar receiver and, when it transmits,
    /// as a waypoint source (vanilla `ServerWaypointManager.addPlayer` on
    /// tracking start).
    pub(crate) fn locator_bar_add_player(&self, player: &Player) {
        if !is_receiving(player) {
            return;
        }
        self.players.locator_bar().lock().add_player(self, player);
    }

    /// Drops every connection that belongs to a leaving player (vanilla
    /// `ServerWaypointManager.removePlayer`).
    pub(crate) fn locator_bar_remove_player(&self, player: &Player) {
        self.players
            .locator_bar()
            .lock()
            .remove_player(self, player);
    }

    /// Refreshes the locator-bar state after `player` moved: its own
    /// connections as a receiver and every other receiver's connection to it as
    /// a source.
    ///
    /// Vanilla does this from `ServerChunkCache.updatePlayer` every tick;
    /// Steel's only transmitters are players, so refreshing on accepted player
    /// movement observes the same position changes without an extra per-tick
    /// pass over every player.
    pub(crate) fn locator_bar_player_moved(&self, player: &Player) {
        let mut manager = self.players.locator_bar().lock();
        manager.update_player(self, player);
        manager.update_waypoint(self, player);
    }

    /// Applies a `locator_bar` rule change: enabling it re-creates every
    /// connection, disabling it drops them all (vanilla
    /// `ServerWaypointManager.updatePlayer` / `breakAllConnections` from
    /// `MinecraftServer.onGameRuleChanged`).
    pub(crate) fn locator_bar_rule_changed(&self) {
        let enabled = locator_bar_enabled(self);
        let mut manager = self.players.locator_bar().lock();
        if enabled {
            manager.refresh_all(self);
        } else {
            manager.break_all_connections(self);
        }
    }
}

impl LocatorBar {
    /// Vanilla `ServerWaypointManager.updatePlayer` for every receiver.
    fn refresh_all(&mut self, world: &World) {
        for receiver_id in self.players.iter().copied().collect::<Vec<_>>() {
            let Some(receiver) = world.players.get_by_entity_id(receiver_id) else {
                continue;
            };
            for source_id in self.waypoints.iter().copied().collect::<Vec<_>>() {
                let Some(source) = world.players.get_by_entity_id(source_id) else {
                    continue;
                };
                self.create_connection(world, &receiver, &source);
            }
        }
    }

    /// Vanilla `ServerWaypointManager.breakAllConnections`.
    fn break_all_connections(&mut self, world: &World) {
        for (receiver_id, connections) in std::mem::take(&mut self.connections) {
            let Some(receiver) = world.players.get_by_entity_id(receiver_id) else {
                continue;
            };
            for source_id in connections.keys() {
                if let Some(source) = world.players.get_by_entity_id(*source_id) {
                    receiver.send_packet(CWaypoint::remove(source.gameprofile.id()));
                }
            }
        }
    }
}

/// Shared lock handle for the locator-bar state (owned by `PlayerMap`).
pub(crate) type SharedLocatorBar = SyncMutex<LocatorBar>;

/// Whether the `locator_bar` game rule currently allows tracking.
pub(crate) fn locator_bar_enabled(world: &World) -> bool {
    world.get_game_rule(&steel_registry::vanilla_game_rules::LOCATOR_BAR)
}
