//! Vanilla enchanting-table maths, ported from `EnchantmentHelper`,
//! `EnchantmentMenu` and `WeightedRandom`.
//!
//! These functions are the single source of truth for the enchanting table
//! (cost slots, clues and `selectEnchantment`), the `enchant_with_levels` loot
//! function and the `enchantment_provider` registry: all of them call the exact
//! same routines in vanilla.
//!
//! Vanilla drives these from a `RandomSource`. Steel has two RNG families: the
//! vanilla-faithful [`Random`](steel_utils::random::Random) (used for world and
//! player-seeded determinism) and `rand::Rng` (used by the loot runtime). The
//! maths therefore takes [`EnchantmentRandom`], implemented for
//! `LegacyRandom`/`Xoroshiro` directly and for `rand::Rng` through
//! [`RandRngAdapter`]. The adapter keeps the same distributions; only the exact
//! sequence differs from vanilla's `LegacyRandomSource`, matching the existing
//! loot-runtime deviation.

use crate::data_components::vanilla_components::{ADDITIONAL_TRADE_COST, ENCHANTABLE};
use crate::enchantment::{Enchantment, EnchantmentRef};
use crate::item_stack::ItemStack;
use crate::loot_table::EnchantmentOptions;
use crate::{REGISTRY, RegistryExt, TaggedRegistryExt, vanilla_items};
use rand::RngExt as _;
use steel_utils::Identifier;
use steel_utils::random::{Random, legacy_random::LegacyRandom, xoroshiro::Xoroshiro};

/// The random operations vanilla's enchanting maths needs.
pub trait EnchantmentRandom {
    /// Vanilla `RandomSource.nextInt(bound)`.
    fn next_int(&mut self, bound: i32) -> i32;
    /// Vanilla `RandomSource.nextFloat()`.
    fn next_float(&mut self) -> f32;
}

impl EnchantmentRandom for LegacyRandom {
    fn next_int(&mut self, bound: i32) -> i32 {
        self.next_i32_bounded(bound)
    }

    fn next_float(&mut self) -> f32 {
        self.next_f32()
    }
}

impl EnchantmentRandom for Xoroshiro {
    fn next_int(&mut self, bound: i32) -> i32 {
        self.next_i32_bounded(bound)
    }

    fn next_float(&mut self) -> f32 {
        self.next_f32()
    }
}

/// Adapts a `rand::Rng` to [`EnchantmentRandom`] for the loot runtime.
///
/// **Deviation from vanilla:** `rand`'s bounded sampling and float conversion
/// are distributionally identical to `LegacyRandomSource` but not bit-identical;
/// Steel's loot runtime already uses `rand::Rng` instead of the level
/// `RandomSource`, so the adapter matches that existing choice.
pub struct RandRngAdapter<'a, R: rand::Rng>(pub &'a mut R);

impl<R: rand::Rng> EnchantmentRandom for RandRngAdapter<'_, R> {
    fn next_int(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        self.0.random_range(0..bound)
    }

    fn next_float(&mut self) -> f32 {
        self.0.random::<f32>()
    }
}

/// A single enchantment level, mirroring vanilla `EnchantmentInstance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnchantmentInstance {
    pub enchantment: EnchantmentRef,
    pub level: i32,
}

impl EnchantmentInstance {
    /// Vanilla `EnchantmentInstance.weight`: the enchantment's own weight.
    #[must_use]
    pub const fn weight(&self) -> u32 {
        self.enchantment.weight
    }
}

/// One enchanting-table slot offer, mirroring the menu's `costs`/`enchantClue`/
/// `levelClue` arrays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnchantmentOffer {
    /// XP level cost shown in the slot (0 = slot disabled).
    pub cost: i32,
    /// Clue enchantment id (`-1` when the slot shows no clue).
    pub clue: i32,
    /// Clue enchantment level (`-1` when the slot shows no clue).
    pub clue_level: i32,
}

