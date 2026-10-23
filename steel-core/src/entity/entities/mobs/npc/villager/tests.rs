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

/// A villager that holds a job site must produce the profession's level offers.
///
/// Regression: an employed villager whose offers fail to generate shakes its head
/// on interaction (`mob_interact` treats an empty offer list as "no trades"), which
/// is what an unemployed villager legitimately does.
#[test]
fn employed_villager_generates_level_offers() {
    use crate::test_support::fresh_test_world;

    let fixture = fresh_test_world("villager_offers");
    let world = &fixture.world;
    let villager: SharedEntity = std::sync::Arc::new(VillagerEntity::new(
        &vanilla_entities::VILLAGER,
        1,
        DVec3::new(0.5, 1.0, 0.5),
        std::sync::Arc::downgrade(world),
    ));
    let entity = villager
        .downcast_ref::<VillagerEntity>()
        .expect("villager downcast");

    let data = entity
        .villager_data()
        .with_type_ref(&vanilla_villager_types::PLAINS)
        .with_profession_ref(&vanilla_villager_professions::FARMER);
    entity.set_villager_data(data);
    entity.ensure_offers_initialized();

    let offers = entity.merchant.offers();
    assert!(
        !offers.is_empty(),
        "an employed farmer must generate its level-{} trade set, got {} offers",
        entity.villager_level(),
        offers.len()
    );
}

/// The full live path: a naturally spawned villager (unemployed, biome type) that then
/// takes a job site must end up with that profession's level offers.
#[test]
fn naturally_spawned_villager_gets_offers_after_taking_a_job() {
    use crate::entity::EntitySpawnReason;
    use crate::test_support::fresh_test_world;
    use steel_registry::{vanilla_poi_types, vanilla_villager_professions};

    let fixture = fresh_test_world("villager_takes_job");
    let world = &fixture.world;
    let villager: SharedEntity = std::sync::Arc::new(VillagerEntity::new(
        &vanilla_entities::VILLAGER,
        1,
        DVec3::new(0.5, 1.0, 0.5),
        std::sync::Arc::downgrade(world),
    ));
    let entity = villager
        .downcast_ref::<VillagerEntity>()
        .expect("villager downcast");

    entity.finalize_spawn(world, EntitySpawnReason::Natural, None);
    assert_eq!(
        entity.profession().key,
        vanilla_villager_professions::NONE.key,
        "a naturally spawned villager starts unemployed"
    );

    assert!(
        entity.apply_profession_from_poi(&vanilla_poi_types::FARMER),
        "the farmer job site must be assignable"
    );
    assert_eq!(
        entity.profession().key,
        vanilla_villager_professions::FARMER.key
    );

    entity.ensure_offers_initialized();
    let offers = entity.merchant.offers();
    assert!(
        !offers.is_empty(),
        "a villager that just took the farmer job must have offers (level {}, type {})",
        entity.villager_level(),
        entity.villager_type().key
    );
}

// --- restock / gossip / pricing invariants --------------------------------------------

/// A farmer villager whose level-1 offers are initialized.
fn test_villager_with_offers(world: &Arc<World>, id: i32) -> Arc<VillagerEntity> {
    let villager = Arc::new(VillagerEntity::new(
        &vanilla_entities::VILLAGER,
        id,
        DVec3::new(0.5, 1.0, 0.5),
        Arc::downgrade(world),
    ));
    villager.set_villager_data(
        villager
            .villager_data()
            .with_type_ref(&vanilla_villager_types::PLAINS)
            .with_profession_ref(&vanilla_villager_professions::FARMER),
    );
    villager.ensure_offers_initialized();
    assert!(
        !villager.merchant.offers().is_empty(),
        "the test villager needs offers to restock"
    );
    villager
}

/// Uses one unit of the first offer, which is what makes a villager `needsToRestock`.
fn use_one_offer(villager: &VillagerEntity) {
    villager
        .merchant
        .offers()
        .iter_mut()
        .next()
        .expect("the farmer has offers")
        .increase_uses();
}

/// Vanilla `Villager.allowedToRestock`: at most two restocks, the second only once
/// `2400` ticks have passed, and `shouldRestock` resets the counter on a new day.
#[test]
fn restock_allows_two_restocks_per_day() {
    use crate::test_support::{advance_test_game_time_to, fresh_test_world};

    init_vanilla_registry();
    let fixture = fresh_test_world("villager_restock");
    let world = &fixture.world;
    let villager = test_villager_with_offers(world, 1);

    // The first restock of the day is always allowed while an offer needs it.
    use_one_offer(&villager);
    assert!(villager.should_restock());
    villager.restock();
    assert_eq!(*villager.number_of_restocks_today.lock(), 1);
    assert_eq!(*villager.last_restock_game_time.lock(), 0);

    // The second restock is refused until 2400 ticks have passed.
    use_one_offer(&villager);
    assert!(
        !villager.should_restock(),
        "`gameTime > lastRestockGameTime + 2400` gates the second restock"
    );
    advance_test_game_time_to(world, 2_401);
    assert!(villager.should_restock());
    villager.restock();
    assert_eq!(*villager.number_of_restocks_today.lock(), 2);

    // A third restock stays refused no matter how much time passes that day.
    use_one_offer(&villager);
    advance_test_game_time_to(world, 4_802);
    assert!(
        !villager.should_restock(),
        "`numberOfRestocksToday < 2` caps the day at two restocks"
    );

    // Half a day after the last restock the counter resets and restocking resumes.
    advance_test_game_time_to(world, 2_401 + 12_001);
    assert!(villager.should_restock());
    assert_eq!(
        *villager.number_of_restocks_today.lock(),
        0,
        "a new day resets `numberOfRestocksToday`"
    );
}

