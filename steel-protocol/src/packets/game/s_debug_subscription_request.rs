//! Serverbound packet subscribing to server-side debug samples.

use std::io::Cursor;

use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::play::S_DEBUG_SUBSCRIPTION_REQUEST;
use steel_utils::codec::VarInt;

/// Maximum subscriptions in one packet (vanilla `ByteBufCodecs.collection(..., 32)`).
pub const MAX_DEBUG_SUBSCRIPTIONS: usize = 32;

/// Sent when the client changes which server debug samples it wants to receive.
///
/// Matches vanilla's `ServerboundDebugSubscriptionRequestPacket`: a `VarInt`
/// count (max 32, like every other bounded collection codec) followed by that
/// many raw `minecraft:debug_subscription` registry ids as `VarInt`s.
///
/// Note on encoding: `ByteBufCodecs.registry(...)` on the wire is just the
/// numeric registry id written with `VarInt.write` — the same direct-id form
/// Steel uses for attribute ids in `c_update_attributes.rs` —
/// not the holder `id + 1` form, and not a namespaced id.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_DEBUG_SUBSCRIPTION_REQUEST)]
pub struct SDebugSubscriptionRequest {
    /// Raw `minecraft:debug_subscription` registry ids to subscribe to.
    /// Sending an empty list unsubscribes from everything.
    pub subscriptions: Vec<i32>,
}

impl steel_utils::serial::ReadFrom for SDebugSubscriptionRequest {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        // Vanilla `ByteBufCodecs.readCount(input, 32)`: only counts above the
        // maximum are rejected, and a non-positive count decodes to an empty set.
        let count = VarInt::read(data)?.0;
        if count > MAX_DEBUG_SUBSCRIPTIONS as i32 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "debug subscription count out of bounds",
            ));
        }
        let mut subscriptions = Vec::with_capacity(count.max(0) as usize);
        for _ in 0..count {
            subscriptions.push(VarInt::read(data)?.0);
        }
        Ok(Self { subscriptions })
    }
}
