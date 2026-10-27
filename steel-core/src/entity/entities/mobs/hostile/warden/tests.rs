//! Wardens' effects layer: the emerge/dig/sniff/roar sounds the AI behaviors
//! emit, the sonic-boom origin and the sonic-charge entity event.
//!
//! Every assertion here drives the real behavior against a real
//! [`WardenEntity`] in a test world and reads the packets that reach a nearby
//! player, so the `Entity::play_sound` / `Entity::broadcast_entity_event` paths
//! are exercised rather than a stub.

use std::io::Cursor;
use std::sync::Arc;

use glam::{DVec3, IVec3};
use steel_protocol::packet_traits::EncodedPacket;
use steel_registry::packets::play::{C_ENTITY_EVENT, C_SOUND};
use steel_registry::sound_event::SoundEventHolder;
use steel_registry::{sound_events, vanilla_entities};
use steel_utils::codec::VarInt;
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::serial::ReadFrom as _;
use steel_utils::{BlockPos, ChunkPos};

use super::WardenEntity;
use crate::bootstrap::init_globals;
use crate::entity::Entity as _;
use crate::entity::SharedEntity;
use crate::entity::ai::behavior::warden as warden_ai;
use crate::entity::ai::brain::behavior::{Behavior as _, BehaviorContext};
use crate::entity::ai::brain::memory::MemoryStore;
use crate::player::{PlayerConnection, ResetReason};
use crate::test_support::{
    TestConnection, TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
};
use crate::world::World;

/// The warden stands at the center of chunk `(0, 0)`; the observer stands on top
/// of it so every emitted sound and event is in range.
const WARDEN_ORIGIN: DVec3 = DVec3::new(8.5, 64.0, 8.5);

struct Fixture {
    world: TestWorld,
    warden: SharedEntity,
    packets: Arc<SyncMutex<Vec<EncodedPacket>>>,
}

fn setup(key: &'static str) -> Fixture {
    init_globals();
    let world = fresh_test_world(key);
    insert_ready_full_chunk(
        &world.world,
        ChunkPos::from_block_pos(BlockPos::new(8, 64, 8)),
    );

    let connection = TestConnection::default();
    let packets = connection.sent_handle();
    let player = TestPlayerBuilder::new(Arc::clone(&world.world), "WardenObserver", 1)
        .connection(Arc::new(PlayerConnection::Other(Box::new(connection))))
        .build();
    player.set_client_loaded(true);
    player.base().set_position_local(WARDEN_ORIGIN);
    assert!(
        world
            .world
            .add_player(Arc::clone(&player), ResetReason::InitialJoin)
    );
    // The entity-event broadcast only reaches players whose client already
    // holds the base chunk packet.
    player
        .chunk_sender()
        .lock()
        .mark_chunk_sent_for_test(ChunkPos::from_block_pos(BlockPos::new(8, 64, 8)));

    let warden: SharedEntity = Arc::new(WardenEntity::new(
        &vanilla_entities::WARDEN,
        2,
        WARDEN_ORIGIN,
        Arc::downgrade(&world.world),
    ));

    packets.lock().clear();
    Fixture {
        world,
        warden,
        packets,
    }
}

impl Fixture {
    fn level(&self) -> &World {
        &self.world.world
    }

