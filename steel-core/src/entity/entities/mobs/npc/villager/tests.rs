use std::io::Cursor;
use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::read_compound as read_borrowed_compound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::{
    init_vanilla_registry, vanilla_entities, vanilla_villager_professions, vanilla_villager_types,
};

use super::*;

/// Vanilla `Villager.addAdditionalSaveData` writes these keys with these codecs.
#[test]
fn villager_saves_vanilla_data() {
    init_vanilla_registry();

    let villager = VillagerEntity::new(&vanilla_entities::VILLAGER, 1, DVec3::ZERO, Weak::new());
    villager.set_villager_data(
        villager
            .villager_data()
            .with_type_ref(&vanilla_villager_types::DESERT)
            .with_profession_ref(&vanilla_villager_professions::FARMER),
    );
    villager.set_villager_data_finalized(true);
    *villager.food_level.lock() = 9;
    villager.gossips.lock().add(
        Uuid::from_u128(7),
        GossipType::MajorPositive,
        GossipType::REPUTATION_CHANGE_PER_EVENT,
    );
    villager.set_villager_xp(70);
    *villager.last_restock_game_time.lock() = 1_200;
    *villager.last_gossip_decay_time.lock() = 24_000;
    *villager.number_of_restocks_today.lock() = 2;

    let mut nbt = NbtCompound::new();
    villager.save_additional(&mut nbt);

    let saved_data = nbt
        .compound("VillagerData")
        .expect("VillagerData compound should be saved");
    assert_eq!(
        saved_data.string("type").map(ToString::to_string),
        Some("minecraft:desert".to_owned())
    );
    assert_eq!(
        saved_data.string("profession").map(ToString::to_string),
        Some("minecraft:farmer".to_owned())
    );
    assert_eq!(saved_data.int("level"), Some(1));
    assert_eq!(nbt.byte("VillagerDataFinalized"), Some(1));
    assert_eq!(nbt.byte("FoodLevel"), Some(9));
    assert_eq!(nbt.int("Xp"), Some(70));
    assert_eq!(nbt.long("LastRestock"), Some(1_200));
    assert_eq!(nbt.long("LastGossipDecay"), Some(24_000));
    assert_eq!(nbt.int("RestocksToday"), Some(2));
    assert!(
        nbt.list("Gossips").is_some(),
        "gossips are saved under the vanilla Gossips key"
    );
    assert!(
        nbt.list("Inventory").is_some(),
        "the inventory is saved under the vanilla Inventory key"
    );
}

/// Reading the vanilla keys restores data, food, gossip, xp and restock state.
#[test]
fn villager_loads_vanilla_data() {
    init_vanilla_registry();

    let mut data = NbtCompound::new();
    data.insert("type", "minecraft:snow");
    data.insert("profession", "minecraft:librarian");
    data.insert("level", 3);

    let mut gossip = NbtCompound::new();
    gossip.insert("Target", NbtTag::IntArray(vec![0, 0, 0, 7]));
    gossip.insert("Type", "major_negative");
    gossip.insert("Value", 20);

    let mut nbt = NbtCompound::new();
    nbt.insert("VillagerData", NbtTag::Compound(data));
    nbt.insert("VillagerDataFinalized", 1_i8);
    nbt.insert("FoodLevel", 5_i8);
    nbt.insert("Xp", 150_i32);
    nbt.insert("LastRestock", 4_800_i64);
    nbt.insert("LastGossipDecay", 96_000_i64);
    nbt.insert("RestocksToday", 1_i32);
    nbt.insert("Gossips", NbtList::Compound(vec![gossip]));

    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed = read_borrowed_compound(&mut Cursor::new(&bytes))
        .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));

    let villager = VillagerEntity::new(&vanilla_entities::VILLAGER, 1, DVec3::ZERO, Weak::new());
    villager.load_additional((&borrowed).into());

    assert_eq!(
        villager.villager_type().key,
        vanilla_villager_types::SNOW.key
    );
    assert_eq!(
        villager.profession().key,
        vanilla_villager_professions::LIBRARIAN.key
    );
    assert_eq!(villager.villager_level(), 3);
    assert!(villager.villager_data_finalized());
    assert_eq!(villager.food_level(), 5);
    assert_eq!(villager.villager_xp(), 150);
    assert_eq!(villager.player_reputation(Uuid::from_u128(7)), -100);
    assert_eq!(*villager.last_restock_game_time.lock(), 4_800);
    assert_eq!(*villager.number_of_restocks_today.lock(), 1);
}
