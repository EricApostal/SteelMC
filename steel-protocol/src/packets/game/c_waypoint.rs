//! Clientbound tracked waypoint packet - locator-bar waypoint add/update/remove.

use std::io::{Result, Write};

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_WAYPOINT;
use steel_utils::{
    Identifier, RgbColor,
    codec::VarInt,
    serial::{PrefixedWrite, WriteTo},
};
use uuid::Uuid;

/// Action the client applies to its waypoint manager (vanilla
/// `ClientboundTrackedWaypointPacket.Operation` declaration order).
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum WaypointOperation {
    /// Starts tracking a new waypoint (vanilla `TRACK`).
    Track = 0,
    /// Stops tracking the waypoint (vanilla `UNTRACK`).
    Untrack = 1,
    /// Refreshes an already tracked waypoint (vanilla `UPDATE`).
    Update = 2,
}

/// Kind discriminator of a tracked waypoint (vanilla `TrackedWaypoint.Type`
/// declaration order). Written between the icon and the kind-specific payload.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum TrackedWaypointKind {
    /// No payload; used by the remove operation.
    Empty = 0,
    /// Payload is a block position.
    Position = 1,
    /// Payload is a chunk column.
    Chunk = 2,
    /// Payload is an azimuth angle.
    Azimuth = 3,
}

/// Waypoint identifier: vanilla writes an `Either<UUID, String>`.
///
/// The discriminant is a single byte: `true` selects the UUID branch (16 bytes),
/// `false` selects the length-prefixed UTF-8 string branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaypointIdentifier {
    /// Waypoint owned by an entity/player, keyed by its UUID.
    Uuid(Uuid),
    /// Waypoint addressed by an arbitrary name string.
    Text(String),
}

impl WriteTo for WaypointIdentifier {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match self {
            Self::Uuid(uuid) => {
                true.write(writer)?;
                uuid.write(writer)
            }
            Self::Text(text) => {
                false.write(writer)?;
                text.write_prefixed::<VarInt>(writer)
            }
        }
    }
}

/// Icon rendered for a waypoint (vanilla `Waypoint.Icon`).
///
/// Wire order: the `minecraft:waypoint_style_asset` resource location
/// (length-prefixed UTF-8), then a presence byte followed by three raw
/// RGB channel bytes when a tint is set (vanilla `ByteBufCodecs.RGB_COLOR`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaypointIcon {
    /// `minecraft:waypoint_style_asset` resource location; vanilla default is
    /// `minecraft:default`.
    pub style: Identifier,
    /// Optional RGB tint packed as a vanilla ARGB integer; only the three color
    /// channels are sent.
    pub color: Option<RgbColor>,
}

impl WriteTo for WaypointIcon {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.style.write(writer)?;
        match self.color {
            Some(color) => {
                true.write(writer)?;
                color.red().write(writer)?;
                color.green().write(writer)?;
                color.blue().write(writer)
            }
            None => false.write(writer),
        }
    }
}

/// Kind-specific payload of a tracked waypoint.
#[derive(Clone, Debug, PartialEq)]
pub enum TrackedWaypointData {
    /// No payload (vanilla `EmptyWaypoint`).
    Empty,
    /// A block position, written as three VarInts (vanilla `Vec3iWaypoint`).
    Position {
        /// Block X coordinate.
        x: i32,
        /// Block Y coordinate.
        y: i32,
        /// Block Z coordinate.
        z: i32,
    },
    /// A chunk column, written as two VarInts (vanilla `ChunkWaypoint`).
    Chunk {
        /// Chunk X coordinate.
        x: i32,
        /// Chunk Z coordinate.
        z: i32,
    },
    /// A yaw angle in radians, written as an `f32` (vanilla `AzimuthWaypoint`).
    Azimuth(f32),
}

impl TrackedWaypointData {
    /// Returns the discriminator written before the payload.
    #[must_use]
    pub const fn kind(&self) -> TrackedWaypointKind {
        match self {
            Self::Empty => TrackedWaypointKind::Empty,
            Self::Position { .. } => TrackedWaypointKind::Position,
            Self::Chunk { .. } => TrackedWaypointKind::Chunk,
            Self::Azimuth(_) => TrackedWaypointKind::Azimuth,
        }
    }
}

impl WriteTo for TrackedWaypointData {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match self {
            Self::Empty => Ok(()),
            Self::Position { x, y, z } => {
                VarInt(*x).write(writer)?;
                VarInt(*y).write(writer)?;
                VarInt(*z).write(writer)
            }
            Self::Chunk { x, z } => {
                VarInt(*x).write(writer)?;
                VarInt(*z).write(writer)
            }
            Self::Azimuth(angle) => angle.write(writer),
        }
    }
}

/// A waypoint the client tracks (vanilla `TrackedWaypoint`).
///
/// Wire order: identifier, icon, kind discriminator, then the kind payload.
#[derive(Clone, Debug, PartialEq)]
pub struct TrackedWaypoint {
    /// Who the waypoint belongs to.
    pub identifier: WaypointIdentifier,
    /// Icon rendered for the waypoint.
    pub icon: WaypointIcon,
    /// Position/angle payload.
    pub data: TrackedWaypointData,
}

impl WriteTo for TrackedWaypoint {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.identifier.write(writer)?;
        self.icon.write(writer)?;
        self.data.kind().write(writer)?;
        self.data.write(writer)
    }
}

/// Adds, updates, or removes a client-tracked waypoint.
///
/// Wire order: VarInt operation ordinal, then the [`TrackedWaypoint`] encoding.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_WAYPOINT)]
pub struct CWaypoint {
    /// Operation the client performs on its waypoint manager.
    pub operation: WaypointOperation,
    /// Waypoint the operation applies to.
    pub waypoint: TrackedWaypoint,
}

impl CWaypoint {
    /// Creates a tracked waypoint packet.
    #[must_use]
    pub const fn new(operation: WaypointOperation, waypoint: TrackedWaypoint) -> Self {
        Self {
            operation,
            waypoint,
        }
    }

    /// Creates a packet that stops tracking a waypoint.
    #[must_use]
    pub const fn remove(identifier: Uuid) -> Self {
        Self::new(
            WaypointOperation::Untrack,
            TrackedWaypoint {
                identifier: WaypointIdentifier::Uuid(identifier),
                icon: WaypointIcon {
                    style: Identifier::vanilla_static("default"),
                    color: None,
                },
                data: TrackedWaypointData::Empty,
            },
        )
    }
}

impl WriteTo for CWaypoint {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.operation.write(writer)?;
        self.waypoint.write(writer)
    }
}
