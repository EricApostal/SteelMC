//! Reputation-driven trade price adjustments.

/// Reputation term of vanilla `Villager.updateSpecialPrices` (Villager.java:417-420):
/// `-Mth.floor(reputation * offer.getPriceMultiplier())`.
///
/// Returns the value to add to a `MerchantOffer`'s `specialPriceDiff`; pass the offer's
/// `priceMultiplier` (the trade's `reputation_discount` provider).
#[must_use]
pub fn reputation_price_discount(reputation: i32, price_multiplier: f32) -> i32 {
    -((reputation as f32 * price_multiplier).floor() as i32)
}

/// Hero of the Village modifier of vanilla `Villager.updateSpecialPrices` (Villager.java:414-415):
/// `heroOfTheVillage == null ? 0.0 : 0.3F + 0.0625F * heroOfTheVillage.getAmplifier()`.
///
/// `amplifier` is the vanilla amplifier (0 for level I); the whole expression is evaluated in
/// `f32` in vanilla before being widened to `f64`.
#[must_use]
pub fn hero_of_the_village_modifier(amplifier: Option<i32>) -> f64 {
    match amplifier {
        None => 0.0,
        Some(amplifier) => f64::from(0.3_f32 + 0.0625_f32 * amplifier as f32),
    }
}

/// Hero of the Village term of vanilla `Villager.updateSpecialPrices` (Villager.java:422-425):
/// `-Math.max((int)Math.floor(heroModifier * offer.getBaseCostA().getCount()), 1)`.
///
/// Returns the value to add to a `MerchantOffer`'s `specialPriceDiff`.
#[must_use]
pub fn hero_of_the_village_price_discount(hero_modifier: f64, base_cost_a_count: i32) -> i32 {
    let cost_reduction = (hero_modifier * f64::from(base_cost_a_count)).floor() as i32;
    -cost_reduction.max(1)
}

/// The full price adjustment vanilla applies per offer in `Villager.updateSpecialPrices`
/// (Villager.java:410-427), on top of a freshly reset `specialPriceDiff`.
///
/// Wave 2 call sites: `Villager::update_special_prices` after `resetSpecialPrices()`, invoked from
/// `start_trading`, `restock`, `set_offers`, `post_data_manipulated`, `on_reputation_event_from`
/// and `gossip` (when a transfer happened while trading). Each offer then adds the result via
/// `MerchantOffer::add_to_special_price_diff`; `MerchantOffer::get_modified_cost_count`
/// (MerchantOffer.java:123-126) clamps the final price to `1..max_stack_size`.
#[must_use]
pub fn special_price_diff(
    reputation: i32,
    price_multiplier: f32,
    hero_modifier: f64,
    base_cost_a_count: i32,
) -> i32 {
    let mut diff = 0;
    if reputation != 0 {
        diff += reputation_price_discount(reputation, price_multiplier);
    }
    if hero_modifier > 0.0 {
        diff += hero_of_the_village_price_discount(hero_modifier, base_cost_a_count);
    }
    diff
}
