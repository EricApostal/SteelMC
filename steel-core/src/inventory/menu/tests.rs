use std::sync::Arc;

use super::{FillDirection, Menu, MenuBuilder, kinds::BasicKind};
use crate::{
    behavior::init_behaviors,
    block_entity::init_block_entities,
    chunk::{Chunk, status::ChunkStatus},
    entity::{Entity as _, entities::ItemEntity},
    inventory::{
        click::{Click, DragKind, MouseButton, QuickCraft, SwapTarget},
        container::{Container as _, SimpleContainer},
        lock::{ContainerLockGuard, ContainerRef},
        slots::{NormalSlot, Slot, SlotStorage},
    },
    player::Player,
    test_support::{TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk},
};
use glam::DVec3;
use steel_registry::{
    data_components::vanilla_components::MAX_STACK_SIZE, init_vanilla_registry,
    item_stack::ItemStack, vanilla_blocks, vanilla_entities, vanilla_items, vanilla_menu_types,
};
use steel_utils::locks::{IntoShared as _, Shared};
use steel_utils::types::{GameType, UpdateFlags};
use steel_utils::{ChunkPos, Downcast as _, DowncastType, DowncastTypeKey, WorldAabb};

struct SingleItemSlot {
    base: NormalSlot,
}

// SAFETY: This test-only key uniquely identifies `SingleItemSlot`.
unsafe impl DowncastType for SingleItemSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:test/slot/single_item");
}

impl Slot for SingleItemSlot {
    fn storage(&self) -> &SlotStorage {
        self.base.storage()
    }

    fn get_item<'a>(&self, guard: &'a ContainerLockGuard) -> &'a ItemStack {
        self.base.get_item(guard)
    }

    fn get_item_mut<'a>(&self, guard: &'a mut ContainerLockGuard) -> &'a mut ItemStack {
        self.base.get_item_mut(guard)
    }

    fn set_item(&self, guard: &mut ContainerLockGuard, stack: ItemStack) {
        self.base.set_item(guard, stack);
    }

    fn get_max_stack_size(&self, _guard: &ContainerLockGuard) -> i32 {
        1
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        self.base.set_changed(guard);
    }

    fn get_container_slot(&self) -> usize {
        self.base.get_container_slot()
    }
}

struct PartialSwapFixture {
    world_fixture: TestWorld,
    player: Arc<Player>,
    target: Shared<SimpleContainer>,
}

fn perform_partial_swap(world_name: &'static str, game_mode: GameType) -> PartialSwapFixture {
    init_vanilla_registry();
    let world_fixture = fresh_test_world(world_name);
    let world = &world_fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let player = TestPlayerBuilder::new(Arc::clone(world), "SwapTester", 1).build();
    player.restore_game_modes(game_mode, None);
    player.abilities.lock().update_for_game_mode(game_mode);
    player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
    {
        let mut inventory = player.inventory.lock();
        for slot in 0..inventory.get_container_size() {
            inventory.set_item(slot, ItemStack::with_count(&vanilla_items::DIRT, 64));
        }
        inventory.set_item(0, ItemStack::with_count(&vanilla_items::DIRT, 2));
    }

    let target = SimpleContainer::new(1).into_shared();
    target
        .lock()
        .set_item(0, ItemStack::new(&vanilla_items::STONE));
    let target_ref = ContainerRef::from(Arc::clone(&target));
    let mut builder = MenuBuilder::new(None, 1);
    let target_slots = builder.custom_section([SingleItemSlot {
        base: NormalSlot::new(target_ref, 0),
    }]);
    let mut menu = builder.build(BasicKind {});
    menu.clicked(
        Click::Swap {
            slot: target_slots.start(),
            with: SwapTarget::Hotbar(0),
        },
        &player,
    );

    PartialSwapFixture {
        world_fixture,
        player,
        target,
    }
}

#[test]
fn swap_locks_player_inventory_when_menu_has_no_inventory_slots() {
    init_vanilla_registry();
    let world_fixture = fresh_test_world("menu_swap_without_inventory_slots");
    let world = &world_fixture.world;
    let player = TestPlayerBuilder::new(Arc::clone(world), "SwapTester", 1).build();
    let container = SimpleContainer::new(45).into_shared();
    container
        .lock()
        .set_item(0, ItemStack::new(&vanilla_items::STONE));

    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X1, 1);
    let menu_slots = builder.section(Arc::clone(&container), 45);
    let mut menu = builder.build(BasicKind {});

    menu.clicked(
        Click::Swap {
            slot: menu_slots.start(),
            with: SwapTarget::Hotbar(0),
        },
        &player,
    );

    assert!(container.lock().get_item(0).is_empty());
    assert!(
        player
            .inventory
            .lock()
            .get_item(0)
            .is(&vanilla_items::STONE)
    );
}

