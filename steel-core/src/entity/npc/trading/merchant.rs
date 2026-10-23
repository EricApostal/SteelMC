//! Vanilla `Merchant`: the interface of an entity that trades.

use parking_lot::MutexGuard;
use steel_registry::item_stack::ItemStack;

use crate::entity::SharedEntity;
use crate::entity::npc::trading::merchant_offer::{MerchantOffer, MerchantOffers};
use crate::player::Player;

/// Locked view of a merchant's offers.
pub type MerchantOffersGuard<'a> = MutexGuard<'a, MerchantOffers>;

/// Vanilla `Merchant`.
///
/// Adapted to Steel: the offers live in the implementation behind a [`SyncMutex`],
/// `Player` is Steel's player type instead of an entity reference, and vanilla's
/// menu/protocol members (`openTradingScreen`, `getNotifyTradeSound`) stay with
/// `MerchantMenu`. `isClientSide` has no server-side meaning here.
///
/// Implementations lazily initialize the offer list, matching vanilla
/// `AbstractVillager.getOffers`.
///
/// [`SyncMutex`]: steel_utils::locks::SyncMutex
pub trait Merchant: Send + Sync {
    /// Records the player currently trading with this merchant.
    fn set_trading_player(&self, player: Option<SharedEntity>);

    /// The player currently trading with this merchant, if any.
    fn get_trading_player(&self) -> Option<SharedEntity>;

    /// Locks and returns this merchant's offers.
    fn offers(&self) -> MerchantOffersGuard<'_>;

    /// Replaces the offer list, vanilla `Merchant.overrideOffers`.
    fn override_offers(&self, offers: MerchantOffers);

    /// Applies a completed trade, vanilla `Merchant.notifyTrade`.
    fn notify_trade(&self, offer: &mut MerchantOffer);

    /// Notifies the merchant that an offer's output changed, vanilla `Merchant.notifyTradeUpdated`.
    fn notify_trade_updated(&self, stack: &ItemStack);

    /// The merchant's current villager experience, vanilla `Merchant.getVillagerXp`.
    fn villager_xp(&self) -> i32;

    /// Overrides the merchant's villager experience, vanilla `Merchant.overrideXp`.
    fn override_xp(&self, xp: i32);

    /// Whether the villager trade progress bar is shown, vanilla `Merchant.showProgressBar`.
    fn show_progress_bar(&self) -> bool;

    /// Whether the merchant can restock, vanilla `Merchant.canRestock`.
    fn can_restock(&self) -> bool {
        false
    }

    /// Whether the merchant is still valid for the given player, vanilla `Merchant.stillValid`.
    fn still_valid(&self, player: &Player) -> bool;
}
