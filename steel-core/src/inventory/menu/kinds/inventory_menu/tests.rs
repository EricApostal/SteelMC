use std::sync::Arc;

use super::{InventoryKind, inventory_menu};
use crate::{
    behavior::init_behaviors,
    entity::{Entity as _, entities::ItemEntity},
    inventory::{
        click::{Click, MouseButton},
        container::Container as _,
    },
    player::player_inventory::PlayerInventory,
    test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk},
};
use glam::DVec3;
use steel_registry::{
    init_vanilla_registry, item_stack::ItemStack, vanilla_entities, vanilla_items,
};
use steel_utils::{ChunkPos, Downcast as _, WorldAabb};

#[test]
fn partial_result_overflow_has_no_thrower() {
    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world("inventory_menu_partial_result_overflow");
    let world = &world_fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let player = TestPlayerBuilder::new(Arc::clone(world), "Crafter", 1).build();
    player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
    let mut menu = inventory_menu(Arc::clone(&player.inventory));
    let Some(InventoryKind { handler, .. }) = menu.kind().downcast_ref::<InventoryKind>() else {
        panic!("inventory_menu should create an inventory menu");
    };
    let (crafting_container, result_id) = (handler.crafting_container(), handler.result_id());

    *menu.behavior_mut().carried_mut() = ItemStack::new(&vanilla_items::OAK_LOG);
    menu.clicked(
        Click::Pickup {
            slot: 1,
            button: MouseButton::Left,
        },
        &player,
    );
    let result = menu
        .behavior()
        .lock_all_containers()
        .get(result_id)
        .expect("result container is registered with the menu")
        .get_item(0)
        .clone();
    assert!(result.is(&vanilla_items::OAK_PLANKS));
    assert_eq!(result.count(), 4);

    let mut matching = result;
    matching.set_count(63);
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..36 {
            inventory.set_item(slot, ItemStack::with_count(&vanilla_items::DIRT, 64));
        }
        inventory.set_item(8, matching);
    }

    menu.clicked(Click::QuickMove { slot: 0 }, &player);

    assert!(crafting_container.lock().get_item(0).is_empty());
    assert!(
        menu.behavior()
            .lock_all_containers()
            .get(result_id)
            .expect("result container is registered with the menu")
            .get_item(0)
            .is_empty()
    );
    assert_eq!(player.inventory.lock().get_item(8).count(), 64);
    let dropped = world.get_entities_in_aabb_matching(
        &WorldAabb::new(-2.0, 62.0, -2.0, 2.0, 68.0, 2.0),
        |entity| entity.entity_type() == &vanilla_entities::ITEM,
    );
    assert_eq!(dropped.len(), 1);
    let Some(item) = dropped[0].as_ref().downcast_ref::<ItemEntity>() else {
        panic!("dropped entity should retain its concrete item type");
    };
    assert!(item.get_item().is(&vanilla_items::OAK_PLANKS));
    assert_eq!(item.get_item().count(), 3);
    assert_eq!(item.get_thrower(), None);
}

