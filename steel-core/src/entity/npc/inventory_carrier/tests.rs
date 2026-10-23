//! Tests for [`InventoryCarrier`].

use std::io::Cursor;

use simdnbt::borrow::read_compound as read_borrowed_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::item_stack::ItemStack;
use steel_registry::{init_vanilla_registry, vanilla_items};

use super::InventoryCarrier;

fn emeralds(count: i32) -> ItemStack {
    ItemStack::with_count(&vanilla_items::EMERALD, count)
}

#[test]
fn add_item_merges_then_fills_empty_slots() {
    init_vanilla_registry();

    let mut carrier = InventoryCarrier::new(8);
    carrier.set_item(1, emeralds(60));

    // A partial merge into the matching slot, the rest into the first empty slot.
    let leftover = carrier.add_item(&emeralds(10));
    assert!(leftover.is_empty());
    assert_eq!(carrier.get_item(1).count(), 64);
    assert_eq!(carrier.get_item(0).count(), 6);
    assert!(carrier.get_item(2).is_empty());

    // A full inventory returns what did not fit.
    let mut full = InventoryCarrier::new(1);
    full.set_item(0, emeralds(64));
    assert!(!full.can_add_item(&emeralds(1)));
    let leftover = full.add_item(&emeralds(3));
    assert_eq!(leftover.count(), 3);
    assert_eq!(full.get_item(0).count(), 64);
}

#[test]
fn can_add_item_accepts_matching_stacks_with_space() {
    init_vanilla_registry();

    let mut carrier = InventoryCarrier::new(1);
    carrier.set_item(0, emeralds(10));

    assert!(carrier.can_add_item(&emeralds(1)));
    carrier.set_item(0, emeralds(64));
    assert!(!carrier.can_add_item(&emeralds(1)));
    // A different item cannot merge into the occupied slot.
    assert!(!carrier.can_add_item(&ItemStack::new(&vanilla_items::DIAMOND)));

    carrier.set_item(0, ItemStack::empty());
    assert!(carrier.can_add_item(&ItemStack::new(&vanilla_items::DIAMOND)));
}

#[test]
fn inventory_nbt_round_trip() {
    init_vanilla_registry();

    let mut carrier = InventoryCarrier::new(8);
    carrier.set_item(0, emeralds(5));
    carrier.set_item(3, ItemStack::with_count(&vanilla_items::BOOK, 2));
    carrier.set_item(7, ItemStack::new(&vanilla_items::DIAMOND));

    let mut nbt = NbtCompound::new();
    carrier.write_to_tag(&mut nbt);

    let list = nbt
        .list("Inventory")
        .expect("the inventory list should be written");
    assert_eq!(
        list.compounds().map(<[NbtCompound]>::len),
        Some(3),
        "only the three non-empty slots are written"
    );

    let mut restored = InventoryCarrier::new(8);
    with_borrowed_nbt(&nbt, |view| restored.read_from_tag(view));

    // The list holds stacks without slots, so they are re-added in order.
    assert_eq!(restored.get_item(0).count(), 5);
    assert_eq!(restored.get_item(0).item().key, vanilla_items::EMERALD.key);
    assert_eq!(restored.get_item(1).count(), 2);
    assert_eq!(restored.get_item(1).item().key, vanilla_items::BOOK.key);
    assert_eq!(restored.get_item(2).count(), 1);
    assert_eq!(restored.get_item(2).item().key, vanilla_items::DIAMOND.key);
    assert!(restored.get_item(3).is_empty());

    // Loading replaces the previous contents.
    let mut reused = InventoryCarrier::new(8);
    reused.set_item(5, ItemStack::new(&vanilla_items::STICK));
    with_borrowed_nbt(&nbt, |view| reused.read_from_tag(view));
    assert!(reused.get_item(5).is_empty());
    assert_eq!(reused.get_item(0).count(), 5);
}

/// Borrowed view of an owned compound, matching how entities load saved NBT.
fn with_borrowed_nbt(nbt: &NbtCompound, read: impl FnOnce(&simdnbt::borrow::NbtCompound<'_, '_>)) {
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let base = read_borrowed_compound(&mut Cursor::new(bytes.as_slice()))
        .expect("test nbt should reborrow");
    read(&(&base).into());
}
