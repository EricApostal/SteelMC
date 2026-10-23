//! Trading, ported from vanilla `net.minecraft.world.item.trading`.
//!
//! [`Merchant`], [`MerchantOffer`], [`MerchantOffers`] and [`ItemCost`] mirror the
//! vanilla classes; the [`offer`] module generates offers from the data-driven
//! `VillagerTrade`/`TradeSet` registry entries.

mod item_cost;
mod merchant;
mod merchant_offer;
pub mod offer;

#[cfg(test)]
mod tests;

pub use item_cost::ItemCost;
pub use merchant::{Merchant, MerchantOffersGuard};
pub use merchant_offer::{MerchantOffer, MerchantOffers};