#[test]
fn shift_click_equips_armor_then_falls_back_to_the_hotbar() {
    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world("inventory_menu_armor_equip");
    let world = &world_fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let player = TestPlayerBuilder::new(Arc::clone(world), "EquipTester", 1).build();
    player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
    let mut menu = inventory_menu(Arc::clone(&player.inventory));

    // Menu slot 9 is main-inventory index 9; the head slot is menu slot 5.
    player
        .inventory
        .lock()
        .set_item(9, ItemStack::new(&vanilla_items::DIAMOND_HELMET));
    menu.clicked(Click::QuickMove { slot: 9 }, &player);
    assert!(
        player
            .inventory
            .lock()
            .get_item(PlayerInventory::ARMOR_TOP_DOWN[0])
            .is(&vanilla_items::DIAMOND_HELMET)
    );

    // With the head slot occupied, vanilla falls back to the hotbar.
    player
        .inventory
        .lock()
        .set_item(10, ItemStack::new(&vanilla_items::DIAMOND_HELMET));
    menu.clicked(Click::QuickMove { slot: 10 }, &player);
    assert!(
        player
            .inventory
            .lock()
            .get_item(PlayerInventory::HOTBAR.start)
            .is(&vanilla_items::DIAMOND_HELMET)
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end armor click with recording connection and listener setup"
)]
fn armor_slot_click_plays_the_equip_sound_and_emits_the_equip_event() {
    use std::io::Cursor;
    use std::sync::Arc;

    use steel_protocol::packet_traits::{CompressionInfo, EncodedPacket};
    use steel_registry::game_events::GameEventRef;
    use steel_registry::packets::play::C_SOUND;
    use steel_registry::{sound_events, vanilla_game_events};
    use steel_utils::BlockPos;
    use steel_utils::SectionPos;
    use steel_utils::codec::VarInt;
    use steel_utils::locks::SyncMutex;
    use steel_utils::serial::ReadFrom as _;
    use text_components::TextComponent;

    use crate::inventory::equipment::{EntityEquipment as _, EquipmentSlot};
    use crate::player::connection::NetworkConnection;
    use crate::player::{PlayerConnection, ResetReason};
    use crate::world::World;
    use crate::world::game_event::{GameEventContext, GameEventListener, SharedGameEventListener};

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

    struct RecordingGameEventListener {
        pos: DVec3,
        events: Arc<SyncMutex<Vec<GameEventRef>>>,
    }

    impl GameEventListener for RecordingGameEventListener {
        fn listener_pos(&self) -> Option<DVec3> {
            Some(self.pos)
        }

        fn listener_radius(&self) -> i32 {
            16
        }

        fn handle_game_event(
            &self,
            _world: &Arc<World>,
            event: GameEventRef,
            _context: &GameEventContext<'_>,
            _source_pos: DVec3,
        ) -> bool {
            self.events.lock().push(event);
            true
        }
    }

    fn sound_ids(packets: &SyncMutex<Vec<EncodedPacket>>) -> Vec<i32> {
        packets
            .lock()
            .iter()
            .filter_map(|packet| {
                let mut cursor = Cursor::new(packet.encoded_data.as_slice());
                VarInt::read(&mut cursor).ok()?;
                if VarInt::read(&mut cursor).ok()?.0 != C_SOUND {
                    return None;
                }
                Some(VarInt::read(&mut cursor).ok()?.0)
            })
            .collect()
    }

    init_vanilla_registry();
    init_behaviors();
    let world_fixture = fresh_test_world("inventory_menu_armor_equip");
    let world = &world_fixture.world;
    let pos = DVec3::new(0.5, 64.0, 0.5);
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));

    let packets = Arc::new(SyncMutex::new(Vec::new()));
    let connection = Arc::new(PlayerConnection::Other(Box::new(RecordingConnection {
        packets: Arc::clone(&packets),
    })));
    let player = TestPlayerBuilder::new(Arc::clone(world), "ArmorTester", 1)
        .connection(connection)
        .build();
    player.base().set_position_local(pos);
    player.set_first_tick(false);
    let _ = world.add_player(Arc::clone(&player), ResetReason::InitialJoin);
    packets.lock().clear();

    let events = Arc::new(SyncMutex::new(Vec::new()));
    let listener: SharedGameEventListener = Arc::new(RecordingGameEventListener {
        pos,
        events: Arc::clone(&events),
    });
    world.register_game_event_listener(
        SectionPos::from_block_pos(BlockPos::new(0, 64, 0)),
        listener,
    );

    let mut menu = inventory_menu(Arc::clone(&player.inventory));
    *menu.behavior_mut().carried_mut() = ItemStack::new(&vanilla_items::DIAMOND_HELMET);
    menu.clicked(
        Click::Pickup {
            slot: 5,
            button: MouseButton::Left,
        },
        &player,
    );

    assert!(
        player
            .inventory
            .lock()
            .get_ref(EquipmentSlot::Head)
            .is(&vanilla_items::DIAMOND_HELMET)
    );
    assert!(events.lock().contains(&&vanilla_game_events::EQUIP));
    assert_eq!(
        sound_ids(&packets),
        [sound_events::ITEM_ARMOR_EQUIP_DIAMOND.packet_holder_id()],
    );
}
