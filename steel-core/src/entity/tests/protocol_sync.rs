//! Packet-level wiring tests for sound and minecart movement sync.
//!
//! Every assertion drives the real server path against a player whose
//! connection records encoded packets, so these tests prove the packets are
//! actually produced and not just implemented.

use std::io::Cursor;
use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packet_traits::EncodedPacket;
use steel_registry::item_stack::ItemStack;
use steel_registry::packets::play::{
    C_ENTITY_POSITION_SYNC, C_MOVE_ENTITY_POS, C_MOVE_ENTITY_POS_ROT, C_MOVE_ENTITY_ROT,
    C_MOVE_MINECART_ALONG_TRACK, C_SOUND_ENTITY,
};
use steel_registry::sound_event::SoundEventHolder;
use steel_registry::{REGISTRY, vanilla_blocks, vanilla_entities, vanilla_items};
use steel_utils::codec::VarInt;
use steel_utils::locks::SyncMutex;
use steel_utils::serial::ReadFrom as _;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos};

use crate::bootstrap::init_globals;
use crate::entity::entities::{MinecartEntity, SheepEntity};
use crate::entity::{Entity as _, SharedEntity};
use crate::player::{Player, PlayerConnection, ResetReason};
use crate::test_support::{
    TestConnection, TestPlayerBuilder, fresh_test_world, insert_entity_ticking_chunks,
    settle_entity_ticking_chunk, tick_test_world,
};
use crate::world::World;

/// Chunk radius loaded around the entity under test.
const TICKING_RADIUS: i32 = 2;
/// Entity id used for the observer player.
const PLAYER_ID: i32 = 1;
/// Entity id used for the entity under test.
const SUBJECT_ID: i32 = 2;

struct Fixture {
    world: Arc<World>,
    player: Arc<Player>,
    packets: Arc<SyncMutex<Vec<EncodedPacket>>>,
}

fn setup(key: &'static str) -> Fixture {
    init_globals();
    let fixture = fresh_test_world(key);
    let world = Arc::clone(&fixture.world);
    let center = ChunkPos::new(0, 0);
    insert_entity_ticking_chunks(&world, center, TICKING_RADIUS);

    let connection = TestConnection::default();
    let packets = connection.sent_handle();
    let player = TestPlayerBuilder::new(Arc::clone(&world), "Observer", PLAYER_ID)
        .connection(Arc::new(PlayerConnection::Other(Box::new(connection))))
        .build();
    player.set_client_loaded(true);
    player
        .base()
        .set_position_local(DVec3::new(8.5, 64.0, 10.5));
    assert!(
        world.add_player(Arc::clone(&player), ResetReason::InitialJoin),
        "the observing player must join the world"
    );
    settle_entity_ticking_chunk(&world, center);
    // Vanilla only syncs entities whose chunk packet the client already holds.
    player
        .chunk_sender()
        .lock()
        .mark_chunk_sent_for_test(center);

    packets.lock().clear();
    Fixture {
        world,
        player,
        packets,
    }
}

impl Fixture {
    /// Re-runs the tracker's visibility pass so the subject is being tracked.
    fn refresh_tracking(&self, subject: &SharedEntity) {
        let view = self
            .player
            .last_tracking_view
            .lock()
            .expect("the joined player has a tracking view");
        let sent = self.player.chunk_sender().lock().sent_chunks_snapshot();
        self.world
            .entity_tracker()
            .update_player(&self.player, &view, |chunk| sent.contains(&chunk));
        assert!(
            self.world
                .entity_tracker()
                .tracking_player_ids(subject.id())
                .contains(&PLAYER_ID),
            "the observer must track the subject entity"
        );
    }
}

