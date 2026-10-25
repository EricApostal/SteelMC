//! Clientbound post effects packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_POST_EFFECTS;
use steel_registry::packets::play::C_POST_EFFECTS as PLAY_C_POST_EFFECTS;
use steel_utils::Identifier;

/// Replaces the client's post-effect chain.
///
/// Mirrors vanilla's `ClientboundPostEffectsPacket`: `Identifier.STREAM_CODEC
/// .apply(ByteBufCodecs.list())`, i.e. a `VarInt` element count followed by each
/// identifier as a VarInt-prefixed UTF-8 string.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_POST_EFFECTS, Play = PLAY_C_POST_EFFECTS)]
pub struct CPostEffects {
    /// Post-effect identifiers, in the order the client should apply them.
    pub post_effects: Vec<Identifier>,
}

impl CPostEffects {
    /// Creates a new post effects packet.
    #[must_use]
    pub const fn new(post_effects: Vec<Identifier>) -> Self {
        Self { post_effects }
    }
}