/// Vanilla `EnchantmentHelper.getEnchantmentCost`: the XP level cost shown and
/// consumed by one enchanting-table slot.
///
/// `slot` is the menu slot index (0, 1, 2) and `bookcases` the powered-bookshelf
/// count around the table. Returns 0 when the item is not enchantable.
#[must_use]
pub fn get_enchantment_cost(
    random: &mut impl EnchantmentRandom,
    slot: i32,
    bookcases: i32,
    item: &ItemStack,
) -> i32 {
    if item.get(ENCHANTABLE).is_none() {
        return 0;
    }

    let bookcases = bookcases.min(15);
    let selected = random.next_int(8) + 1 + (bookcases >> 1) + random.next_int(bookcases + 1);

    if slot == 0 {
        (selected / 3).max(1)
    } else if slot == 1 {
        selected * 2 / 3 + 1
    } else {
        selected.max(bookcases * 2)
    }
}

/// Vanilla `EnchantmentMenu.slotsChanged`: the three slot offers for the given
/// player enchantment seed.
///
/// Vanilla reseeds one `RandomSource` with the seed for the costs and then with
/// `seed + slot` for each clue; the same Legacy-RNG sequence is reproduced here.
#[must_use]
pub fn enchanting_table_offers(
    enchantment_seed: i64,
    bookcases: i32,
    item: &ItemStack,
) -> [EnchantmentOffer; 3] {
    let mut offers = [EnchantmentOffer::default(); 3];
    let mut random = LegacyRandom::from_seed(enchantment_seed as u64);

    for (slot, offer) in offers.iter_mut().enumerate() {
        let slot = slot as i32;
        offer.cost = get_enchantment_cost(&mut random, slot, bookcases, item);
        offer.clue = -1;
        offer.clue_level = -1;
        if offer.cost < slot + 1 {
            offer.cost = 0;
        }
    }

    for (slot, offer) in offers.iter_mut().enumerate() {
        if offer.cost <= 0 {
            continue;
        }
        let mut random = LegacyRandom::from_seed((enchantment_seed + slot as i64) as u64);
        let list = get_enchantment_list(&mut random, item, slot as i32, offer.cost);
        if !list.is_empty() {
            let index = random.next_int(list.len() as i32) as usize;
            let instance = list[index];
            offer.clue = REGISTRY
                .enchantments
                .id_from_key(&instance.enchantment.key)
                .map_or(-1, |id| id as i32);
            offer.clue_level = instance.level;
        }
    }

    offers
}

/// Vanilla `EnchantmentMenu.getEnchantmentList`: `selectEnchantment` restricted
/// to the `minecraft:in_enchanting_table` tag, with vanilla's extra book
/// removal.
#[must_use]
pub fn get_enchantment_list(
    random: &mut impl EnchantmentRandom,
    item: &ItemStack,
    slot: i32,
    enchantment_cost: i32,
) -> Vec<EnchantmentInstance> {
    let tag = Identifier::vanilla_static("in_enchanting_table");
    let Some(source) = REGISTRY.enchantments.get_tag(&tag) else {
        return Vec::new();
    };

    let mut list = select_enchantment(random, item, enchantment_cost, &source);
    if item.is(&vanilla_items::BOOK) && list.len() > 1 {
        let index = random.next_int(list.len() as i32) as usize;
        list.remove(index);
    }

    let _ = slot;
    list
}

/// Every registered enchantment, mirroring vanilla
/// `registryAccess.lookupOrThrow(Registries.ENCHANTMENT).listElements()`.
#[must_use]
pub fn all_enchantments() -> Vec<EnchantmentRef> {
    REGISTRY
        .enchantments
        .iter()
        .map(|(_, enchantment)| enchantment)
        .collect()
}

