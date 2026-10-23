//! Serverbound packet acknowledging the configuration phase.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_CONFIGURATION_ACKNOWLEDGED;

/// Sent when the client acknowledges that it is ready to leave the play
/// state and re-enter configuration.
///
/// Matches vanilla's `ServerboundConfigurationAcknowledgedPacket`: a unit
/// packet with no payload. Vanilla marks it terminal.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_CONFIGURATION_ACKNOWLEDGED)]
pub struct SConfigurationAcknowledged {}
