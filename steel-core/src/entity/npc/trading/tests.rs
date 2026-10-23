//! Tests for the trading port.

use std::io::Cursor;
use std::sync::LazyLock;

use rand::SeedableRng as _;
use rand::rngs::StdRng;
use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::borrow::read_compound as read_borrowed_compound;
use simdnbt::owned::NbtCompound;
use steel_registry::data_component_predicate::DataComponentExactPredicate;
use steel_registry::data_components::DataComponentPatch;
use steel_registry::data_components::components::ItemEnchantments;
use steel_registry::data_components::vanilla_components::{
    ADDITIONAL_TRADE_COST, STORED_ENCHANTMENTS,
};
use steel_registry::enchantment::EnchantmentRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::item_stack_template::ItemStackTemplate;
use steel_registry::items::ItemRef;
use steel_registry::loot_table::{
    ConditionalLootFunction, LootContext, LootFunction, NumberProvider,
};
use steel_registry::trade::{TradeCost as DataTradeCost, TradeSet, VillagerTrade};
use steel_registry::villager_type::VillagerTypeRef;
use steel_registry::{
    init_vanilla_registry, vanilla_enchantments, vanilla_items, vanilla_villager_types,
};
use steel_utils::Identifier;
use steel_utils::serial::{ReadFrom as _, WriteTo as _};

use super::ItemCost;
use super::merchant_offer::{MerchantOffer, MerchantOffers};
use super::offer::{add_offers_from_trade_set, create_offer, trade_set_rng};

fn emeralds(count: i32) -> ItemStack {
    ItemStack::with_count(&vanilla_items::EMERALD, count)
}

fn cost(item: ItemRef, count: i32) -> ItemCost {
    ItemCost::new(item, count, DataComponentExactPredicate::EMPTY)
}

/// Vanilla `MerchantOffer.getModifiedCostCount` for the demand, price multiplier
/// and special price inputs that feed it.
#[test]
fn modified_cost_count_matches_vanilla() {
    init_vanilla_registry();

    // (item, base count, demand, price multiplier, special price, expected count)
    let cases: &[(ItemRef, i32, i32, f32, i32, i32)] = &[
        (&vanilla_items::EMERALD, 10, 0, 0.0, 0, 10),
        (&vanilla_items::EMERALD, 10, 0, 0.2, 0, 10),
        (&vanilla_items::EMERALD, 10, 5, 0.05, 0, 12),
        (&vanilla_items::EMERALD, 10, 5, 0.05, -4, 8),
        (&vanilla_items::EMERALD, 10, 5, 0.05, -100, 1),
        // Demand can go negative; the demand diff is clamped at zero.
        (&vanilla_items::EMERALD, 10, -20, 0.05, 0, 10),
        (&vanilla_items::EMERALD, 6, 10, 0.5, 0, 36),
        // Clamped to the result item's max stack size.
        (&vanilla_items::SNOWBALL, 14, 10, 0.5, 0, 16),
    ];

    for &(item, base, demand, multiplier, special, expected) in cases {
        let mut offer = MerchantOffer::with_demand(
            cost(item, base),
            None,
            ItemStack::new(&vanilla_items::DIAMOND),
            0,
            4,
            1,
            multiplier,
            demand,
        );
        offer.set_special_price_diff(special);

        assert_eq!(
            offer.cost_a().count(),
            expected,
            "base={base} demand={demand} multiplier={multiplier} special={special}"
        );
    }
}

#[test]
fn second_cost_and_result_are_not_price_adjusted() {
    init_vanilla_registry();

    let mut offer = MerchantOffer::with_demand(
        cost(&vanilla_items::EMERALD, 10),
        Some(cost(&vanilla_items::BOOK, 3)),
        ItemStack::new(&vanilla_items::DIAMOND),
        0,
        4,
        10,
        0.5,
        5,
    );
    offer.set_special_price_diff(20);

    assert_eq!(offer.cost_b().count(), 3);
    assert_eq!(offer.base_cost_a().count(), 10);
    assert_eq!(offer.item_cost_b().map(ItemCost::count), Some(3));
}