/// Resolves a loot `EnchantmentOptions` (tag or explicit list) into enchantments.
///
/// Mirrors vanilla `HolderSet::stream` for the `options` field of
/// `EnchantRandomlyFunction` / `EnchantWithLevelsFunction`.
#[must_use]
pub fn enchantments_in_options(options: &EnchantmentOptions) -> Vec<EnchantmentRef> {
    match options {
        EnchantmentOptions::Tag(tag) => REGISTRY.enchantments.get_tag(tag).unwrap_or_default(),
        EnchantmentOptions::List(keys) => keys
            .iter()
            .filter_map(|key| REGISTRY.enchantments.by_key(key))
            .collect(),
    }
}

/// Vanilla `EnchantmentHelper.selectEnchantment`: rolls the enchantments granted
/// by one table/loot operation of the given cost.
#[must_use]
pub fn select_enchantment(
    random: &mut impl EnchantmentRandom,
    item: &ItemStack,
    enchantment_cost: i32,
    source: &[EnchantmentRef],
) -> Vec<EnchantmentInstance> {
    let mut results = Vec::new();
    let Some(enchantability) = item.get(ENCHANTABLE).map(|enchantable| enchantable.value()) else {
        return results;
    };

    let mut enchantment_cost = enchantment_cost
        + 1
        + random.next_int(enchantability / 4 + 1)
        + random.next_int(enchantability / 4 + 1);
    let random_span = (random.next_float() + random.next_float() - 1.0) * 0.15;
    let scaled = (enchantment_cost as f32 + enchantment_cost as f32 * random_span).round();
    enchantment_cost = (scaled as i32).clamp(1, i32::MAX);

    let mut enchantments = get_available_enchantment_results(enchantment_cost, item, source);
    if !enchantments.is_empty() {
        if let Some(picked) = weighted_random_item(random, &enchantments) {
            results.push(picked);
        }

        while random.next_int(50) <= enchantment_cost {
            if let Some(last) = results.last() {
                filter_compatible_enchantments(&mut enchantments, last);
            }

            if enchantments.is_empty() {
                break;
            }

            if let Some(picked) = weighted_random_item(random, &enchantments) {
                results.push(picked);
            }
            enchantment_cost /= 2;
        }
    }

    results
}

/// Vanilla `EnchantmentHelper.getAvailableEnchantmentResults`: all enchantment
/// levels whose min/max cost window contains `value`.
#[must_use]
pub fn get_available_enchantment_results(
    value: i32,
    item: &ItemStack,
    source: &[EnchantmentRef],
) -> Vec<EnchantmentInstance> {
    let is_book = item.is(&vanilla_items::BOOK);
    let mut results = Vec::new();

    for enchantment in source {
        if !enchantment.is_primary_item(item) && !is_book {
            continue;
        }

        for level in (enchantment.get_min_level()..=enchantment.get_max_level()).rev() {
            if value >= enchantment.get_min_cost(level) && value <= enchantment.get_max_cost(level)
            {
                results.push(EnchantmentInstance { enchantment, level });
                break;
            }
        }
    }

    results
}

/// Vanilla `EnchantmentHelper.filterCompatibleEnchantments`.
pub fn filter_compatible_enchantments(
    enchants: &mut Vec<EnchantmentInstance>,
    target: &EnchantmentInstance,
) {
    enchants
        .retain(|candidate| Enchantment::are_compatible(target.enchantment, candidate.enchantment));
}

/// Vanilla `EnchantmentHelper.isEnchantmentCompatible`.
#[must_use]
pub fn is_enchantment_compatible(existing: &[EnchantmentRef], target: EnchantmentRef) -> bool {
    existing
        .iter()
        .all(|enchantment| Enchantment::are_compatible(enchantment, target))
}

/// Vanilla `EnchantmentHelper.enchantItem`: applies `selectEnchantment` to the
/// stack, converting a plain book into an enchanted book first.
#[must_use]
pub fn enchant_item(
    random: &mut impl EnchantmentRandom,
    item: ItemStack,
    enchantment_cost: i32,
    source: &[EnchantmentRef],
) -> ItemStack {
    let enchants = select_enchantment(random, &item, enchantment_cost, source);
    let mut item = item;

    if item.is(&vanilla_items::BOOK) {
        item.set_item(&vanilla_items::ENCHANTED_BOOK.key);
    }

    for enchant in enchants {
        item.upgrade_enchantment(enchant.enchantment.key.clone(), enchant.level as u32);
    }

    item
}

