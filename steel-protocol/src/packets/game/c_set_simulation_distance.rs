//! Clientbound set simulation distance packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SET_SIMULATION_DISTANCE;

/// Tells the client which simulation distance the server runs with.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_SET_SIMULATION_DISTANCE)]
pub struct CSetSimulationDistance {
    /// Simulation distance in chunks.
    #[write(as = VarInt)]
    pub simulation_distance: i32,
}

impl CSetSimulationDistance {
    /// Creates a new simulation distance packet.
    #[must_use]
    pub const fn new(simulation_distance: i32) -> Self {
        Self {
            simulation_distance,
        }
    }
}