#[test]
fn satisfied_by_and_take_respect_the_second_cost() {
    init_vanilla_registry();

    let offer = MerchantOffer::with_second_cost(
        cost(&vanilla_items::EMERALD, 2),
        Some(cost(&vanilla_items::BOOK, 1)),
        ItemStack::new(&vanilla_items::DIAMOND),
        4,
        1,
        0.0,
    );

    let empty = ItemStack::empty();
    let book = ItemStack::with_count(&vanilla_items::BOOK, 1);

    assert!(offer.satisfied_by(&emeralds(2), &book));
    assert!(offer.satisfied_by(&emeralds(4), &book));
    assert!(!offer.satisfied_by(&emeralds(1), &book));
    assert!(!offer.satisfied_by(&emeralds(2), &empty));
    assert!(!offer.satisfied_by(&book, &book));

    // A trade without a second cost requires an empty second slot.
    let single = MerchantOffer::new(
        cost(&vanilla_items::EMERALD, 2),
        ItemStack::new(&vanilla_items::DIAMOND),
        4,
        1,
        0.0,
    );
    assert!(single.satisfied_by(&emeralds(2), &empty));
    assert!(!single.satisfied_by(&emeralds(2), &book));

    let mut buy_a = emeralds(3);
    let mut buy_b = ItemStack::with_count(&vanilla_items::BOOK, 2);
    assert!(offer.take(&mut buy_a, &mut buy_b));
    assert_eq!(buy_a.count(), 1);
    assert_eq!(buy_b.count(), 1);

    let mut too_few = emeralds(1);
    let mut buy_b = ItemStack::with_count(&vanilla_items::BOOK, 2);
    assert!(!offer.take(&mut too_few, &mut buy_b));
    assert_eq!(too_few.count(), 1);
    assert_eq!(buy_b.count(), 2);
}

#[test]
fn take_uses_the_adjusted_first_cost() {
    init_vanilla_registry();

    // 10 emeralds with demand 5 and a 0.05 multiplier cost 12.
    let offer = MerchantOffer::with_demand(
        cost(&vanilla_items::EMERALD, 10),
        None,
        ItemStack::new(&vanilla_items::DIAMOND),
        0,
        4,
        1,
        0.05,
        5,
    );
    assert_eq!(offer.cost_a().count(), 12);

    let mut empty = ItemStack::empty();
    let mut eleven = emeralds(11);
    assert!(!offer.satisfied_by(&eleven, &empty));
    assert!(!offer.take(&mut eleven, &mut empty));
    assert_eq!(eleven.count(), 11);

    let mut twelve = emeralds(12);
    assert!(offer.take(&mut twelve, &mut empty));
    assert_eq!(twelve.count(), 0);
}

#[test]
fn update_demand_follows_vanilla() {
    init_vanilla_registry();

    let mut offer = MerchantOffer::with_second_cost(
        cost(&vanilla_items::EMERALD, 1),
        None,
        ItemStack::new(&vanilla_items::DIAMOND),
        12,
        1,
        0.0,
    );

    offer.increase_uses();
    offer.increase_uses();
    offer.update_demand();

    // demand = demand + uses - (maxUses - uses)
    assert_eq!(offer.demand(), 2 - (12 - 2));
    assert!(offer.needs_restock());
    assert!(!offer.is_out_of_stock());

    offer.reset_uses();
    assert_eq!(offer.uses(), 0);
    assert!(!offer.needs_restock());

    offer.set_to_out_of_stock();
    assert!(offer.is_out_of_stock());
}

#[test]
fn offers_lookup_uses_the_selection_hint() {
    init_vanilla_registry();

    let mut offers = MerchantOffers::new();
    offers.push(MerchantOffer::new(
        cost(&vanilla_items::EMERALD, 1),
        ItemStack::new(&vanilla_items::DIAMOND),
        4,
        1,
        0.0,
    ));
    offers.push(MerchantOffer::new(
        cost(&vanilla_items::BOOK, 1),
        ItemStack::new(&vanilla_items::GOLD_INGOT),
        4,
        1,
        0.0,
    ));

    let book = ItemStack::with_count(&vanilla_items::BOOK, 1);
    let emerald = emeralds(1);
    let empty = ItemStack::empty();

    assert_eq!(
        offers
            .get_recipe_for(&book, &empty, 1)
            .map(|offer| offer.result().item().key.clone()),
        Some(vanilla_items::GOLD_INGOT.key.clone())
    );
    // A hint pointing at an offer that does not match falls back to a scan.
    assert_eq!(
        offers
            .get_recipe_for(&emerald, &empty, 1)
            .map(|offer| offer.result().item().key.clone()),
        Some(vanilla_items::DIAMOND.key.clone())
    );
    // No offer accepts a payment that is not one of its costs.
    assert!(
        offers
            .get_recipe_for(
                &ItemStack::with_count(&vanilla_items::DIAMOND, 1),
                &empty,
                0
            )
            .is_none()
    );

    offers
        .get_recipe_for_mut(&book, &empty, 1)
        .expect("the book offer should match")
        .increase_uses();
    assert_eq!(offers[1].uses(), 1);
}