#[test]
fn draining_a_block_entity_slot_marks_its_chunk_dirty() {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    let world_fixture = fresh_test_world("persistent_menu_drain");
    let world = &world_fixture.world;
    let pos = steel_utils::BlockPos::new(0, 64, 0);
    let holder = insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
    assert!(world.set_block(
        pos,
        vanilla_blocks::BARREL.default_state(),
        UpdateFlags::UPDATE_NONE,
    ));
    let container = ContainerRef::from_block_entity(
        world
            .get_block_entity(pos)
            .expect("barrel should create its block entity"),
    )
    .expect("barrel should expose a container");
    {
        let mut guard = ContainerLockGuard::lock_all(&[&container]);
        assert!(guard.set_item(
            container.container_id(),
            0,
            ItemStack::new(&vanilla_items::STONE),
        ));
    }
    holder
        .try_chunk(ChunkStatus::Full)
        .expect("full chunk should remain loaded")
        .clear_dirty();

    let player = TestPlayerBuilder::new(Arc::clone(world), "DrainTester", 1).build();
    player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
    let mut builder = MenuBuilder::new(None, 1);
    let drained = builder.section(container, 1);
    builder.drain(drained);
    let mut menu = builder.build(BasicKind {});

    menu.removed(&player);

    assert!(
        holder
            .try_chunk(ChunkStatus::Full)
            .is_some_and(Chunk::is_dirty)
    );
}

#[test]
fn one_slot_creative_clone_drag_is_a_vanilla_noop() {
    init_vanilla_registry();
    let world_fixture = fresh_test_world("one_slot_clone_drag");
    let world = &world_fixture.world;
    let player = TestPlayerBuilder::new(Arc::clone(world), "CloneTester", 1).build();
    player.restore_game_modes(GameType::Creative, None);
    let container = SimpleContainer::new(1).into_shared();
    container
        .lock()
        .set_item(0, ItemStack::new(&vanilla_items::STONE));
    let mut builder = MenuBuilder::new(None, 1);
    let slots = builder.section(Arc::clone(&container), 1);
    let mut menu = builder.build(BasicKind {});
    *menu.behavior_mut().carried_mut() = ItemStack::new(&vanilla_items::STONE);

    menu.clicked(
        Click::QuickCraft(QuickCraft::Start {
            kind: DragKind::Clone,
        }),
        &player,
    );
    menu.clicked(
        Click::QuickCraft(QuickCraft::AddSlot {
            slot: slots.start(),
        }),
        &player,
    );
    menu.clicked(Click::QuickCraft(QuickCraft::End), &player);

    assert_eq!(container.lock().get_item(0).count(), 1);
    assert_eq!(menu.behavior().carried().count(), 1);
    assert_eq!(menu.behavior().quickcraft(), None);
}

#[test]
fn partial_swap_overflow_marks_displaced_item_as_thrown() {
    let fixture = perform_partial_swap("menu_partial_swap_overflow", GameType::Survival);
    let player_id = fixture.player.gameprofile.id();

    assert_eq!(fixture.player.inventory.lock().get_item(0).count(), 1);
    let target_item = fixture.target.lock().get_item(0).clone();
    assert!(target_item.is(&vanilla_items::DIRT));
    assert_eq!(target_item.count(), 1);
    let dropped = fixture.world_fixture.world.get_entities_in_aabb_matching(
        &WorldAabb::new(-2.0, 62.0, -2.0, 2.0, 68.0, 2.0),
        |entity| entity.entity_type() == &vanilla_entities::ITEM,
    );
    assert_eq!(dropped.len(), 1);
    let Some(item) = dropped[0].as_ref().downcast_ref::<ItemEntity>() else {
        panic!("dropped entity should retain its concrete item type");
    };
    assert!(item.get_item().is(&vanilla_items::STONE));
    assert_eq!(item.get_thrower(), Some(player_id));
}

#[test]
fn partial_swap_overflow_is_discarded_in_creative() {
    let fixture = perform_partial_swap("creative_menu_partial_swap_overflow", GameType::Creative);

    assert_eq!(fixture.player.inventory.lock().get_item(0).count(), 1);
    let target_item = fixture.target.lock().get_item(0).clone();
    assert!(target_item.is(&vanilla_items::DIRT));
    assert_eq!(target_item.count(), 1);
    assert!(
        fixture
            .player
            .inventory
            .lock()
            .items()
            .iter()
            .all(|item| !item.is(&vanilla_items::STONE))
    );
    assert!(
        fixture
            .world_fixture
            .world
            .get_entities_in_aabb_matching(
                &WorldAabb::new(-2.0, 62.0, -2.0, 2.0, 68.0, 2.0),
                |entity| entity.entity_type() == &vanilla_entities::ITEM,
            )
            .is_empty()
    );
}

/// A menu over one plain container, with no player slots and no routes.
fn plain_menu(container: Shared<SimpleContainer>) -> Menu {
    let mut builder = MenuBuilder::new(None, 1);
    builder.section_all(container);
    builder.build(BasicKind {})
}

/// A detached player for click tests that never need a loaded world.
fn click_player(key: &'static str) -> (TestWorld, Arc<Player>) {
    init_vanilla_registry();
    let world_fixture = fresh_test_world(key);
    let player = TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "ClickTester", 1).build();
    (world_fixture, player)
}

