//! Clientbound sound entity packet - plays a sound attached to an entity.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SOUND_ENTITY;
use steel_registry::sound_event::SoundEventHolder;

use super::SoundSource;

/// Plays a sound effect that follows an entity on the client.
///
/// Vanilla uses a hand-written codec (`Packet.codec`), but the field order and
/// widths are identical to a composed codec: registry-holder sound, `SoundSource`
/// enum, entity id `VarInt`, volume, pitch, and seed.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_SOUND_ENTITY)]
pub struct CSoundEntity {
    /// The holder-encoded sound event.
    pub sound: SoundEventHolder,
    /// The sound source category.
    pub source: SoundSource,
    /// Entity id the sound is attached to.
    #[write(as = VarInt)]
    pub id: i32,
    /// Volume (1.0 = normal).
    pub volume: f32,
    /// Pitch (1.0 = normal).
    pub pitch: f32,
    /// Random seed for sound variations.
    pub seed: i64,
}
