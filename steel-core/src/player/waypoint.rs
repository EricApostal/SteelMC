//! Locator-bar waypoint tracking (vanilla `ServerWaypointManager` and the
//! `WaypointTransmitter` connections for players).
//!
//! Vanilla keeps one connection per (receiver, transmitter) pair inside the
//! level's waypoint manager. Each connection sends `ClientboundTrackedWaypointPacket`
//! operations: `TRACK` on connect, `UPDATE` when the transmitted block position,
//! chunk or azimuth changes, and `UNTRACK` on disconnect.
//!
//! Steel stores the connections on the receiving player, so the tracking state is
//! dropped with the player and needs no separate manager owner.

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_protocol::packets::game::{
    CWaypoint, TrackedWaypoint, TrackedWaypointData, WaypointIcon, WaypointIdentifier,
    WaypointOperation,
};
use steel_registry::vanilla_attributes;
use steel_registry::vanilla_game_rules::LOCATOR_BAR;
use steel_utils::{ChunkPos, Identifier};
use uuid::Uuid;

use crate::entity::{Entity as _, LivingEntity as _};
use crate::player::Player;

/// Vanilla `WaypointTransmitter.REALLY_FAR_DISTANCE`: beyond this the client is
/// given an azimuth instead of a position.
const REALLY_FAR_DISTANCE: f64 = 332.0;

/// Vanilla `EntityAzimuthConnection` update threshold (0.5 degrees in radians).
const AZIMUTH_UPDATE_THRESHOLD: f32 = 0.008_726_646;

/// One live waypoint connection held by a receiving player.
#[derive(Debug, Clone)]
pub(crate) struct WaypointConnection {
    /// Connection kind currently in use.
    kind: ConnectionKind,
    /// Last block position sent to the client.
    last_block_pos: (i32, i32, i32),
    /// Last chunk sent to the client.
    last_chunk: (i32, i32),
    /// Last azimuth sent to the client, in radians.
    last_angle: f32,
}

/// Vanilla `WaypointTransmitter` connection implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionKind {
    /// `EntityBlockConnection`: the source chunk is visible to the receiver.
    Block,
    /// `EntityChunkConnection`: the source chunk is outside the receiver's view.
    Chunk,
    /// `EntityAzimuthConnection`: the source is really far away.
    Azimuth,
}

impl ConnectionKind {
    const fn waypoint_data(
        self,
        block_pos: (i32, i32, i32),
        chunk: (i32, i32),
        angle: f32,
    ) -> TrackedWaypointData {
        match self {
            Self::Block => TrackedWaypointData::Position {
                x: block_pos.0,
                y: block_pos.1,
                z: block_pos.2,
            },
            Self::Chunk => TrackedWaypointData::Chunk {
                x: chunk.0,
                z: chunk.1,
            },
            Self::Azimuth => TrackedWaypointData::Azimuth(angle),
        }
    }
}

