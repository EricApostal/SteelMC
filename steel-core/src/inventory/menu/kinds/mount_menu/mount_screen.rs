//! Regression tests for the mount inventory screen.
//!
//! Vanilla `ServerPlayer.openHorseInventory` sends
//! `ClientboundMountScreenOpenPacket` with the container id, the mount's
//! inventory columns and the mount's entity id, and the client builds the same
//! `HorseInventoryMenu` (saddle slot, body-armor slot, chest storage, player
//! inventory) the server installed. These tests pin that contract end to end:
//! the columns come from the mount's own `hasChest` state, the packet's
//! container id really addresses the mount menu, slot clicks move items out of
//! the mount inventory, and closing the screen unregisters the menu.

use std::{io::Cursor, sync::Arc};

use rustc_hash::FxHashMap;
use steel_protocol::packet_traits::{CompressionInfo, EncodedPacket};
use steel_protocol::packets::game::{ClickType, HashedStack, SContainerClick};
use steel_registry::{
    init_vanilla_registry,
    item_stack::ItemStack,
    packets::play::{C_CONTAINER_CLOSE, C_MOUNT_SCREEN_OPEN},
    vanilla_entities, vanilla_items,
};
use steel_utils::ChunkPos;
use steel_utils::codec::VarInt;
use steel_utils::locks::SyncMutex;
use steel_utils::serial::ReadFrom as _;
use steel_utils::types::InteractionHand;
use text_components::TextComponent;

use super::{SLOT_INVENTORY_START, get_inventory_size};
use crate::behavior::InteractionResult;
use crate::entity::entities::mobs::passive::equine::DonkeyEntity;
use crate::entity::{Entity as _, Mob, SharedEntity, next_entity_id};
use crate::player::connection::NetworkConnection;
use crate::player::{Player, PlayerConnection};
use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
use crate::world::World;
use steel_registry::entity_data::EntityPose;

/// Captures every packet the server sends toward the fake client.
#[derive(Default)]
struct RecordingConnection {
    packets: Arc<SyncMutex<Vec<EncodedPacket>>>,
}

impl NetworkConnection for RecordingConnection {
    fn compression(&self) -> Option<CompressionInfo> {
        None
    }

    fn send_encoded(&self, packet: EncodedPacket) {
        self.packets.lock().push(packet);
    }

    fn send_encoded_bundle(&self, packets: Vec<EncodedPacket>) {
        self.packets.lock().extend(packets);
    }

    fn disconnect_with_reason(&self, _reason: TextComponent) {}

    fn tick(&self) {}

    fn latency(&self) -> i32 {
        0
    }

    fn close(&self) {}

    fn closed(&self) -> bool {
        false
    }
}

struct RecordingPlayer {
    player: Arc<Player>,
    packets: Arc<SyncMutex<Vec<EncodedPacket>>>,
}

fn recording_player(world: Arc<World>, name: &str, entity_id: i32) -> RecordingPlayer {
    let packets = Arc::new(SyncMutex::new(Vec::new()));
    let player = TestPlayerBuilder::new(world, name, entity_id)
        .connection(Arc::new(PlayerConnection::Other(Box::new(
            RecordingConnection {
                packets: Arc::clone(&packets),
            },
        ))))
        .build();
    RecordingPlayer { player, packets }
}