    /// Runs `f` with a fresh [`BehaviorContext`] over the warden body.
    fn with_context(&self, f: impl FnOnce(&mut BehaviorContext<'_>)) {
        let mut memories = MemoryStore::new();
        let mut ctx = BehaviorContext::new(&mut memories, self.level(), &self.warden, 0);
        f(&mut ctx);
    }
}

/// Decodes every `C_SOUND` packet a player received into `(sound, volume, pitch)`.
fn played_sounds(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<(String, f32, f32)> {
    packets
        .lock()
        .iter()
        .filter_map(|packet| {
            let mut cursor = Cursor::new(packet.encoded_data.as_slice());
            VarInt::read(&mut cursor).ok()?;
            if VarInt::read(&mut cursor).ok()?.0 != C_SOUND {
                return None;
            }
            let holder = SoundEventHolder::read(&mut cursor).ok()?;
            let sound = holder.registry_ref()?;
            VarInt::read(&mut cursor).ok()?; // sound source category
            IVec3::read(&mut cursor).ok()?; // fixed-point position
            let volume = f32::read(&mut cursor).ok()?;
            let pitch = f32::read(&mut cursor).ok()?;
            Some((sound.key.to_string(), volume, pitch))
        })
        .collect()
}

/// Decodes every `C_ENTITY_EVENT` packet a player received into its event byte.
fn entity_events(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<i8> {
    packets
        .lock()
        .iter()
        .filter_map(|packet| {
            let mut cursor = Cursor::new(packet.encoded_data.as_slice());
            VarInt::read(&mut cursor).ok()?;
            if VarInt::read(&mut cursor).ok()?.0 != C_ENTITY_EVENT {
                return None;
            }
            i32::read(&mut cursor).ok()?; // entity id
            i8::read(&mut cursor).ok()
        })
        .collect()
}

#[test]
fn digging_plays_the_vanilla_dig_sound_on_the_ground() {
    let fixture = setup("warden_dig_sound");
    fixture.with_context(|ctx| {
        ctx.body.set_on_ground(true);
        warden_ai::digging::Digging::new(warden_ai::DIGGING_DURATION).start(ctx);
    });

    assert_eq!(
        played_sounds(&fixture.packets),
        vec![(sound_events::ENTITY_WARDEN_DIG.key.to_string(), 5.0, 1.0)]
    );
}

#[test]
fn digging_plays_the_agitated_sound_and_discards_off_the_ground() {
    let fixture = setup("warden_dig_agitated_sound");
    fixture.with_context(|ctx| {
        ctx.body.set_on_ground(false);
        warden_ai::digging::Digging::new(warden_ai::DIGGING_DURATION).start(ctx);
    });

    assert_eq!(
        played_sounds(&fixture.packets),
        vec![(
            sound_events::ENTITY_WARDEN_AGITATED.key.to_string(),
            5.0,
            1.0
        )]
    );
    assert!(fixture.warden.removal_reason().is_some());
}

#[test]
fn emerging_plays_the_vanilla_emerge_sound() {
    let fixture = setup("warden_emerge_sound");
    fixture.with_context(|ctx| {
        warden_ai::emerging::Emerging::new(warden_ai::EMERGE_DURATION).start(ctx);
    });

    assert_eq!(
        played_sounds(&fixture.packets),
        vec![(sound_events::ENTITY_WARDEN_EMERGE.key.to_string(), 5.0, 1.0)]
    );
}

#[test]
fn sniffing_plays_the_vanilla_sniff_sound() {
    let fixture = setup("warden_sniff_sound");
    fixture.with_context(|ctx| {
        warden_ai::sniffing::Sniffing::new(warden_ai::SNIFFING_DURATION).start(ctx);
    });

    assert_eq!(
        played_sounds(&fixture.packets),
        vec![(sound_events::ENTITY_WARDEN_SNIFF.key.to_string(), 5.0, 1.0)]
    );
}

#[test]
fn roaring_plays_the_vanilla_roar_sound() {
    let fixture = setup("warden_roar_sound");
    fixture.with_context(|ctx| {
        warden_ai::roar::Roar::new().tick(ctx);
    });

    assert_eq!(
        played_sounds(&fixture.packets),
        vec![(sound_events::ENTITY_WARDEN_ROAR.key.to_string(), 3.0, 1.0)]
    );
}

#[test]
fn sonic_boom_start_broadcasts_the_sonic_charge_entity_event() {
    let fixture = setup("warden_sonic_charge_event");
    fixture.with_context(|ctx| {
        warden_ai::sonic_boom::SonicBoom::new().start(ctx);
    });

    assert_eq!(
        entity_events(&fixture.packets),
        vec![EntityStatus::SonicCharge as i8]
    );
    assert_eq!(
        played_sounds(&fixture.packets),
        vec![(
            sound_events::ENTITY_WARDEN_SONIC_CHARGE.key.to_string(),
            3.0,
            1.0
        )]
    );
}

#[test]
fn sonic_boom_origin_is_the_feet_position_plus_the_warden_chest_attachment() {
    let fixture = setup("warden_sonic_origin");
    fixture.with_context(|ctx| {
        let origin = warden_ai::sonic_boom_origin(ctx);
        let chest = warden_ai::warden_chest_attachment(ctx);
        let feet = ctx.body.position();

        // Vanilla sums the *feet* position with the `WARDEN_CHEST` attachment
        // point, which is `(0.0, 1.6, 0.0)` for a 2.9-tall warden (entities.json).
        assert_eq!(origin, feet + chest);
        assert!(
            (origin.y - (feet.y + 1.6)).abs() < 1e-6,
            "the beam must start 1.6 blocks above the feet, got {}",
            origin.y - feet.y
        );

        // The old eye-derived origin sat 2.465 blocks higher (the warden's eye height).
        let eye_origin = warden_ai::eye_position(&fixture.warden) + chest;
        assert!(
            (origin.y - eye_origin.y).abs() > 2.0,
            "the origin must not be measured from the eye position"
        );
    });
}
