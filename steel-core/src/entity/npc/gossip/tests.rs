//! Tests for the gossip container, its persistence and the reputation price math.

use std::io::Cursor;

use simdnbt::borrow::read_compound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::UuidExt as _;
use steel_utils::random::RandomSource;
use steel_utils::random::legacy_random::LegacyRandom;
use uuid::Uuid;

use super::MAX_GOSSIP_TOPICS;
use super::container::{GossipContainer, GossipEntry, merge_values_for_addition};
use super::discount::{
    hero_of_the_village_modifier, hero_of_the_village_price_discount, reputation_price_discount,
    special_price_diff,
};
use super::gossip_type::GossipType;

fn seeded_random() -> RandomSource {
    RandomSource::Legacy(LegacyRandom::from_seed(1))
}

fn entry(target: Uuid, kind: GossipType, value: i32) -> GossipEntry {
    GossipEntry {
        target,
        kind,
        value,
    }
}

/// The full stored state, ordered so assertions are independent of map iteration order.
fn values(container: &GossipContainer) -> Vec<(Uuid, GossipType, i32)> {
    let mut values: Vec<_> = container
        .unpack()
        .map(|entry| (entry.target, entry.kind, entry.value))
        .collect();
    values.sort_by_key(|(target, kind, value)| (target.as_u128(), kind.id(), *value));
    values
}

#[test]
fn add_caps_the_value_at_the_type_maximum() {
    let id = Uuid::from_u128(1);

    // MAJOR_POSITIVE: max 20, weight 5.
    let mut positive = GossipContainer::new();
    for _ in 0..5 {
        positive.add(
            id,
            GossipType::MajorPositive,
            GossipType::REPUTATION_CHANGE_PER_EVENT,
        );
    }
    assert_eq!(values(&positive), vec![(id, GossipType::MajorPositive, 20)]);
    assert_eq!(positive.get_reputation(id, |_| true), 100);

    // MINOR_NEGATIVE: max 200, weight -1.
    let mut negative = GossipContainer::new();
    for _ in 0..10 {
        negative.add(
            id,
            GossipType::MinorNegative,
            GossipType::REPUTATION_CHANGE_PER_EVENT,
        );
    }
    assert_eq!(
        values(&negative),
        vec![(id, GossipType::MinorNegative, 200)]
    );
    assert_eq!(negative.get_reputation(id, |_| true), -200);
}

#[test]
fn saturating_addition_never_lowers_an_out_of_range_value() {
    // Vanilla `GossipContainer.mergeValuesForAddition`.
    assert_eq!(
        merge_values_for_addition(GossipType::MajorPositive, 20, 25),
        20
    );
    assert_eq!(
        merge_values_for_addition(GossipType::MajorPositive, 10, 5),
        15
    );
    assert_eq!(
        merge_values_for_addition(GossipType::MajorPositive, 30, 5),
        30
    );
}

#[test]
fn add_discards_values_below_the_discard_threshold() {
    let id = Uuid::from_u128(1);
    let mut container = GossipContainer::new();
    container.add(
        id,
        GossipType::MinorPositive,
        GossipType::REPUTATION_CHANGE_PER_EVENT,
    );
    container.remove(id, GossipType::MinorPositive, 24);
    assert!(container.is_empty());
    assert_eq!(container.get_reputation(id, |_| true), 0);
}

#[test]
fn decay_subtracts_decay_per_day_and_drops_small_values() {
    let id = Uuid::from_u128(1);
    let other = Uuid::from_u128(2);
    let mut container = GossipContainer::from_entries([
        entry(id, GossipType::MajorNegative, 100),
        entry(id, GossipType::MinorNegative, 21),
        entry(id, GossipType::MinorPositive, 3),
        entry(id, GossipType::MajorPositive, 5),
        entry(id, GossipType::Trading, 4),
        entry(other, GossipType::Trading, 3),
    ]);

    container.decay();
    assert_eq!(
        values(&container),
        vec![
            (id, GossipType::MajorNegative, 90),
            (id, GossipType::MajorPositive, 5),
            (id, GossipType::MinorPositive, 2),
            (id, GossipType::Trading, 2),
        ]
    );

    // MINOR_POSITIVE and TRADING drop out, removing `other` entirely.
    container.decay();
    assert_eq!(
        values(&container),
        vec![
            (id, GossipType::MajorNegative, 80),
            (id, GossipType::MajorPositive, 5),
        ]
    );
}

