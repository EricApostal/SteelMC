//! Clientbound resource pack push packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::config::C_RESOURCE_PACK_PUSH;
use steel_registry::packets::play::C_RESOURCE_PACK_PUSH as PLAY_C_RESOURCE_PACK_PUSH;
use text_components::TextComponent;
use uuid::Uuid;

/// Pushes a resource pack onto the client's stack.
///
/// Mirrors vanilla's `ClientboundResourcePackPushPacket`: UUID, URL
/// (`STRING_UTF8`, max 32767), hash (`stringUtf8(40)`, max 40), required flag
/// and an optional prompt component (presence byte, then the component NBT).
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Config = C_RESOURCE_PACK_PUSH, Play = PLAY_C_RESOURCE_PACK_PUSH)]
pub struct CResourcePackPush {
    /// Pack UUID, used by the client to report the download result.
    pub id: Uuid,
    /// Download URL of the pack.
    #[write(as = Prefixed(VarInt), bound = 32767)]
    pub url: String,
    /// SHA-1 hash of the pack, at most 40 characters (vanilla `MAX_HASH_LENGTH`).
    #[write(as = Prefixed(VarInt), bound = 40)]
    pub hash: String,
    /// Whether the client must accept the pack.
    pub required: bool,
    /// Optional message shown while the client decides.
    pub prompt: Option<TextComponent>,
}

impl CResourcePackPush {
    /// Creates a new resource pack push packet without a prompt.
    ///
    /// # Panics
    /// Panics if `hash` is longer than 40 characters, matching vanilla's
    /// canonical constructor.
    #[must_use]
    pub fn new(id: Uuid, url: String, hash: String, required: bool) -> Self {
        assert!(
            hash.len() <= 40,
            "Hash is too long (max 40, was {})",
            hash.len()
        );
        Self {
            id,
            url,
            hash,
            required,
            prompt: None,
        }
    }

    /// Sets the optional prompt component.
    #[must_use]
    pub fn with_prompt(mut self, prompt: TextComponent) -> Self {
        self.prompt = Some(prompt);
        self
    }
}