#[test]
fn click_placement_uses_the_component_stack_size() {
    let (_world, player) = click_player("menu_component_stack_size");
    let container = SimpleContainer::new(1).into_shared();
    let mut menu = plain_menu(Arc::clone(&container));

    let mut stack = ItemStack::new(&vanilla_items::STONE);
    stack.set(MAX_STACK_SIZE, 16);
    stack.set_count(20);
    *menu.behavior_mut().carried_mut() = stack;

    menu.clicked(
        Click::Pickup {
            slot: 0,
            button: MouseButton::Left,
        },
        &player,
    );

    // `Slot.safeInsert` caps the transfer at the item's own stack size.
    assert_eq!(container.lock().get_item(0).count(), 16);
    assert_eq!(menu.behavior().carried().count(), 4);
}

#[test]
fn right_click_places_one_item_then_tops_the_slot_up() {
    let (_world, player) = click_player("menu_right_click_placement");
    let container = SimpleContainer::new(1).into_shared();
    let mut menu = plain_menu(Arc::clone(&container));
    *menu.behavior_mut().carried_mut() = ItemStack::with_count(&vanilla_items::STONE, 8);

    menu.clicked(
        Click::Pickup {
            slot: 0,
            button: MouseButton::Right,
        },
        &player,
    );
    assert_eq!(container.lock().get_item(0).count(), 1);
    assert_eq!(menu.behavior().carried().count(), 7);

    menu.clicked(
        Click::Pickup {
            slot: 0,
            button: MouseButton::Right,
        },
        &player,
    );
    assert_eq!(container.lock().get_item(0).count(), 2);
    assert_eq!(menu.behavior().carried().count(), 6);
}

#[test]
fn double_click_collects_matching_stacks_in_both_directions() {
    for (direction, key) in [
        (FillDirection::Forward, "menu_pickup_all_forward"),
        (FillDirection::Backward, "menu_pickup_all_backward"),
    ] {
        let (_world, player) = click_player(key);
        let container = SimpleContainer::new(3).into_shared();
        {
            let mut guard = container.lock();
            guard.set_item(0, ItemStack::with_count(&vanilla_items::STONE, 3));
            guard.set_item(1, ItemStack::with_count(&vanilla_items::STONE, 2));
        }
        let mut menu = plain_menu(Arc::clone(&container));
        *menu.behavior_mut().carried_mut() = ItemStack::with_count(&vanilla_items::STONE, 1);

        // The double-clicked slot itself is empty, so collection runs over the
        // whole menu in the requested direction.
        menu.clicked(Click::PickupAll { slot: 2, direction }, &player);

        assert_eq!(menu.behavior().carried().count(), 6);
        let guard = container.lock();
        assert!(guard.get_item(0).is_empty());
        assert!(guard.get_item(1).is_empty());
    }
}

#[test]
fn drag_distributes_the_carried_stack_by_kind() {
    for (kind, per_slot, leftover) in [(DragKind::Left, 3, 0), (DragKind::Right, 1, 6)] {
        let (_world, player) = click_player(match kind {
            DragKind::Left => "menu_drag_left",
            DragKind::Right => "menu_drag_right",
            DragKind::Clone => unreachable!("only survival drags are exercised"),
        });
        let container = SimpleContainer::new(3).into_shared();
        let mut menu = plain_menu(Arc::clone(&container));
        *menu.behavior_mut().carried_mut() = ItemStack::with_count(&vanilla_items::STONE, 9);

        menu.clicked(Click::QuickCraft(QuickCraft::Start { kind }), &player);
        for slot in 0..3 {
            menu.clicked(Click::QuickCraft(QuickCraft::AddSlot { slot }), &player);
        }
        menu.clicked(Click::QuickCraft(QuickCraft::End), &player);

        let guard = container.lock();
        for slot in 0..3 {
            assert_eq!(guard.get_item(slot).count(), per_slot);
        }
        assert_eq!(menu.behavior().carried().count(), leftover);
        assert_eq!(menu.behavior().quickcraft(), None);
    }
}

#[test]
fn a_non_drag_click_during_a_drag_only_cancels_the_drag() {
    let (_world, player) = click_player("menu_drag_interrupt");
    let container = SimpleContainer::new(1).into_shared();
    let mut menu = plain_menu(Arc::clone(&container));
    *menu.behavior_mut().carried_mut() = ItemStack::with_count(&vanilla_items::STONE, 4);

    menu.clicked(
        Click::QuickCraft(QuickCraft::Start {
            kind: DragKind::Left,
        }),
        &player,
    );
    menu.clicked(Click::QuickCraft(QuickCraft::AddSlot { slot: 0 }), &player);
    assert_eq!(menu.behavior().quickcraft(), Some(DragKind::Left));

    menu.clicked(
        Click::Pickup {
            slot: 0,
            button: MouseButton::Left,
        },
        &player,
    );

    // Vanilla resets the drag and never applies the interrupting click.
    assert_eq!(menu.behavior().quickcraft(), None);
    assert!(container.lock().get_item(0).is_empty());
    assert_eq!(menu.behavior().carried().count(), 4);
}