#[test]
fn transfer_moves_a_decayed_copy_and_leaves_the_source_alone() {
    let id = Uuid::from_u128(7);
    let source = GossipContainer::from_entries([entry(id, GossipType::MajorNegative, 100)]);
    let mut target = GossipContainer::new();

    assert_eq!(
        target.transfer_from(&source, &mut seeded_random(), MAX_GOSSIP_TOPICS),
        1
    );
    assert_eq!(values(&target), vec![(id, GossipType::MajorNegative, 90)]);
    assert_eq!(values(&source), vec![(id, GossipType::MajorNegative, 100)]);

    // Both containers decay on their own clock afterwards.
    target.decay();
    assert_eq!(values(&target), vec![(id, GossipType::MajorNegative, 80)]);
}

#[test]
fn transfer_merges_with_the_higher_value() {
    let id = Uuid::from_u128(1);
    let source = GossipContainer::from_entries([entry(id, GossipType::MajorNegative, 100)]);
    let mut target = GossipContainer::from_entries([entry(id, GossipType::MajorNegative, 95)]);

    assert_eq!(
        target.transfer_from(&source, &mut seeded_random(), MAX_GOSSIP_TOPICS),
        1
    );
    assert_eq!(values(&target), vec![(id, GossipType::MajorNegative, 95)]);
}

#[test]
fn transfer_counts_gossip_that_decays_below_the_threshold_but_stores_nothing() {
    let id = Uuid::from_u128(1);
    let source = GossipContainer::from_entries([entry(id, GossipType::MinorPositive, 6)]);
    let mut target = GossipContainer::new();

    assert_eq!(
        target.transfer_from(&source, &mut seeded_random(), MAX_GOSSIP_TOPICS),
        1
    );
    assert!(target.is_empty());
}

#[test]
fn transfer_moves_at_most_max_gossip_topics() {
    let ids: Vec<Uuid> = (1..=4).map(Uuid::from_u128).collect();
    let source = GossipContainer::from_entries(ids.iter().flat_map(|&id| {
        [
            entry(id, GossipType::MajorNegative, 100),
            entry(id, GossipType::MinorNegative, 100),
            entry(id, GossipType::MinorPositive, 25),
            entry(id, GossipType::MajorPositive, 30),
            entry(id, GossipType::Trading, 25),
        ]
    }));
    assert_eq!(source.unpack().count(), 20);

    let mut target = GossipContainer::new();
    let received = target.transfer_from(&source, &mut seeded_random(), MAX_GOSSIP_TOPICS);
    assert!(received <= MAX_GOSSIP_TOPICS, "received {received}");
    assert_eq!(target.unpack().count(), received);

    // Everything moves when the source holds fewer topics than the cap and no two draws collide.
    let small = GossipContainer::from_entries([
        entry(ids[0], GossipType::MajorNegative, 100),
        entry(ids[0], GossipType::MinorPositive, 25),
        entry(ids[1], GossipType::Trading, 25),
    ]);
    let mut target = GossipContainer::new();
    assert_eq!(
        target.transfer_from(&small, &mut seeded_random(), MAX_GOSSIP_TOPICS),
        3
    );
    assert_eq!(target.unpack().count(), 3);
}

#[test]
fn nbt_round_trip_preserves_gossip() {
    let a = Uuid::from_u128(1);
    let b = Uuid::from_u128(0xdead_beef);
    let original = GossipContainer::from_entries([
        entry(a, GossipType::MajorNegative, 100),
        entry(a, GossipType::Trading, 25),
        entry(b, GossipType::MinorPositive, 3),
    ]);

    let mut nbt = NbtCompound::new();
    original.save_to(&mut nbt);
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed =
        read_compound(&mut Cursor::new(bytes.as_slice())).expect("gossip nbt should reborrow");

    let mut loaded = GossipContainer::new();
    loaded.load_from((&borrowed).into());
    assert_eq!(values(&loaded), values(&original));
}

