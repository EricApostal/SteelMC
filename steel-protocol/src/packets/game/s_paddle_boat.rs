//! Serverbound packet for boat paddling input.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_PADDLE_BOAT;

/// Sent when the player presses or releases a paddle while rowing a boat.
///
/// Matches vanilla's `ServerboundPaddleBoatPacket`: the left-paddle state
/// followed by the right-paddle state, each a single boolean byte.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_PADDLE_BOAT)]
pub struct SPaddleBoat {
    /// Whether the left paddle is currently pressed.
    pub left: bool,
    /// Whether the right paddle is currently pressed.
    pub right: bool,
}