/// Decodes every `C_SOUND_ENTITY` packet into `(entity id, sound key)`.
fn entity_sounds(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<(i32, String)> {
    packets
        .lock()
        .iter()
        .filter_map(|packet| {
            let mut cursor = Cursor::new(packet.encoded_data.as_slice());
            VarInt::read(&mut cursor).ok()?;
            if VarInt::read(&mut cursor).ok()?.0 != C_SOUND_ENTITY {
                return None;
            }
            let sound = SoundEventHolder::read(&mut cursor).ok()?;
            let key = sound.registry_ref()?.key.to_string();
            VarInt::read(&mut cursor).ok()?; // sound source category
            let entity_id = VarInt::read(&mut cursor).ok()?.0;
            f32::read(&mut cursor).ok()?; // volume
            f32::read(&mut cursor).ok()?; // pitch
            i64::read(&mut cursor).ok()?; // seed
            Some((entity_id, key))
        })
        .collect()
}

/// Decodes the entity ids of every `C_MOVE_MINECART_ALONG_TRACK` packet.
fn minecart_packet_entity_ids(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<i32> {
    packets
        .lock()
        .iter()
        .filter_map(|packet| {
            let mut cursor = Cursor::new(packet.encoded_data.as_slice());
            VarInt::read(&mut cursor).ok()?;
            if VarInt::read(&mut cursor).ok()?.0 != C_MOVE_MINECART_ALONG_TRACK {
                return None;
            }
            Some(VarInt::read(&mut cursor).ok()?.0)
        })
        .collect()
}

#[test]
fn shearing_a_sheep_sends_a_sound_entity_packet() {
    let fixture = setup("protocol_sync_sheep_shear");
    let sheep = Arc::new(SheepEntity::new(
        &vanilla_entities::SHEEP,
        SUBJECT_ID,
        DVec3::new(8.5, 64.0, 8.5),
        Arc::downgrade(&fixture.world),
    ));
    let shared: SharedEntity = Arc::clone(&sheep) as SharedEntity;
    fixture
        .world
        .try_add_entity(shared)
        .expect("the sheep joins the entity-ticking fixture chunk");
    fixture.packets.lock().clear();

    sheep.shear(&fixture.world, &ItemStack::new(&vanilla_items::SHEARS));

    // Vanilla `Sheep.shear` → `level.playSound(null, this, SHEEP_SHEAR, …)`.
    let sounds = entity_sounds(&fixture.packets);
    assert!(
        sounds
            .iter()
            .any(|(entity_id, key)| *entity_id == SUBJECT_ID
                && key == "minecraft:entity.sheep.shear"),
        "shearing must send C_SOUND_ENTITY for the sheep, got {sounds:?}"
    );
}

/// Decodes every ordinary entity movement packet into its entity id.
fn plain_movement_entity_ids(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<i32> {
    const MOVEMENT_IDS: [i32; 4] = [
        C_MOVE_ENTITY_POS,
        C_MOVE_ENTITY_POS_ROT,
        C_MOVE_ENTITY_ROT,
        C_ENTITY_POSITION_SYNC,
    ];

    packets
        .lock()
        .iter()
        .filter_map(|packet| {
            let mut cursor = Cursor::new(packet.encoded_data.as_slice());
            VarInt::read(&mut cursor).ok()?;
            if !MOVEMENT_IDS.contains(&VarInt::read(&mut cursor).ok()?.0) {
                return None;
            }
            Some(VarInt::read(&mut cursor).ok()?.0)
        })
        .collect()
}

#[test]
fn a_minecart_on_a_rail_sends_plain_movement_not_track_steps() {
    let fixture = setup("protocol_sync_minecart_plain_movement");
    let rail = REGISTRY.blocks.get_default_state_id(&vanilla_blocks::RAIL);
    fixture
        .world
        .set_block(BlockPos::new(8, 64, 8), rail, UpdateFlags::UPDATE_ALL);

    let cart: SharedEntity = Arc::new(MinecartEntity::new(
        &vanilla_entities::MINECART,
        SUBJECT_ID,
        DVec3::new(8.5, 64.0625, 8.5),
        Arc::downgrade(&fixture.world),
    ));
    cart.set_velocity(DVec3::new(0.0, 0.0, 0.3));
    fixture
        .world
        .try_add_entity(Arc::clone(&cart))
        .expect("the minecart joins the entity-ticking fixture chunk");
    let start = cart.position();
    fixture.refresh_tracking(&cart);
    fixture.packets.lock().clear();

    tick_test_world(&fixture.world, 3, true);

    assert!(
        cart.position() != start,
        "the cart must actually travel along the rail"
    );
    // Vanilla `ServerEntity.sendChanges` only takes the `handleMinecartPosRot`
    // branch for `NewMinecartBehavior` carts (`minecart_improvements` flag).
    // Steel runs `OldMinecartBehavior`, so the cart must sync with the ordinary
    // movement packets and never with `ClientboundMoveMinecartPacket` — a vanilla
    // client ignores that packet for old-behaviour carts.
    assert!(
        minecart_packet_entity_ids(&fixture.packets).is_empty(),
        "an old-behaviour minecart must not send C_MOVE_MINECART_ALONG_TRACK"
    );
    let movements = plain_movement_entity_ids(&fixture.packets);
    assert!(
        movements.contains(&SUBJECT_ID),
        "the cart must be synced with an ordinary movement packet, got {movements:?}"
    );
}