impl Player {
    /// Returns the vanilla `waypoint_transmit_range` attribute value.
    fn waypoint_transmit_range(&self) -> f64 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::WAYPOINT_TRANSMIT_RANGE)
            .unwrap_or(0.0)
    }

    /// Returns the vanilla `waypoint_receive_range` attribute value.
    fn waypoint_receive_range(&self) -> f64 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::WAYPOINT_RECEIVE_RANGE)
            .unwrap_or(0.0)
    }

    /// Vanilla `LivingEntity.isTransmittingWaypoint`.
    #[must_use]
    pub fn is_transmitting_waypoint(&self) -> bool {
        self.waypoint_transmit_range() > 0.0
    }

    /// Vanilla `ServerPlayer.isReceivingWaypoints`.
    #[must_use]
    pub fn is_receiving_waypoints(&self) -> bool {
        self.waypoint_receive_range() > 0.0
    }

    /// Vanilla `ServerWaypointManager.updatePlayer` for a single receiver.
    ///
    /// Runs once per tick: connects new transmitters, updates existing
    /// connections and drops the ones vanilla would break.
    pub(crate) fn tick_waypoints(&self) {
        let world = self.get_world();
        let locator_bar_enabled = world.get_game_rule(&LOCATOR_BAR);
        let receiving = locator_bar_enabled
            && self.is_receiving_waypoints()
            && !self.is_removed()
            && !self.is_spectator();

        if !receiving {
            self.untrack_all_waypoints();
            return;
        }

        let mut seen = Vec::new();
        world.players.iter_players(|uuid, transmitter| {
            if transmitter.gameprofile.id() == self.gameprofile.id() {
                return true;
            }
            seen.push(*uuid);
            self.update_waypoint_connection(transmitter);
            true
        });

        // Vanilla `ServerWaypointManager.removePlayer`/`untrackWaypoint` drops
        // connections whose transmitter left the level.
        let stale: Vec<Uuid> = self
            .waypoint_tracks
            .lock()
            .keys()
            .copied()
            .filter(|uuid| !seen.contains(uuid))
            .collect();
        for uuid in stale {
            self.send_packet(CWaypoint::remove(uuid));
            self.waypoint_tracks.lock().remove(&uuid);
        }
    }

    /// Vanilla `ServerWaypointManager.createConnection` + `updateConnection`.
    fn update_waypoint_connection(&self, transmitter: &Player) {
        let Some(desired) = self.desired_waypoint_connection(transmitter) else {
            if self
                .waypoint_tracks
                .lock()
                .remove(&transmitter.gameprofile.id())
                .is_some()
            {
                self.send_packet(CWaypoint::remove(transmitter.gameprofile.id()));
            }
            return;
        };

        let uuid = transmitter.gameprofile.id();
        let block_pos = transmitter.block_position();
        let block_pos = (block_pos.x(), block_pos.y(), block_pos.z());
        let chunk = ChunkPos::from_entity_pos(transmitter.position());
        let chunk = (chunk.0.x, chunk.0.y);
        let angle = self.waypoint_azimuth(transmitter);

        let mut tracks = self.waypoint_tracks.lock();
        let icon = waypoint_icon();

        match tracks.get_mut(&uuid) {
            None => {
                let data = ConnectionKind::waypoint_data(desired, block_pos, chunk, angle);
                self.send_packet(CWaypoint::new(
                    WaypointOperation::Track,
                    TrackedWaypoint {
                        identifier: WaypointIdentifier::Uuid(uuid),
                        icon,
                        data,
                    },
                ));
                tracks.insert(
                    uuid,
                    WaypointConnection {
                        kind: desired,
                        last_block_pos: block_pos,
                        last_chunk: chunk,
                        last_angle: angle,
                    },
                );
            }
            Some(connection) => {
                let changed_kind = connection.kind != desired;
                let changed_payload = match desired {
                    ConnectionKind::Block => connection.last_block_pos != block_pos,
                    ConnectionKind::Chunk => connection.last_chunk != chunk,
                    ConnectionKind::Azimuth => {
                        (angle - connection.last_angle).abs() > AZIMUTH_UPDATE_THRESHOLD
                    }
                };
                if !changed_kind && !changed_payload {
                    return;
                }

                if changed_kind {
                    // A different connection shape needs a fresh `TRACK`; vanilla
                    // disconnects the broken connection first.
                    self.send_packet(CWaypoint::remove(uuid));
                }

                let data = ConnectionKind::waypoint_data(desired, block_pos, chunk, angle);
                self.send_packet(CWaypoint::new(
                    if changed_kind {
                        WaypointOperation::Track
                    } else {
                        WaypointOperation::Update
                    },
                    TrackedWaypoint {
                        identifier: WaypointIdentifier::Uuid(uuid),
                        icon,
                        data,
                    },
                ));
                connection.kind = desired;
                connection.last_block_pos = block_pos;
                connection.last_chunk = chunk;
                connection.last_angle = angle;
            }
        }
    }

    /// Drops every waypoint connection this player holds.
    fn untrack_all_waypoints(&self) {
        let tracks = std::mem::take(&mut *self.waypoint_tracks.lock());
        for uuid in tracks.keys() {
            self.send_packet(CWaypoint::remove(*uuid));
        }
    }

    /// Returns the connection kind vanilla would use for `transmitter`, or `None`
    /// when `doesSourceIgnoreReceiver` suppresses the waypoint.
    fn desired_waypoint_connection(&self, transmitter: &Player) -> Option<ConnectionKind> {
        if !transmitter.is_transmitting_waypoint() {
            return None;
        }
        // Vanilla `WaypointTransmitter.doesSourceIgnoreReceiver`.
        if self.is_spectator() {
            // Spectator receivers still see waypoints.
        } else if !transmitter.is_spectator()
            && transmitter
                .passengers()
                .iter()
                .all(|passenger| passenger.id() != self.id())
        {
            let broadcast_range = transmitter
                .waypoint_transmit_range()
                .min(self.waypoint_receive_range());
            if transmitter.position().distance(self.position()) >= broadcast_range {
                return None;
            }
        } else {
            return None;
        }

        if transmitter.position().distance(self.position()) > REALLY_FAR_DISTANCE {
            return Some(ConnectionKind::Azimuth);
        }

        let chunk = ChunkPos::from_entity_pos(transmitter.position());
        let chunk_visible = self
            .last_tracking_view
            .lock()
            .as_ref()
            .is_some_and(|view| view.contains(chunk));
        if chunk_visible {
            Some(ConnectionKind::Block)
        } else {
            Some(ConnectionKind::Chunk)
        }
    }

    /// Vanilla `EntityAzimuthConnection`: the angle is measured from the
    /// transmitter to the receiver, rotated a quarter turn clockwise.
    fn waypoint_azimuth(&self, transmitter: &Player) -> f32 {
        let direction = rotate_clockwise_90(self.position() - transmitter.position());
        direction.z.atan2(direction.x) as f32
    }
}

/// Vanilla `Vec3.rotateClockwise90`: `(x, y, z)` becomes `(-z, y, x)`.
fn rotate_clockwise_90(vector: DVec3) -> DVec3 {
    DVec3::new(-vector.z, vector.y, vector.x)
}

/// Vanilla `Waypoint.Icon` for a player with no team color or custom style.
fn waypoint_icon() -> WaypointIcon {
    WaypointIcon {
        style: Identifier::vanilla_static("default"),
        color: None,
    }
}

/// The per-player waypoint connection table.
pub(crate) type WaypointTracks = FxHashMap<Uuid, WaypointConnection>;

/// Shared access for the player struct's lock field.
pub(crate) fn new_tracks() -> WaypointTracks {
    FxHashMap::default()
}
