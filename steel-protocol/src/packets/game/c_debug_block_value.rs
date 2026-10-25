//! Clientbound debug block value packet - a debug-renderer subscription value for a block position.
//!
//! This file also hosts the shared debug-subscription model (`DebugSubscriptionUpdate` and, in
//! `c_debug_event.rs`, `DebugSubscriptionEvent`) that vanilla keeps in
//! `net.minecraft.util.debug.DebugSubscription`; the three block/chunk/entity packets all carry an
//! `Update`, so the type is defined once here and imported by its siblings.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_DEBUG_BLOCK_VALUE;
use steel_utils::{BlockPos, serial::WriteTo};

/// A debug subscription value update, mirroring vanilla `DebugSubscription.Update<T>`.
///
/// Vanilla wire format (`DebugSubscription.Update.STREAM_CODEC`):
/// 1. `VarInt` registry ID of the subscription in `minecraft:debug_subscription`.
/// 2. `bool` presence byte - `0` means "no value" (vanilla `Optional.empty()`).
/// 3. When present, the value encoded with that subscription's own stream codec.
///
/// Vanilla resolves the value codec from the registry ID it just read. Steel has no
/// `debug_subscription` registry yet, so the value stays a type parameter and the caller must pair
/// `subscription_id` with the value type whose codec that subscription uses.
#[derive(WriteTo, Clone, Debug)]
pub struct DebugSubscriptionUpdate<V: WriteTo> {
    /// Registry ID of the debug subscription (`Registries.DEBUG_SUBSCRIPTION`).
    #[write(as = VarInt)]
    pub subscription_id: i32,
    /// The subscription value; `None` is vanilla's empty update.
    pub value: Option<V>,
}

impl<V: WriteTo> DebugSubscriptionUpdate<V> {
    /// Creates an update carrying a value (vanilla `DebugSubscription::packUpdate`).
    #[must_use]
    pub const fn new(subscription_id: i32, value: V) -> Self {
        Self {
            subscription_id,
            value: Some(value),
        }
    }

    /// Creates an empty update for the given subscription (vanilla `DebugSubscription::emptyUpdate`).
    #[must_use]
    pub const fn empty(subscription_id: i32) -> Self {
        Self {
            subscription_id,
            value: None,
        }
    }
}

/// Sent to update the client's debug-renderer data for a single block.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_DEBUG_BLOCK_VALUE)]
pub struct CDebugBlockValue<V: WriteTo> {
    /// Position of the block the value belongs to (vanilla writes a packed `i64`).
    pub block_pos: BlockPos,
    /// The subscription value that changed for this block.
    pub update: DebugSubscriptionUpdate<V>,
}

impl<V: WriteTo> CDebugBlockValue<V> {
    /// Creates a new debug block value packet.
    #[must_use]
    pub const fn new(block_pos: BlockPos, update: DebugSubscriptionUpdate<V>) -> Self {
        Self { block_pos, update }
    }
}
