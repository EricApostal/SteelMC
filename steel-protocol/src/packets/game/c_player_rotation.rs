//! Clientbound player rotation packet - rotates the client's player.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PLAYER_ROTATION;

/// Rotates the player's view on the client.
///
/// Each rotation carries its own relative flag, following the same convention as
/// [`CPlayerPosition`](super::CPlayerPosition): when a flag is set the angle is
/// relative to the player's current rotation, otherwise it is absolute.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_PLAYER_ROTATION)]
pub struct CPlayerRotation {
    /// Target yaw (Y rotation).
    pub y_rot: f32,
    /// Whether `y_rot` is relative to the current yaw.
    pub relative_y: bool,
    /// Target pitch (X rotation).
    pub x_rot: f32,
    /// Whether `x_rot` is relative to the current pitch.
    pub relative_x: bool,
}

impl CPlayerRotation {
    /// Creates a new player rotation packet.
    #[must_use]
    pub const fn new(y_rot: f32, relative_y: bool, x_rot: f32, relative_x: bool) -> Self {
        Self {
            y_rot,
            relative_y,
            x_rot,
            relative_x,
        }
    }
}