/// Vanilla `Villager.gossip`: an exchange needs `GOSSIP_COOLDOWN` ticks on *both* sides
/// since the last one, and `transferFrom` decays the shared value.
#[test]
fn gossip_transfer_respects_the_cooldown() {
    use crate::test_support::fresh_test_world;

    init_vanilla_registry();
    let fixture = fresh_test_world("villager_gossip_cooldown");
    let world = &fixture.world;
    let first = test_villager_with_offers(world, 1);
    let second = test_villager_with_offers(world, 2);
    let traded = Uuid::from_u128(9);

    first
        .gossips
        .lock()
        .add(traded, GossipType::MinorNegative, 25);

    first.gossip(world, &second, 5_000);
    assert_eq!(
        second.player_reputation(traded),
        -5,
        "the transfer subtracts the type's per-transfer decay (25 - 20)"
    );
    assert_eq!(*first.last_gossip_time.lock(), 5_000);
    assert_eq!(*second.last_gossip_time.lock(), 5_000);

    // Inside the cooldown neither side gossips again. Only one entry is left in the
    // source so the transfer's random draw cannot miss it.
    let other = Uuid::from_u128(10);
    first.gossips.lock().clear();
    first
        .gossips
        .lock()
        .add(other, GossipType::MinorNegative, 25);
    first.gossip(world, &second, 5_000 + GOSSIP_COOLDOWN - 1);
    assert_eq!(
        second.player_reputation(other),
        0,
        "an exchange inside the cooldown is refused"
    );

    first.gossip(world, &second, 5_000 + GOSSIP_COOLDOWN);
    assert_eq!(second.player_reputation(other), -5);
    assert_eq!(*first.last_gossip_time.lock(), 5_000 + GOSSIP_COOLDOWN);
}

/// Vanilla `Villager.maybeDecayGossip`: gossip decays once every `GOSSIP_DECAY_INTERVAL`
/// ticks, and values that fall below the discard threshold are dropped.
#[test]
fn gossip_decays_every_24000_ticks() {
    use crate::test_support::{advance_test_game_time_to, fresh_test_world};

    init_vanilla_registry();
    let fixture = fresh_test_world("villager_gossip_decay");
    let world = &fixture.world;
    let villager = test_villager_with_offers(world, 1);
    let traded = Uuid::from_u128(11);

    // Vanilla's `lastGossipDecayTime` starts at 0, where `maybeDecayGossip` only arms the
    // timer; a non-zero value is needed to observe the interval from a fixed epoch.
    *villager.last_gossip_decay_time.lock() = 1;
    villager
        .gossips
        .lock()
        .add(traded, GossipType::MinorNegative, 25);
    advance_test_game_time_to(world, 1);

    villager.maybe_decay_gossip();
    assert_eq!(villager.player_reputation(traded), -25);

    advance_test_game_time_to(world, 1 + GOSSIP_DECAY_INTERVAL - 1);
    villager.maybe_decay_gossip();
    assert_eq!(
        villager.player_reputation(traded),
        -25,
        "the interval has not elapsed yet"
    );

    advance_test_game_time_to(world, 1 + GOSSIP_DECAY_INTERVAL);
    villager.maybe_decay_gossip();
    assert_eq!(
        villager.player_reputation(traded),
        -5,
        "one decay subtracts the type's per-day amount (25 - 20)"
    );

    advance_test_game_time_to(world, 1 + 2 * GOSSIP_DECAY_INTERVAL);
    villager.maybe_decay_gossip();
    assert_eq!(
        villager.player_reputation(traded),
        0,
        "values below the discard threshold are dropped"
    );
}

