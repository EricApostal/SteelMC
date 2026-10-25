//! Clientbound debug event packet - a one-shot debug-renderer subscription event.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_DEBUG_EVENT;
use steel_utils::serial::WriteTo;

/// A debug subscription event, mirroring vanilla `DebugSubscription.Event<T>`.
///
/// Vanilla wire format (`DebugSubscription.Event.STREAM_CODEC`):
/// 1. `VarInt` registry ID of the subscription in `minecraft:debug_subscription`.
/// 2. The value, encoded with that subscription's own stream codec (never optional, unlike the
///    `DebugSubscriptionUpdate` carried by block/chunk/entity value packets).
///
/// Vanilla resolves the value codec from the registry ID it just read. Steel has no
/// `debug_subscription` registry yet, so the value stays a type parameter and the caller must pair
/// `subscription_id` with the value type whose codec that subscription uses.
#[derive(WriteTo, Clone, Debug)]
pub struct DebugSubscriptionEvent<V: WriteTo> {
    /// Registry ID of the debug subscription (`Registries.DEBUG_SUBSCRIPTION`).
    #[write(as = VarInt)]
    pub subscription_id: i32,
    /// The event's value.
    pub value: V,
}

impl<V: WriteTo> DebugSubscriptionEvent<V> {
    /// Creates an event for the given subscription (vanilla `DebugSubscription::packEvent`).
    #[must_use]
    pub const fn new(subscription_id: i32, value: V) -> Self {
        Self {
            subscription_id,
            value,
        }
    }
}

/// Sent to fire a one-shot debug-renderer event.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_DEBUG_EVENT)]
pub struct CDebugEvent<V: WriteTo> {
    /// The event that was fired.
    pub event: DebugSubscriptionEvent<V>,
}

impl<V: WriteTo> CDebugEvent<V> {
    /// Creates a new debug event packet.
    #[must_use]
    pub const fn new(event: DebugSubscriptionEvent<V>) -> Self {
        Self { event }
    }
}
