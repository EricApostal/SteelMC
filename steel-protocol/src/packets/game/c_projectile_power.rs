//! Clientbound projectile power packet - updates a projectile's acceleration power.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PROJECTILE_POWER;

/// Sets the acceleration power of a projectile on the client.
///
/// Vanilla uses a hand-written codec (`Packet.codec`); the wire shape is a
/// `VarInt` entity id followed by a `double`.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_PROJECTILE_POWER)]
pub struct CProjectilePower {
    /// Entity id of the projectile.
    #[write(as = VarInt)]
    pub id: i32,
    /// Acceleration power applied per tick.
    pub acceleration_power: f64,
}
