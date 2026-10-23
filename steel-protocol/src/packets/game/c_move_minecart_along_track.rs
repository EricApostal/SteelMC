//! Clientbound move minecart packet - replays minecart track interpolation steps.

use std::io::{self, Write};

use glam::DVec3;
use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_MOVE_MINECART_ALONG_TRACK;
use steel_utils::serial::WriteTo as WriteToTrait;

/// One minecart track interpolation step.
///
/// Mirrors vanilla's `NewMinecartBehavior.MinecartStep`: the client chains the
/// steps to lerp position, movement, and rotation.
#[derive(Clone, Copy, Debug)]
pub struct MinecartStep {
    /// Interpolated position.
    pub position: DVec3,
    /// Interpolated movement (delta).
    pub movement: DVec3,
    /// Yaw in degrees.
    pub y_rot: f32,
    /// Pitch in degrees.
    pub x_rot: f32,
    /// Step weight used to pace the interpolation.
    pub weight: f32,
}

impl MinecartStep {
    /// Creates a new minecart step.
    #[must_use]
    pub const fn new(
        position: DVec3,
        movement: DVec3,
        y_rot: f32,
        x_rot: f32,
        weight: f32,
    ) -> Self {
        Self {
            position,
            movement,
            y_rot,
            x_rot,
            weight,
        }
    }
}

impl WriteToTrait for MinecartStep {
    fn write(&self, writer: &mut impl Write) -> io::Result<()> {
        // Vanilla `MinecartStep.STREAM_CODEC` order: position, movement,
        // ROTATION_BYTE yaw, ROTATION_BYTE pitch, float weight.
        self.position.write(writer)?;
        self.movement.write(writer)?;
        super::to_angle_byte(self.y_rot).write(writer)?;
        super::to_angle_byte(self.x_rot).write(writer)?;
        self.weight.write(writer)
    }
}

/// Replays minecart track interpolation steps on the client.
///
/// The client chains `lerp_steps` to lerp the minecart along the track.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_MOVE_MINECART_ALONG_TRACK)]
pub struct CMoveMinecartAlongTrack {
    /// Entity id of the minecart.
    #[write(as = VarInt)]
    pub entity_id: i32,
    /// Interpolation steps the client replays in order.
    pub lerp_steps: Vec<MinecartStep>,
}
