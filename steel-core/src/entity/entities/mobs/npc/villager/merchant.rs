//! Vanilla `Merchant` state of a villager and the trading-menu adapter.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::trade::TradeSet;
use steel_registry::villager_type::VillagerTypeRef;
use steel_utils::Downcast as _;
use steel_utils::locks::{Shared, SyncMutex};

use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::entity::npc::trading::offer;
use crate::entity::npc::trading::{Merchant, MerchantOffer, MerchantOffers, MerchantOffersGuard};
use crate::entity::{Entity, SharedEntity};
use crate::player::Player;
use crate::world::World;

/// Vanilla `AbstractVillager` merchant fields (offers + trading player).
///
/// The offer list sits behind its own mutex because [`Merchant::offers`] hands out a
/// guard, and the state is shared with [`VillagerMerchantHandle`]: a menu needs an owned
/// trader handle, and `Arc<dyn Entity>` cannot be upcast to `Arc<dyn Merchant>`.
pub struct VillagerMerchantState {
    offers: Shared<MerchantOffers>,
    /// Vanilla `AbstractVillager.getOffers` builds the list lazily on first use.
    offers_initialized: AtomicBool,
    trading_player: SyncMutex<Option<SharedEntity>>,
}

impl Default for VillagerMerchantState {
    fn default() -> Self {
        Self {
            offers: Arc::new(SyncMutex::new(MerchantOffers::new())),
            offers_initialized: AtomicBool::new(false),
            trading_player: SyncMutex::new(None),
        }
    }
}

impl VillagerMerchantState {
    /// Locks the offer list.
    #[must_use]
    pub fn offers(&self) -> MerchantOffersGuard<'_> {
        self.offers.lock()
    }

    /// Whether vanilla would already have built the offer list.
    #[must_use]
    pub fn offers_initialized(&self) -> bool {
        self.offers_initialized.load(Ordering::Acquire)
    }

    /// Marks the offer list as built.
    pub fn mark_offers_initialized(&self) {
        self.offers_initialized.store(true, Ordering::Release);
    }

    /// Replaces the offer list, vanilla `Villager.setOffers`.
    pub fn set_offers(&self, offers: MerchantOffers) {
        *self.offers.lock() = offers;
        self.mark_offers_initialized();
    }

    /// Drops the offer list, vanilla `Villager.setVillagerData` on a profession change.
    pub fn reset_offers(&self) {
        self.offers.lock().clear();
        self.offers_initialized.store(false, Ordering::Release);
    }

    /// Vanilla `AbstractVillager.setTradingPlayer`.
    pub fn set_trading_player(&self, player: Option<SharedEntity>) {
        *self.trading_player.lock() = player;
    }

    /// Vanilla `AbstractVillager.getTradingPlayer`.
    #[must_use]
    pub fn trading_player(&self) -> Option<SharedEntity> {
        self.trading_player.lock().clone()
    }

    /// Vanilla `AbstractVillager.isTrading`.
    #[must_use]
    pub fn is_trading(&self) -> bool {
        self.trading_player.lock().is_some()
    }
}

/// Vanilla `AbstractVillager.addOffersFromTradeSet` for one level's trade set.
///
/// Vanilla draws from the world's persistent `RandomSequences` entry named by the trade
/// set; Steel folds that identifier into the world seed (see [`offer::trade_set_rng`]).
pub(super) fn add_offers_from_trade_set(
    world: &World,
    entity: &dyn Entity,
    trade_set: &TradeSet,
    villager_type: Option<VillagerTypeRef>,
    offers: &mut MerchantOffers,
) {
    let mut rng = offer::trade_set_rng(world.seed(), trade_set.random_sequence.as_ref());
    let position = entity.position();
    let equipment = crate::entity::loot_equipment(entity);
    let mut context = LootContext::new(&mut rng)
        .with_origin(position.x, position.y, position.z)
        .with_game_time(world.game_time())
        .with_this_entity(crate::entity::entity_loot_ref(entity, equipment.as_ref()));
    offer::add_offers_from_trade_set(&mut context, offers, trade_set, villager_type);
}

/// Owning trader handle handed to Steel's merchant menu.
///
/// Vanilla passes `this` to `Merchant::openTradingScreen`; Steel's menu needs an owned
/// `Arc<dyn Merchant>` and `Arc<dyn Entity>` cannot be upcast into another trait object,
/// so a villager wraps its own shared entity here. The offer list and trading player
/// resolve through the shared [`VillagerMerchantState`] even if the wrapped entity is not
/// a villager.
pub struct VillagerMerchantHandle {
    entity: SharedEntity,
    merchant: Arc<VillagerMerchantState>,
}

impl VillagerMerchantHandle {
    /// Wraps a villager's shared entity and merchant state.
    #[must_use]
    pub fn new(entity: SharedEntity, merchant: Arc<VillagerMerchantState>) -> Self {
        Self { entity, merchant }
    }

    fn villager(&self) -> Option<&VillagerEntity> {
        self.entity.as_ref().downcast_ref::<VillagerEntity>()
    }
}

impl Merchant for VillagerMerchantHandle {
    fn set_trading_player(&self, player: Option<SharedEntity>) {
        self.merchant.set_trading_player(player);
    }

    fn get_trading_player(&self) -> Option<SharedEntity> {
        self.merchant.trading_player()
    }

    fn offers(&self) -> MerchantOffersGuard<'_> {
        if let Some(villager) = self.villager() {
            villager.ensure_offers_initialized();
        }

        self.merchant.offers()
    }

    fn override_offers(&self, offers: MerchantOffers) {
        self.merchant.set_offers(offers);
    }

    fn notify_trade(&self, offer: &mut MerchantOffer) {
        if let Some(villager) = self.villager() {
            villager.notify_trade(offer);
        }
    }

    fn notify_trade_updated(&self, stack: &ItemStack) {
        if let Some(villager) = self.villager() {
            villager.notify_trade_updated(stack);
        }
    }

    fn villager_xp(&self) -> i32 {
        self.villager().map_or(0, VillagerEntity::villager_xp)
    }

    fn override_xp(&self, xp: i32) {
        if let Some(villager) = self.villager() {
            villager.override_villager_xp(xp);
        }
    }

    fn show_progress_bar(&self) -> bool {
        true
    }

    fn can_restock(&self) -> bool {
        true
    }

    fn still_valid(&self, player: &Player) -> bool {
        self.villager()
            .is_some_and(|villager| villager.still_valid_for_trade(player))
    }
}