/// Vanilla `Villager.updateSpecialPrices`: the reputation discount is reset and re-applied
/// per offer, Hero of the Village stacks on top, and an open trade screen is refreshed.
#[test]
fn update_special_prices_applies_reputation_and_hero_discounts() {
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use steel_utils::ChunkPos;

    init_vanilla_registry();
    let fixture = fresh_test_world("villager_special_prices");
    let world = &fixture.world;
    insert_ready_full_chunk(world, ChunkPos::new(0, 0));
    let villager = test_villager_with_offers(world, 1);
    let shared: SharedEntity = villager.clone();
    world
        .try_add_entity(shared)
        .expect("the villager joins the loaded chunk");

    let player = TestPlayerBuilder::new(Arc::clone(world), "PriceTester", 2).build();
    let traded_with = player.uuid();
    villager.gossips.lock().add(
        traded_with,
        GossipType::MajorPositive,
        GossipType::REPUTATION_CHANGE_PER_EVERLASTING_MEMORY,
    );
    // `GossipContainer.add` caps the value at the type maximum (20), weighted 5x.
    let reputation = villager.player_reputation(traded_with);
    assert_eq!(reputation, 100);

    // No trade screen is open yet, so the refresh is a no-op.
    assert!(player.trading_container_id().is_none());
    villager.update_special_prices(&player);

    let reputation_diffs: Vec<i32> = villager
        .merchant
        .offers()
        .iter()
        .map(MerchantOffer::special_price_diff)
        .collect();
    {
        let offers = villager.merchant.offers();
        for (offer, diff) in offers.iter().zip(&reputation_diffs) {
            assert_eq!(
                *diff,
                discount::reputation_price_discount(reputation, offer.price_multiplier()),
                "only the reputation term applies without Hero of the Village"
            );
        }
    }

    // `resetSpecialPrices` makes the call idempotent instead of accumulating discounts.
    villager.update_special_prices(&player);
    let repeated: Vec<i32> = villager
        .merchant
        .offers()
        .iter()
        .map(MerchantOffer::special_price_diff)
        .collect();
    assert_eq!(repeated, reputation_diffs);

    // Hero of the Village (amplifier 1) adds `-max(floor(0.3625 * count), 1)` per offer.
    LivingEntity::add_mob_effect(
        &*player,
        MobEffectInstance::with_duration(vanilla_mob_effects::HERO_OF_THE_VILLAGE, 200, 1),
    );
    villager.update_special_prices(&player);

    let hero_modifier = discount::hero_of_the_village_modifier(Some(1));
    {
        let offers = villager.merchant.offers();
        for (offer, reputation_diff) in offers.iter().zip(&reputation_diffs) {
            assert_eq!(
                offer.special_price_diff(),
                discount::special_price_diff(
                    reputation,
                    offer.price_multiplier(),
                    hero_modifier,
                    offer.base_cost_a().count(),
                ),
            );
            assert!(
                offer.special_price_diff() < *reputation_diff,
                "Hero of the Village reduces every offer by at least one"
            );
        }
    }

    // With a trade screen open the same call re-sends the offers for that container
    // (`Villager.updateSpecialPrices`'s `menu.updateSellItem()` + `sendMerchantOffers`).
    villager.start_trading(&player);
    assert!(villager.is_trading(), "`startTrading` records the player");
    assert!(
        player.trading_container_id().is_some(),
        "the trade screen is open"
    );
    villager.update_special_prices(&player);

    // `resetSpecialPrices` keeps the refresh idempotent with the menu open as well.
    let after_refresh: Vec<i32> = villager
        .merchant
        .offers()
        .iter()
        .map(MerchantOffer::special_price_diff)
        .collect();
    villager.update_special_prices(&player);
    let repeated_with_menu: Vec<i32> = villager
        .merchant
        .offers()
        .iter()
        .map(MerchantOffer::special_price_diff)
        .collect();
    assert_eq!(repeated_with_menu, after_refresh);
}

/// Vanilla `VillagerProfession.bootstrap`: only the farmer requests items and has a
/// secondary POI. This guards the build-generator wiring of
/// `build_assets/villager_professions.json` into `VillagerProfession`.
#[test]
fn profession_requested_items_and_secondary_poi_match_vanilla() {
    init_vanilla_registry();

    let farmer = &*vanilla_villager_professions::FARMER;
    let requested: Vec<_> = farmer
        .requested_items
        .iter()
        .map(|item| item.key.path.as_ref())
        .collect();
    assert_eq!(
        requested,
        vec!["wheat", "wheat_seeds", "beetroot_seeds", "bone_meal"],
        "farmer requested items come from the generator, not a hard-coded empty slice"
    );
    assert_eq!(
        farmer
            .secondary_poi
            .iter()
            .map(|block| block.key.path.as_ref())
            .collect::<Vec<_>>(),
        vec!["farmland"]
    );

    for profession in [
        &*vanilla_villager_professions::ARMORER,
        &*vanilla_villager_professions::LIBRARIAN,
        &*vanilla_villager_professions::NONE,
        &*vanilla_villager_professions::NITWIT,
    ] {
        assert!(
            profession.requested_items.is_empty() && profession.secondary_poi.is_empty(),
            "vanilla gives {} no requested items or secondary POI",
            profession.key
        );
    }
}