#[test]
fn nbt_load_skips_malformed_entries() {
    let target = Uuid::from_u128(5);
    let target_array = NbtTag::IntArray(target.to_int_array().to_vec());

    let mut good = NbtCompound::new();
    good.insert("Target", target_array.clone());
    good.insert("Type", "minor_negative");
    good.insert("Value", 7);

    // Removed gossip type, as still found in old saves (vanilla `RemoveGolemGossipFix`).
    let mut unknown_type = NbtCompound::new();
    unknown_type.insert("Target", target_array.clone());
    unknown_type.insert("Type", "golem");
    unknown_type.insert("Value", 50);

    // Vanilla's codec requires `ExtraCodecs.POSITIVE_INT`.
    let mut zero_value = NbtCompound::new();
    zero_value.insert("Target", target_array);
    zero_value.insert("Type", "trading");
    zero_value.insert("Value", 0);

    let mut missing_target = NbtCompound::new();
    missing_target.insert("Type", "trading");
    missing_target.insert("Value", 5);

    let mut nbt = NbtCompound::new();
    nbt.insert(
        "Gossips",
        NbtList::Compound(vec![good, unknown_type, zero_value, missing_target]),
    );
    let mut bytes = Vec::new();
    nbt.write(&mut bytes);
    let borrowed =
        read_compound(&mut Cursor::new(bytes.as_slice())).expect("gossip nbt should reborrow");

    let mut loaded =
        GossipContainer::from_entries([entry(Uuid::from_u128(9), GossipType::Trading, 9)]);
    loaded.load_from((&borrowed).into());
    assert_eq!(
        values(&loaded),
        vec![(target, GossipType::MinorNegative, 7)]
    );
}

#[test]
fn reputation_sums_weighted_values_of_selected_types() {
    let id = Uuid::from_u128(1);
    let mut container = GossipContainer::new();
    container.add(id, GossipType::MajorPositive, 20);
    container.add(
        id,
        GossipType::MinorNegative,
        GossipType::REPUTATION_CHANGE_PER_EVENT,
    );
    container.add(
        id,
        GossipType::Trading,
        GossipType::REPUTATION_CHANGE_PER_TRADE,
    );

    assert_eq!(container.get_reputation(id, |_| true), 77);
    assert_eq!(container.get_reputation(id, |kind| kind.weight() > 0), 102);
    assert_eq!(container.get_reputation(Uuid::from_u128(2), |_| true), 0);
    assert_eq!(
        container.get_count_for_type(GossipType::MajorPositive, |value| value > 0.0),
        1
    );
    assert_eq!(
        container.get_count_for_type(GossipType::MinorNegative, |value| value > 0.0),
        0
    );
}

#[test]
fn reputation_and_hero_discount_match_vanilla_price_math() {
    // Villager.java:417-420: `-Mth.floor(reputation * offer.getPriceMultiplier())`.
    assert_eq!(reputation_price_discount(20, 0.05), -1);
    assert_eq!(reputation_price_discount(100, 0.05), -5);
    assert_eq!(reputation_price_discount(-25, 0.2), 5);
    // `Mth.floor` rounds toward negative infinity, so a fractional reputation increase costs 2.
    assert_eq!(reputation_price_discount(-25, 0.05), 2);
    assert_eq!(reputation_price_discount(0, 0.05), 0);

    // Villager.java:414-415: `0.3F + 0.0625F * amplifier`, evaluated in f32.
    assert_eq!(hero_of_the_village_modifier(None), 0.0);
    assert_eq!(hero_of_the_village_modifier(Some(0)), f64::from(0.3_f32));
    assert_eq!(
        hero_of_the_village_modifier(Some(4)),
        f64::from(0.3_f32 + 0.0625_f32 * 4.0)
    );

    // Villager.java:422-425: `-Math.max((int)Math.floor(modifier * baseCount), 1)`.
    assert_eq!(
        hero_of_the_village_price_discount(hero_of_the_village_modifier(Some(0)), 4),
        -1
    );
    assert_eq!(
        hero_of_the_village_price_discount(hero_of_the_village_modifier(Some(0)), 1),
        -1
    );
    assert_eq!(
        hero_of_the_village_price_discount(hero_of_the_village_modifier(Some(4)), 4),
        -2
    );

    // Villager.java:410-427: both terms are added on top of a reset special price diff.
    assert_eq!(
        special_price_diff(100, 0.05, hero_of_the_village_modifier(Some(4)), 4),
        -7
    );
    assert_eq!(
        special_price_diff(0, 0.05, hero_of_the_village_modifier(None), 4),
        0
    );
}
