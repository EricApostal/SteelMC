//! Offer generation: vanilla `VillagerTrade.getOffer` and
//! `AbstractVillager.addOffersFromTradeSet`.

use rand::rngs::StdRng;
use rand::{RngExt as _, SeedableRng as _};
use steel_registry::data_components::vanilla_components::{
    ADDITIONAL_TRADE_COST, STORED_ENCHANTMENTS,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::trade::{TradeCost, TradeSet, VillagerTrade};
use steel_registry::villager_type::VillagerTypeRef;
use steel_utils::Identifier;

use crate::entity::npc::trading::item_cost::ItemCost;
use crate::entity::npc::trading::merchant_offer::{MerchantOffer, MerchantOffers};

/// Vanilla `VillagerTrade.getOffer`.
///
/// Vanilla evaluates the `merchant_predicate` loot condition against the trading
/// entity; Steel's loot `EntityRef` has no villager-variant component, so the
/// entity's own type is matched against the trade's predicate directly.
pub fn create_offer<R: rand::Rng>(
    trade: &VillagerTrade,
    ctx: &mut LootContext<'_, R>,
    villager_type: Option<VillagerTypeRef>,
) -> Option<MerchantOffer> {
    if let Some(variants) = trade.variant_predicate
        && !villager_type.is_some_and(|villager_type| variants.contains(&villager_type))
    {
        return None;
    }

    let mut result = trade.gives.create();
    let mut additional_cost = 0;
    for modifier in trade.given_item_modifier {
        modifier.apply(&mut result, ctx);
        if result.is_empty() {
            return None;
        }
    }

    // Vanilla `ItemStack.remove` hands back the component the modifier set.
    if let Some(additional_trade_cost) = result.get(ADDITIONAL_TRADE_COST).copied() {
        result.remove(ADDITIONAL_TRADE_COST);
        additional_cost += additional_trade_cost;
    }

    if !trade.double_trade_price_enchantments.is_empty()
        && let Some(enchantments) = result.get(STORED_ENCHANTMENTS)
        && enchantments.iter().any(|(enchantment, _)| {
            trade
                .double_trade_price_enchantments
                .iter()
                .any(|double| double.key == *enchantment)
        })
    {
        additional_cost *= 2;
    }

    let item_cost = to_item_cost(&trade.wants, ctx, additional_cost);
    if item_cost.count() < 1 {
        return None;
    }

    let additional_item_cost = trade
        .additional_wants
        .as_ref()
        .map(|additional_wants| to_item_cost(additional_wants, ctx, 0));
    if additional_item_cost
        .as_ref()
        .is_some_and(|cost| cost.count() < 1)
    {
        return None;
    }

    Some(MerchantOffer::with_second_cost(
        item_cost,
        additional_item_cost,
        result,
        trade.max_uses.get_int(ctx.rng).max(1),
        trade.xp.get_int(ctx.rng).max(0),
        trade.reputation_discount.get_float(ctx).max(0.0),
    ))
}

/// Vanilla `TradeCost.toItemCost`.
fn to_item_cost<R: rand::Rng>(
    cost: &TradeCost,
    ctx: &mut LootContext<'_, R>,
    additional_cost: i32,
) -> ItemCost {
    let count = (cost.count.get_int(ctx.rng) + additional_cost)
        .clamp(0, ItemStack::new(cost.item).max_stack_size());
    ItemCost::new(cost.item, count, cost.components().clone())
}

/// Vanilla `AbstractVillager.addOffersFromTradeSet` for an already-built loot context.
pub fn add_offers_from_trade_set<R: rand::Rng>(
    ctx: &mut LootContext<'_, R>,
    offers: &mut MerchantOffers,
    trade_set: &TradeSet,
    villager_type: Option<VillagerTypeRef>,
) {
    let number_of_offers = trade_set.amount.get_int(ctx.rng);
    if trade_set.allow_duplicates {
        add_offers_from_item_listings(
            ctx,
            offers,
            trade_set.trades,
            number_of_offers,
            villager_type,
        );
    } else {
        add_offers_from_item_listings_without_duplicates(
            ctx,
            offers,
            trade_set.trades,
            number_of_offers,
            villager_type,
        );
    }
}

/// Vanilla `AbstractVillager.addOffersFromItemListings` (duplicates allowed).
pub fn add_offers_from_item_listings<R: rand::Rng>(
    ctx: &mut LootContext<'_, R>,
    offers: &mut MerchantOffers,
    potential_offers: &[&'static VillagerTrade],
    number_of_offers: i32,
    villager_type: Option<VillagerTypeRef>,
) {
    let mut remaining = potential_offers.to_vec();
    let mut offers_found = 0;

    while offers_found < number_of_offers && !remaining.is_empty() {
        let roll = ctx.rng.random_range(0..remaining.len());
        match create_offer(remaining[roll], ctx, villager_type) {
            Some(offer) => {
                offers.push(offer);
                offers_found += 1;
            }
            None => {
                remaining.remove(roll);
            }
        }
    }
}

/// Vanilla `AbstractVillager.addOffersFromItemListingsWithoutDuplicates`.
pub fn add_offers_from_item_listings_without_duplicates<R: rand::Rng>(
    ctx: &mut LootContext<'_, R>,
    offers: &mut MerchantOffers,
    potential_offers: &[&'static VillagerTrade],
    number_of_offers: i32,
    villager_type: Option<VillagerTypeRef>,
) {
    let mut remaining = potential_offers.to_vec();
    let mut offers_found = 0;

    while offers_found < number_of_offers && !remaining.is_empty() {
        let index = ctx.rng.random_range(0..remaining.len());
        let trade = remaining.remove(index);
        if let Some(offer) = create_offer(trade, ctx, villager_type) {
            offers.push(offer);
            offers_found += 1;
        }
    }
}

/// Creates the RNG used to generate a trade set's offers.
///
/// Vanilla draws from the world's persistent `RandomSequences` source named by
/// `TradeSet::random_sequence` (seeded with the world seed). Steel has no
/// `RandomSequences` saved data yet, so the sequence identifier is folded into the
/// world seed instead; this keeps trade generation reproducible per world and trade
/// set rather than drawing from an entropy source.
#[must_use]
pub fn trade_set_rng(world_seed: i64, random_sequence: Option<&Identifier>) -> StdRng {
    let sequence = random_sequence.map_or(0, hash_identifier);
    StdRng::seed_from_u64((world_seed as u64) ^ sequence)
}

/// Vanilla hashes the sequence identifier with MD5 (`RandomSupport.seedFromHashOf`);
/// this is a dependency-free stand-in with the same role.
fn hash_identifier(identifier: &Identifier) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in identifier
        .namespace
        .as_bytes()
        .iter()
        .copied()
        .chain([0])
        .chain(identifier.path.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}
