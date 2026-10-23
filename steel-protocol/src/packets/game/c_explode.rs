//! Clientbound explode packet - shows an explosion and destroys blocks client-side.

use glam::DVec3;
use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_EXPLODE;
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventHolder;

/// Block-destroy particle entry for the explosion.
///
/// Mirrors vanilla's `ExplosionParticleInfo` (particle plus its scaling and speed),
/// paired with its `WeightedList` weight. Vanilla's `WeightedList.streamCodec`
/// writes a `VarInt`-prefixed list of `Weighted` entries, each encoded as the
/// element followed by its `VarInt` weight, so the weight lives on the same struct.
#[derive(WriteTo, Clone, Debug)]
pub struct ExplosionBlockParticle {
    /// Particle spawned for destroyed blocks.
    pub particle: ParticleData,
    /// Particle scaling.
    pub scaling: f32,
    /// Particle speed.
    pub speed: f32,
    /// Selection weight of this entry.
    #[write(as = VarInt)]
    pub weight: i32,
}

/// Shows an explosion on the client.
///
/// Vanilla builds this with a composed `STREAM_CODEC` over `Vec3`, radius, block
/// count, an optional knockback vector, the explosion particle, the explosion
/// sound holder, the weighted block-particle list, and the play-sound flag. The
/// optional knockback and the particle list both use `bool`-prefixed /
/// `VarInt`-prefixed shapes, matching Steel's `Option<T>` and `Vec<T>` writes.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_EXPLODE)]
pub struct CExplode {
    /// Explosion center.
    pub center: DVec3,
    /// Explosion radius.
    pub radius: f32,
    /// Number of blocks destroyed (client looks them up from its own world).
    pub block_count: i32,
    /// Knockback applied to the player, if the player is affected.
    pub player_knockback: Option<DVec3>,
    /// Explosion particle.
    pub explosion_particle: ParticleData,
    /// Explosion sound.
    pub explosion_sound: SoundEventHolder,
    /// Weighted block-destroy particles.
    pub block_particles: Vec<ExplosionBlockParticle>,
    /// Whether the client plays `explosion_sound` (false when handled otherwise).
    pub play_sound: bool,
}
