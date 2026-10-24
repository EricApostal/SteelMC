//! `/reload` client resynchronization.
//!
//! Vanilla `ReloadCommand.reloadPacks` runs `MinecraftServer.reloadResources`,
//! whose tail end (`PlayerList.reloadResources`) pushes the registry tags and
//! the synchronized recipes to every online player. Steel's [`crate::datapack`]
//! layer owns the same resend, plus a command-tree re-projection so the client's
//! graph matches the freshly published function registry. These tests run the
//! real command against a test server and assert the packets land.

use std::sync::Arc;

use steel_protocol::packet_traits::EncodedPacket;
use steel_registry::init_vanilla_registry;
use steel_registry::packets::play::{C_COMMANDS, C_UPDATE_RECIPES, C_UPDATE_TAGS};
use steel_utils::locks::SyncMutex;

use super::{
    UNROUTABLE_SERVICES, offline_server, packet_id, run_command, shutdown_server,
    test_player_with_packets, test_storage_root, with_server_runtime,
};
use crate::behavior::init_behaviors;
use crate::command::execution::CommandSource;
use crate::command::sender::CommandSender;
use crate::entity::{init_entities, next_entity_id};

#[test]
fn reload_resends_client_datapack_state() {
    with_server_runtime(|runtime| {
        runtime.block_on(async {
            init_vanilla_registry();
            init_entities();
            init_behaviors();

            let save_root = test_storage_root("datapack-reload-resend");
            let server = Arc::new(
                offline_server(runtime, &save_root, UNROUTABLE_SERVICES)
                    .await
                    .expect("reload test server should initialize"),
            );

            let (player, sent_packets) = test_player_with_packets(
                &server,
                Arc::clone(server.overworld()),
                "Resender",
                next_entity_id(),
            );
            assert!(server.online_players.insert(Arc::clone(&player)));

            let source = CommandSource::new(CommandSender::Console, Arc::clone(&server));
            run_command(&server, source, "reload");

            assert_command_tree_resent(&sent_packets);
            assert_datapack_payloads_resent(&sent_packets);

            shutdown_server(&server, &save_root).await;
        });
    });
}

/// The command tree arrives on the connection the reload just resynchronized.
fn assert_command_tree_resent(sent_packets: &Arc<SyncMutex<Vec<EncodedPacket>>>) {
    assert!(
        sent_packets
            .lock()
            .iter()
            .any(|packet| packet_id(packet) == C_COMMANDS),
        "`/reload` should re-send the command tree to every online player"
    );
}

/// Vanilla parity: registry tags then recipes, matching `PlayerList.reloadResources`.
fn assert_datapack_payloads_resent(sent_packets: &Arc<SyncMutex<Vec<EncodedPacket>>>) {
    let ids: Vec<i32> = sent_packets.lock().iter().map(packet_id).collect();
    assert!(
        ids.contains(&C_UPDATE_TAGS),
        "`/reload` should re-send the registry tags to every online player"
    );
    assert!(
        ids.contains(&C_UPDATE_RECIPES),
        "`/reload` should re-send the synchronized recipes to every online player"
    );
}
