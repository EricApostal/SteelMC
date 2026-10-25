//! Clientbound debug sample packet - sampled values for the client's debug chart.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_DEBUG_SAMPLE;

/// Remote debug sample kinds, mirroring vanilla `RemoteDebugSampleType`.
///
/// Vanilla writes the value with `FriendlyByteBuf.writeEnum`, i.e. the ordinal as a `VarInt`.
/// Vanilla's single constant maps to the `minecraft:dedicated_server_tick_time` subscription.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum RemoteDebugSampleType {
    /// Dedicated-server tick time samples (vanilla ordinal 0).
    TickTime = 0,
}

/// Sent with a batch of samples for the client's F3 debug chart.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_DEBUG_SAMPLE)]
pub struct CDebugSample {
    /// The sampled values, written as vanilla's `long[]`: `VarInt` count followed by one `i64`
    /// per sample.
    pub sample: Vec<i64>,
    /// Which sampler produced the samples.
    pub debug_sample_type: RemoteDebugSampleType,
}

impl CDebugSample {
    /// Creates a new debug sample packet.
    #[must_use]
    pub const fn new(sample: Vec<i64>, debug_sample_type: RemoteDebugSampleType) -> Self {
        Self {
            sample,
            debug_sample_type,
        }
    }
}