fn base_trade(result: ItemRef, wants_count: i32) -> VillagerTrade {
    VillagerTrade {
        key: Identifier::vanilla_static("steel_test/trade"),
        wants: DataTradeCost {
            item: &vanilla_items::EMERALD,
            count: NumberProvider::Constant(wants_count as f32),
            components: &steel_registry::trade::EMPTY_COMPONENTS,
        },
        additional_wants: None,
        gives: ItemStackTemplate::new(result),
        max_uses: NumberProvider::Constant(12.0),
        xp: NumberProvider::Constant(5.0),
        reputation_discount: NumberProvider::Constant(0.2),
        variant_predicate: None,
        given_item_modifier: &[],
        double_trade_price_enchantments: &[],
    }
}

static EMERALD_TRADE: LazyLock<VillagerTrade> =
    LazyLock::new(|| base_trade(&vanilla_items::EMERALD, 1));
static DIAMOND_TRADE: LazyLock<VillagerTrade> =
    LazyLock::new(|| base_trade(&vanilla_items::DIAMOND, 1));
static GOLD_TRADE: LazyLock<VillagerTrade> =
    LazyLock::new(|| base_trade(&vanilla_items::GOLD_INGOT, 1));
/// A trade whose `wants` count of zero can never be paid.
static BROKEN_TRADE: LazyLock<VillagerTrade> =
    LazyLock::new(|| base_trade(&vanilla_items::STICK, 0));