/// Decodes every `CMountScreenOpen` the server sent: container id, column
/// count and mount entity id, in vanilla `ClientboundMountScreenOpenPacket`
/// wire order (`writeContainerId`, `writeVarInt`, `writeInt`).
fn mount_screen_packets(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<(i32, i32, i32)> {
    packets
        .lock()
        .iter()
        .filter_map(|packet| {
            let mut cursor = Cursor::new(packet.encoded_data.as_slice());
            let _length = VarInt::read(&mut cursor).ok()?;
            let packet_id = VarInt::read(&mut cursor).ok()?;
            if packet_id.0 != C_MOUNT_SCREEN_OPEN {
                return None;
            }
            let container_id = VarInt::read(&mut cursor).ok()?.0;
            let columns = VarInt::read(&mut cursor).ok()?.0;
            let entity_id = i32::read(&mut cursor).ok()?;
            Some((container_id, columns, entity_id))
        })
        .collect()
}

#[test]
fn mount_screen_open_uses_the_mount_menu_and_vanilla_columns() {
    init_vanilla_registry();
    let world_fixture = fresh_test_world("mount_menu_mount_screen");
    let world = Arc::clone(&world_fixture.world);
    insert_ready_full_chunk(&world, ChunkPos::new(0, 0));

    let recording = recording_player(Arc::clone(&world), "MountTester", 9001);
    let player = &recording.player;

    // A tamed adult donkey at the player's feet, chested through the vanilla
    // `AbstractChestedHorse.mobInteract` chest branch.
    let donkey = Arc::new(DonkeyEntity::new(
        &vanilla_entities::DONKEY,
        next_entity_id(),
        player.position(),
        Arc::downgrade(&world),
    ));
    donkey.set_tamed(true);
    world
        .try_add_entity(Arc::clone(&donkey) as SharedEntity)
        .expect("donkey should attach to the loaded chunk");
    player.inventory.lock().set_item_in_hand(
        InteractionHand::MainHand,
        ItemStack::new(&vanilla_items::CHEST),
    );
    assert!(
        matches!(
            Mob::mob_interact(&*donkey, player, InteractionHand::MainHand),
            InteractionResult::Success
        ),
        "a tame donkey should accept a chest"
    );
    assert!(donkey.has_chest());
    assert_eq!(donkey.inventory_columns(), 5);

    // Crouch (shift) + empty-hand interact: vanilla `AbstractHorse.mobInteract`
    // opens `HasCustomInventoryScreen` instead of riding. Steel reads the
    // shift state from the synced shift-key flag (`Player::is_secondary_use_active`
    // -> `is_crouching` -> `isShiftKeyDown`), which is what a sneaking client
    // actually sets; the pose alone does not make the player crouch.
    player
        .inventory
        .lock()
        .set_item_in_hand(InteractionHand::MainHand, ItemStack::empty());
    player.set_crouching(true);
    player.set_pose(EntityPose::Sneaking);
    assert!(
        matches!(
            Mob::mob_interact(&*donkey, player, InteractionHand::MainHand),
            InteractionResult::Success
        ),
        "a shift-click should open the mount screen"
    );

    let screens = mount_screen_packets(&recording.packets);
    assert_eq!(screens.len(), 1, "exactly one mount screen should open");
    let (container_id, columns, entity_id) = screens[0];
    assert_eq!(columns, donkey.inventory_columns());
    assert_eq!(entity_id, donkey.id());

    // Vanilla `HorseInventoryMenu` layout for a 5-column mount.
    assert_eq!(SLOT_INVENTORY_START + get_inventory_size(columns) + 36, 53);
    assert_eq!(donkey.carried_items().lock().size(), 15);

    // The packet's container id really is the installed mount menu: clicking
    // its first chest slot picks the stone out of the donkey's inventory.
    donkey
        .carried_items()
        .lock()
        .set_item(0, ItemStack::new(&vanilla_items::STONE));
    player.handle_container_click(SContainerClick {
        container_id,
        state_id: 0,
        slot_num: SLOT_INVENTORY_START as i16,
        button_num: 0,
        click_type: ClickType::Pickup,
        changed_slots: FxHashMap::default(),
        carried_item: HashedStack::Empty,
    });
    assert!(
        donkey.carried_items().lock().get_item(0).is_empty(),
        "the chest slot click should move the stone out of the mount inventory"
    );

    // Closing unregisters the menu: the server echoes the close, the donkey
    // keeps only what the menu showed, and clicks on the stale container id
    // can no longer move items.
    donkey
        .carried_items()
        .lock()
        .set_item(1, ItemStack::new(&vanilla_items::STONE));
    player.close_container();
    let closes: Vec<i32> = recording
        .packets
        .lock()
        .iter()
        .filter_map(|packet| {
            let mut cursor = Cursor::new(packet.encoded_data.as_slice());
            let _length = VarInt::read(&mut cursor).ok()?;
            let packet_id = VarInt::read(&mut cursor).ok()?.0;
            (packet_id == C_CONTAINER_CLOSE)
                .then(|| VarInt::read(&mut cursor).ok())
                .flatten()
                .map(|id| id.0)
        })
        .collect();
    assert_eq!(
        closes,
        [container_id],
        "the close should echo the screen id"
    );
    assert!(
        !player.has_container_open(),
        "the mount menu should be unregistered after close"
    );
    player.handle_container_click(SContainerClick {
        container_id,
        state_id: 0,
        slot_num: SLOT_INVENTORY_START as i16 + 1,
        button_num: 0,
        click_type: ClickType::Pickup,
        changed_slots: FxHashMap::default(),
        carried_item: HashedStack::Empty,
    });
    assert!(
        !donkey.carried_items().lock().get_item(1).is_empty(),
        "a click on the unregistered container id must not touch the mount inventory"
    );
}
