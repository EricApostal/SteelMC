//! Clientbound update enabled features packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_UPDATE_ENABLED_FEATURES;
use steel_utils::Identifier;

/// Tells the client which feature flags the server has enabled.
///
/// Mirrors vanilla's `ClientboundUpdateEnabledFeaturesPacket`, whose features
/// are encoded by `Identifier.STREAM_CODEC.apply(ByteBufCodecs.collection(
/// HashSet::new))`: a `VarInt` element count followed by each identifier as a
/// VarInt-prefixed UTF-8 string. Vanilla models the field as a set, so entries
/// are unique and their wire order is not significant.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_UPDATE_ENABLED_FEATURES)]
pub struct CUpdateEnabledFeatures {
    /// Enabled feature flag identifiers, e.g. `minecraft:vanilla`.
    pub features: Vec<Identifier>,
}

impl CUpdateEnabledFeatures {
    /// Creates a new update enabled features packet.
    #[must_use]
    pub const fn new(features: Vec<Identifier>) -> Self {
        Self { features }
    }
}
