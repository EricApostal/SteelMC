//! The signed command path: a vanilla client of a server with a chat session signs its
//! commands and sends `ServerboundChatCommandSignedPacket`, so the signed packet is the only
//! command packet Steel sees for those players. It must reach the dispatcher.

use std::sync::Arc;

use steel_protocol::utils::RawPacket;
use steel_registry::{init_vanilla_registry, packets::play, vanilla_game_rules};
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedWrite as _, WriteTo as _};
use tokio::runtime::Builder;
use uuid::Uuid;

use crate::permission::{OP_GROUP, PermissionSet, PermissionSubjectIndex, PermissionSubjectState};
use crate::player::Player;
use crate::player::ResetReason;
use crate::player::connection::JavaConnection;
use crate::server::Server;

use super::{fresh_test_world, test_player_with_uuid_and_packets, test_server};

/// Encodes `ServerboundChatCommandSignedPacket` the way a vanilla client does: the command,
/// the timestamp, the salt, one per-argument signature, and the last-seen update.
///
/// The signature bytes are arbitrary: Steel checks no argument signature (see
/// `Player::handle_signed_chat_command`), and a client without a chat session is accepted
/// unsigned by vanilla too.
fn signed_command_payload(command: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    command
        .write_prefixed_bound::<VarInt>(&mut payload, 256)
        .expect("command should encode");
    0_i64.write(&mut payload).expect("timestamp should encode");
    0_i64.write(&mut payload).expect("salt should encode");
    VarInt(1)
        .write(&mut payload)
        .expect("argument count should encode");
    "value"
        .write_prefixed_bound::<VarInt>(&mut payload, 16)
        .expect("argument name should encode");
    payload.extend_from_slice(&[0u8; 256]);
    // `LastSeenMessages$Update`: offset, the 20-bit acknowledged set (three bytes), checksum.
    VarInt(0).write(&mut payload).expect("offset should encode");
    payload.extend_from_slice(&[0u8; 3]);
    0_u8.write(&mut payload).expect("checksum should encode");
    payload
}

/// Encodes `ServerboundChatCommandPacket`: just the VarInt-prefixed command.
fn unsigned_command_payload(command: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    command
        .write_prefixed_bound::<VarInt>(&mut payload, 256)
        .expect("command should encode");
    payload
}

/// Feeds one raw command packet through the connection's play-packet handling and then runs
/// the command phase the game tick drains.
fn run_command_packet(id: i32, payload: Vec<u8>, server: &Arc<Server>, player: &Arc<Player>) {
    JavaConnection::handle_play_packet_for_test(RawPacket::new(id, payload), player, server);
    server.run_queued_commands_for_test();
}

#[test]
fn signed_and_unsigned_chat_commands_both_reach_the_dispatcher() {
    init_vanilla_registry();
    let world_fixture = fresh_test_world("signed_chat_command");
    let world = Arc::clone(&world_fixture.world);
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should initialize");
    runtime.block_on(async {
        // `/gamerule` derives `minecraft.command.gamerule`, which the published operator
        // group grants; the command owner also revalidates live server/world membership.
        let uuid = Uuid::from_u128(0x5eed_c0de);
        let mut published = PermissionSubjectIndex::new();
        published.set(
            uuid,
            PermissionSubjectState::new(vec![OP_GROUP.to_owned()], PermissionSet::new()),
        );
        let server = test_server(Arc::clone(&world), published)
            .await
            .expect("test server should initialize");
        let (player, _) =
            test_player_with_uuid_and_packets(&server, Arc::clone(&world), uuid, "Signer", 1);
        assert!(server.online_players.insert(Arc::clone(&player)));
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));
        let _ = player.mark_joined_world();

        assert!(
            !world.get_game_rule(&vanilla_game_rules::KEEP_INVENTORY),
            "the rule the commands toggle starts disabled"
        );

        run_command_packet(
            play::S_CHAT_COMMAND_SIGNED,
            signed_command_payload("gamerule keep_inventory true"),
            &server,
            &player,
        );
        assert!(
            world.get_game_rule(&vanilla_game_rules::KEEP_INVENTORY),
            "the signed command must execute, not be dropped"
        );

        run_command_packet(
            play::S_CHAT_COMMAND,
            unsigned_command_payload("gamerule keep_inventory false"),
            &server,
            &player,
        );
        assert!(
            !world.get_game_rule(&vanilla_game_rules::KEEP_INVENTORY),
            "the unsigned command path must keep working"
        );
    });
}