static TRADES: LazyLock<[&'static VillagerTrade; 3]> =
    LazyLock::new(|| [&EMERALD_TRADE, &DIAMOND_TRADE, &GOLD_TRADE]);
static MIXED_TRADES: LazyLock<[&'static VillagerTrade; 3]> =
    LazyLock::new(|| [&EMERALD_TRADE, &BROKEN_TRADE, &GOLD_TRADE]);

fn trade_set(
    trades: &'static [&'static VillagerTrade],
    amount: f32,
    allow_duplicates: bool,
) -> TradeSet {
    TradeSet {
        key: Identifier::vanilla_static("steel_test/level_1"),
        trades,
        amount: NumberProvider::Constant(amount),
        allow_duplicates,
        random_sequence: None,
    }
}

fn generate(set: &TradeSet) -> MerchantOffers {
    let mut rng = trade_set_rng(42, set.random_sequence.as_ref());
    let mut ctx = LootContext::new(&mut rng);
    let mut offers = MerchantOffers::new();
    add_offers_from_trade_set(&mut ctx, &mut offers, set, None);
    offers
}

#[test]
fn trade_set_generates_the_vanilla_number_of_offers() {
    init_vanilla_registry();

    // `allow_duplicates: false` samples distinct trades.
    let unique = generate(&trade_set(&TRADES[..], 2.0, false));
    assert_eq!(unique.len(), 2);
    assert_ne!(unique[0].result().item().key, unique[1].result().item().key);

    assert_eq!(generate(&trade_set(&TRADES[..], 3.0, false)).len(), 3);
    assert_eq!(generate(&trade_set(&TRADES[..], 5.0, true)).len(), 5);
}

#[test]
fn trades_without_an_offer_are_skipped() {
    init_vanilla_registry();

    // Without duplicates a rejected trade is dropped from the sample, so the set
    // yields one offer less than requested.
    assert_eq!(generate(&trade_set(&MIXED_TRADES[..], 3.0, false)).len(), 2);
    // With duplicates the sample keeps drawing from the remaining trades.
    assert_eq!(generate(&trade_set(&MIXED_TRADES[..], 3.0, true)).len(), 3);
}

#[test]
fn trade_set_rng_is_seeded_from_the_sequence() {
    let sequence = Identifier::vanilla_static("trade_set/librarian/level_1");
    let other_sequence = Identifier::vanilla_static("trade_set/farmer/level_1");

    assert_eq!(
        rolls(trade_set_rng(42, Some(&sequence))),
        rolls(trade_set_rng(42, Some(&sequence)))
    );
    assert_ne!(
        rolls(trade_set_rng(42, Some(&sequence))),
        rolls(trade_set_rng(43, Some(&sequence)))
    );
    assert_ne!(
        rolls(trade_set_rng(42, Some(&sequence))),
        rolls(trade_set_rng(42, Some(&other_sequence)))
    );
}

fn rolls(mut rng: StdRng) -> Vec<i32> {
    use rand::RngExt as _;
    (0..8).map(|_| rng.random_range(0..1000)).collect()
}

static SWAMP_VARIANTS: [VillagerTypeRef; 1] = [&vanilla_villager_types::SWAMP];
static SWAMP_TRADE: LazyLock<VillagerTrade> = LazyLock::new(|| VillagerTrade {
    variant_predicate: Some(&SWAMP_VARIANTS),
    ..base_trade(&vanilla_items::EMERALD, 1)
});

#[test]
fn variant_predicate_filters_offers() {
    init_vanilla_registry();

    let mut rng = StdRng::seed_from_u64(1);
    let mut ctx = LootContext::new(&mut rng);
    assert!(create_offer(&SWAMP_TRADE, &mut ctx, None).is_none());
    assert!(
        create_offer(
            &SWAMP_TRADE,
            &mut ctx,
            Some(&vanilla_villager_types::PLAINS)
        )
        .is_none()
    );
    assert!(create_offer(&SWAMP_TRADE, &mut ctx, Some(&vanilla_villager_types::SWAMP)).is_some());
}

static DOUBLE_PRICE_ENCHANTMENTS: [EnchantmentRef; 1] = [&vanilla_enchantments::SHARPNESS];
static ADDITIONAL_COST_GIVES: LazyLock<ItemStackTemplate> = LazyLock::new(|| {
    let mut enchantments = ItemEnchantments::empty();
    enchantments.set(vanilla_enchantments::SHARPNESS.key.clone(), 1);

    let mut patch = DataComponentPatch::new();
    patch.set(ADDITIONAL_TRADE_COST, 4);
    patch.set(STORED_ENCHANTMENTS, enchantments);

    ItemStackTemplate::try_with_count_and_patch(&vanilla_items::EMERALD, 1, patch)
        .expect("the test template should be valid")
});
static ADDITIONAL_COST_TRADE: LazyLock<VillagerTrade> = LazyLock::new(|| VillagerTrade {
    gives: (*ADDITIONAL_COST_GIVES).clone(),
    double_trade_price_enchantments: &DOUBLE_PRICE_ENCHANTMENTS,
    ..base_trade(&vanilla_items::EMERALD, 2)
});

#[test]
fn offer_consumes_the_additional_trade_cost_component() {
    init_vanilla_registry();

    let mut rng = StdRng::seed_from_u64(3);
    let mut ctx = LootContext::new(&mut rng);
    let offer =
        create_offer(&ADDITIONAL_COST_TRADE, &mut ctx, None).expect("the trade should be offered");

    // 2 base emeralds + 4 additional cost, doubled by the stored enchantment.
    assert_eq!(offer.cost_a().count(), 10);
    assert!(!offer.result().has(ADDITIONAL_TRADE_COST));
}

static DISCARD_MODIFIERS: [ConditionalLootFunction; 1] = [ConditionalLootFunction {
    function: LootFunction::SetCount {
        count: NumberProvider::Constant(0.0),
        add: false,
    },
    conditions: &[],
}];
static DISCARD_TRADE: LazyLock<VillagerTrade> = LazyLock::new(|| VillagerTrade {
    given_item_modifier: &DISCARD_MODIFIERS,
    ..base_trade(&vanilla_items::EMERALD, 1)
});

#[test]
fn given_item_modifier_can_discard_a_trade() {
    init_vanilla_registry();

    let mut rng = StdRng::seed_from_u64(5);
    let mut ctx = LootContext::new(&mut rng);
    assert!(create_offer(&DISCARD_TRADE, &mut ctx, None).is_none());
}

#[test]
fn offer_nbt_round_trip_preserves_state() {
    init_vanilla_registry();

    let mut offers = MerchantOffers::new();
    let mut offer = MerchantOffer::with_demand(
        cost(&vanilla_items::EMERALD, 4),
        Some(cost(&vanilla_items::BOOK, 1)),
        ItemStack::with_count(&vanilla_items::DIAMOND, 2),
        0,
        12,
        7,
        0.05,
        3,
    );
    offer.increase_uses();
    offer.increase_uses();
    offer.update_demand();
    offer.add_to_special_price_diff(-2);
    offers.push(offer);

    // A second offer with every optional field left at its default.
    offers.push(MerchantOffer::new(
        cost(&vanilla_items::GOLD_INGOT, 1),
        ItemStack::new(&vanilla_items::EMERALD),
        4,
        1,
        0.0,
    ));

    let restored = with_borrowed_nbt(&offers.to_nbt(), |nbt| {
        MerchantOffers::from_nbt(nbt).expect("offers should decode")
    });

    assert_eq!(restored.len(), 2);

    let first = &restored[0];
    assert_eq!(first.uses(), 2);
    assert_eq!(first.max_uses(), 12);
    assert_eq!(first.xp(), 7);
    assert_eq!(first.demand(), 3 + 2 - (12 - 2));
    assert_eq!(first.special_price_diff(), -2);
    assert_eq!(first.price_multiplier(), 0.05);
    assert_eq!(first.base_cost_a().count(), 4);
    assert_eq!(first.cost_b().count(), 1);
    assert_eq!(first.result().count(), 2);
    assert_eq!(first.result().item().key, vanilla_items::DIAMOND.key);

    let second = &restored[1];
    assert_eq!(second.uses(), 0);
    assert_eq!(second.max_uses(), 4);
    assert_eq!(second.xp(), 1);
    assert_eq!(second.demand(), 0);
    assert_eq!(second.special_price_diff(), 0);
    assert_eq!(second.price_multiplier(), 0.0);
    assert!(second.should_reward_exp());
    assert_eq!(second.cost_b().count(), 0);
}

#[test]
fn stream_round_trip_preserves_state() {
    init_vanilla_registry();

    let mut offer = MerchantOffer::with_demand(
        cost(&vanilla_items::EMERALD, 4),
        Some(cost(&vanilla_items::BOOK, 1)),
        ItemStack::new(&vanilla_items::DIAMOND),
        0,
        12,
        7,
        0.05,
        3,
    );
    offer.increase_uses();
    offer.set_special_price_diff(-2);

    let mut offers = MerchantOffers::new();
    offers.push(offer.copy());
    offers.push(MerchantOffer::new(
        cost(&vanilla_items::GOLD_INGOT, 1),
        ItemStack::new(&vanilla_items::EMERALD),
        4,
        1,
        0.0,
    ));
    offers[1].set_to_out_of_stock();

    let mut encoded = Vec::new();
    offers.write(&mut encoded).expect("offers should encode");
    let decoded =
        MerchantOffers::read(&mut Cursor::new(encoded.as_slice())).expect("offers should decode");

    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].uses(), 1);
    assert_eq!(decoded[0].max_uses(), 12);
    assert_eq!(decoded[0].xp(), 7);
    assert_eq!(decoded[0].demand(), 3);
    assert_eq!(decoded[0].special_price_diff(), -2);
    assert_eq!(decoded[0].price_multiplier(), 0.05);
    assert_eq!(decoded[0].cost_b().count(), 1);
    assert!(decoded[1].is_out_of_stock());
    assert_eq!(decoded[1].uses(), 4);
}

/// Borrowed view of an owned compound, matching how entities load saved NBT.
fn with_borrowed_nbt<T>(nbt: &NbtCompound, read: impl FnOnce(&NbtCompoundView<'_, '_>) -> T) -> T {
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let base = read_borrowed_compound(&mut Cursor::new(bytes.as_slice()))
        .expect("test nbt should reborrow");
    read(&(&base).into())
}