/// Vanilla `EnchantmentsByCost` / `EnchantmentsByCostWithDifficulty` body: rolls
/// `selectEnchantment` at `cost` and upgrades every result onto `item`
/// (`ItemEnchantments.Mutable.upgrade`).
pub fn enchant_item_from_provider_selection(
    item: &mut ItemStack,
    cost: i32,
    source: &[EnchantmentRef],
    random: &mut impl EnchantmentRandom,
) {
    for instance in select_enchantment(random, item, cost, source) {
        item.upgrade_enchantment(instance.enchantment.key.clone(), instance.level as u32);
    }
}

/// Vanilla `EnchantmentHelper.createBook`.
#[must_use]
pub fn create_book(instance: &EnchantmentInstance) -> ItemStack {
    let mut item = ItemStack::new(&vanilla_items::ENCHANTED_BOOK);
    item.upgrade_enchantment(instance.enchantment.key.clone(), instance.level as u32);
    item
}

/// Vanilla `EnchantRandomlyFunction.enchantItem` level roll
/// (`Mth.nextInt(random, getMinLevel(), getMaxLevel())`).
#[must_use]
pub fn random_enchantment_level(
    random: &mut impl EnchantmentRandom,
    enchantment: EnchantmentRef,
) -> i32 {
    let min = enchantment.get_min_level();
    let max = enchantment.get_max_level();
    if min >= max {
        min
    } else {
        random.next_int(max - min + 1) + min
    }
}

/// Vanilla `EnchantRandomlyFunction.enchantItem` additional-cost component
/// (`2 + random.nextInt(5 + level * 10) + 3 * level`).
pub fn apply_additional_trade_cost(
    item: &mut ItemStack,
    level: i32,
    random: &mut impl EnchantmentRandom,
) {
    let cost = 2 + random.next_int(5 + level * 10) + 3 * level;
    item.set(ADDITIONAL_TRADE_COST, cost);
}

/// Vanilla `WeightedRandom.getRandomItem`: weighted pick over enchantment
/// weights, returning `None` for an empty or zero-weight list.
#[must_use]
pub fn weighted_random_item(
    random: &mut impl EnchantmentRandom,
    items: &[EnchantmentInstance],
) -> Option<EnchantmentInstance> {
    let total_weight: i64 = items.iter().map(|item| i64::from(item.weight())).sum();
    if total_weight <= 0 || total_weight > i64::from(i32::MAX) {
        return None;
    }

    let mut selection = random.next_int(total_weight as i32);
    for item in items {
        selection -= item.weight() as i32;
        if selection < 0 {
            return Some(*item);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{EnchantmentInstance, get_enchantment_cost, weighted_random_item};
    use crate::item_stack::ItemStack;
    use crate::vanilla_items;
    use steel_utils::random::legacy_random::LegacyRandom;

    #[test]
    fn unenchantable_item_has_no_cost() {
        let mut random = LegacyRandom::from_seed(1);
        let item = ItemStack::new(&vanilla_items::STONE);

        assert_eq!(get_enchantment_cost(&mut random, 0, 15, &item), 0);
    }

    #[test]
    fn weight_is_taken_from_the_enchantment() {
        let instance = EnchantmentInstance {
            enchantment: &crate::vanilla_enchantments::SHARPNESS,
            level: 1,
        };

        assert_eq!(
            instance.weight(),
            crate::vanilla_enchantments::SHARPNESS.weight
        );
    }

    #[test]
    fn zero_weight_lists_return_none() {
        let mut random = LegacyRandom::from_seed(1);

        assert!(weighted_random_item(&mut random, &[]).is_none());
    }
}
