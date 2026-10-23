//! Serverbound packet for locking the world difficulty.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_LOCK_DIFFICULTY;

/// Sent when the player locks the difficulty from the options screen.
///
/// Matches vanilla's `ServerboundLockDifficultyPacket`: a single boolean.
/// The server must validate permissions before applying the change.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_LOCK_DIFFICULTY)]
pub struct SLockDifficulty {
    /// Whether the difficulty should be locked.
    pub locked: bool,
}
