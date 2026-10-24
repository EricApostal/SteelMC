//! Vanilla `Merchant` implementation backing the wandering trader.
//!
//! Vanilla keeps the merchant state on `AbstractVillager` itself; Steel shares it
//! through an `Arc<TraderState>` so the goals, the merchant menu and the entity can
//! all reach the same state, and the entity can hand the menu an owned
//! `Arc<dyn Merchant>` (it cannot recover an `Arc<Self>` from `&self`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use glam::DVec3;
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::sound_events;
use steel_registry::vanilla_trade_sets;
use steel_utils::BlockPos;
use steel_utils::locks::SyncMutex;

use crate::entity::entities::ExperienceOrbEntity;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::npc::trading::offer::{add_offers_from_trade_set, trade_set_rng};
use crate::entity::npc::trading::{Merchant, MerchantOffer, MerchantOffers, MerchantOffersGuard};
use crate::entity::{Entity, SharedEntity, WeakEntity, entity_loot_ref, living_loot_equipment};
use crate::player::Player;
use crate::world::World;

/// Vanilla `AbstractVillager` inventory size.
const VILLAGER_INVENTORY_SIZE: usize = 8;
/// Vanilla `WanderingTrader.rewardTradeXp`: `3 + random.nextInt(4)`.
const TRADE_XP_BASE: i32 = 3;
/// Vanilla `WanderingTrader.rewardTradeXp`: `random.nextInt(4)`.
const TRADE_XP_RANDOM_BOUND: i32 = 4;

/// Merchant state shared between the trader entity, its goals and the trade menu.
pub(crate) struct TraderState {
    offers: SyncMutex<MerchantOffers>,
    offers_initialized: AtomicBool,
    trading_player: SyncMutex<Option<SharedEntity>>,
    villager_xp: SyncMutex<i32>,
    despawn_delay: AtomicI32,
    wander_target: SyncMutex<Option<BlockPos>>,
    inventory: SyncMutex<InventoryCarrier>,
    entity: SyncMutex<Option<WeakEntity>>,
}

impl TraderState {
    /// Creates the shared state with vanilla `AbstractVillager` defaults.
    pub(crate) fn new() -> Self {
        Self {
            offers: SyncMutex::new(MerchantOffers::new()),
            offers_initialized: AtomicBool::new(false),
            trading_player: SyncMutex::new(None),
            villager_xp: SyncMutex::new(0),
            despawn_delay: AtomicI32::new(0),
            wander_target: SyncMutex::new(None),
            inventory: SyncMutex::new(InventoryCarrier::new(VILLAGER_INVENTORY_SIZE)),
            entity: SyncMutex::new(None),
        }
    }

    /// Retains the entity's own shared handle, captured on the first server tick.
    ///
    /// Vanilla reaches the trader through `this`; Steel only receives the shared
    /// handle in `LivingEntity::server_ai_step`, so it is stored once there.
    pub(crate) fn bind_entity(&self, entity: &SharedEntity) {
        let mut stored = self.entity.lock();
        if stored.is_none() {
            *stored = Some(Arc::downgrade(entity));
        }
    }

    /// Returns the live entity handle, if it has been bound and is still alive.
    fn upgraded(&self) -> Option<SharedEntity> {
        self.entity.lock().as_ref().and_then(WeakEntity::upgrade)
    }

    pub(crate) const fn inventory(&self) -> &SyncMutex<InventoryCarrier> {
        &self.inventory
    }

    pub(crate) fn despawn_delay(&self) -> i32 {
        self.despawn_delay.load(Ordering::Relaxed)
    }

    pub(crate) fn set_despawn_delay(&self, despawn_delay: i32) {
        self.despawn_delay.store(despawn_delay, Ordering::Relaxed);
    }

    /// Vanilla `WanderingTrader.maybeDespawn`: counts down while not trading.
    ///
    /// Returns `true` when the timer expired and the entity must be discarded.
    pub(crate) fn tick_despawn_delay(&self) -> bool {
        if self.despawn_delay.load(Ordering::Relaxed) <= 0 || self.is_trading() {
            return false;
        }

        self.despawn_delay.fetch_sub(1, Ordering::Relaxed) == 1
    }

    pub(crate) fn wander_target(&self) -> Option<BlockPos> {
        *self.wander_target.lock()
    }

    pub(crate) fn set_wander_target(&self, wander_target: Option<BlockPos>) {
        *self.wander_target.lock() = wander_target;
    }

    pub(crate) fn is_trading(&self) -> bool {
        self.trading_player.lock().is_some()
    }

    pub(crate) fn trading_player(&self) -> Option<SharedEntity> {
        self.trading_player.lock().clone()
    }

    pub(crate) fn offers_initialized(&self) -> bool {
        self.offers_initialized.load(Ordering::Acquire)
    }

    /// Vanilla `AbstractVillager.readAdditionalSaveData` stores the saved offer list.
    pub(crate) fn set_offers(&self, offers: MerchantOffers) {
        *self.offers.lock() = offers;
        self.offers_initialized.store(true, Ordering::Release);
    }

    /// Vanilla `AbstractVillager.getOffers`: generates the offers on first use.
    ///
    /// Returns `true` when an offer list is available afterwards.
    pub(crate) fn ensure_offers(&self, entity: &dyn Entity) -> bool {
        if self.offers_initialized.load(Ordering::Acquire) {
            return true;
        }

        let Some(world) = entity.level() else {
            return self.offers_initialized.load(Ordering::Acquire);
        };

        let offers = Self::update_trades(entity, world.as_ref());
        *self.offers.lock() = offers;
        self.offers_initialized.store(true, Ordering::Release);
        true
    }

    /// Vanilla `WanderingTrader.updateTrades`: buying, uncommon, then common.
    fn update_trades(entity: &dyn Entity, world: &World) -> MerchantOffers {
        let mut offers = MerchantOffers::new();
        let position = entity.position();
        let equipment = entity.as_living_entity().map(living_loot_equipment);
        let this_entity = entity_loot_ref(entity, equipment.as_ref());

        for trade_set in [
            &*vanilla_trade_sets::WANDERING_TRADER_BUYING,
            &*vanilla_trade_sets::WANDERING_TRADER_UNCOMMON,
            &*vanilla_trade_sets::WANDERING_TRADER_COMMON,
        ] {
            let mut rng = trade_set_rng(world.seed(), trade_set.random_sequence.as_ref());
            let mut ctx = LootContext::new(&mut rng)
                .with_origin(position.x, position.y, position.z)
                .with_game_time(world.game_time())
                .with_this_entity(this_entity);
            add_offers_from_trade_set(&mut ctx, &mut offers, trade_set, None);
        }

        offers
    }

    /// Vanilla `WanderingTrader.rewardTradeXp`.
    fn reward_trade_xp(entity: &dyn Entity, offer: &MerchantOffer) {
        if !offer.should_reward_exp() {
            return;
        }
        let Some(world) = entity.level() else {
            return;
        };

        let xp = TRADE_XP_BASE + rand::random_range(0..TRADE_XP_RANDOM_BOUND);
        let position = entity.position() + DVec3::new(0.0, 0.5, 0.0);
        ExperienceOrbEntity::award(&world, position, xp);
    }
}

impl Merchant for TraderState {
    fn set_trading_player(&self, player: Option<SharedEntity>) {
        *self.trading_player.lock() = player;
    }

    fn get_trading_player(&self) -> Option<SharedEntity> {
        self.trading_player()
    }

    fn offers(&self) -> MerchantOffersGuard<'_> {
        self.offers.lock()
    }

    /// Vanilla `AbstractVillager.overrideOffers` ignores the new list.
    fn override_offers(&self, _offers: MerchantOffers) {}

    /// Vanilla `AbstractVillager.notifyTrade`.
    fn notify_trade(&self, offer: &mut MerchantOffer) {
        offer.increase_uses();

        let Some(entity) = self.upgraded() else {
            return;
        };
        if let Some(mob) = entity.as_mob() {
            mob.reset_ambient_sound_time();
        }
        Self::reward_trade_xp(&*entity, offer);
    }

    /// Vanilla `AbstractVillager.notifyTradeUpdated`.
    fn notify_trade_updated(&self, stack: &ItemStack) {
        let Some(entity) = self.upgraded() else {
            return;
        };
        let Some(mob) = entity.as_mob() else {
            return;
        };

        let interval = mob.ambient_sound_interval();
        if mob.mob_base().ambient_sound_time() <= -interval + 20 {
            return;
        }

        mob.mob_base().set_ambient_sound_time(-interval);
        let sound: SoundEventRef = if stack.is_empty() {
            &sound_events::ENTITY_WANDERING_TRADER_NO
        } else {
            &sound_events::ENTITY_WANDERING_TRADER_YES
        };
        entity.play_sound(sound, 1.0, 1.0);
    }

    /// Vanilla `AbstractVillager.getVillagerXp` returns 0.
    fn villager_xp(&self) -> i32 {
        *self.villager_xp.lock()
    }

    /// Vanilla `AbstractVillager.overrideXp` ignores the new value.
    fn override_xp(&self, _xp: i32) {}

    /// Vanilla `WanderingTrader.showProgressBar` is `false`.
    fn show_progress_bar(&self) -> bool {
        false
    }

    /// Vanilla `AbstractVillager.stillValid`.
    fn still_valid(&self, player: &Player) -> bool {
        let Some(entity) = self.upgraded() else {
            return false;
        };

        let is_trading_player = self
            .trading_player
            .lock()
            .as_ref()
            .is_some_and(|trading_player| trading_player.id() == player.id());

        is_trading_player
            && entity.is_alive()
            && player.is_within_entity_interaction_range(
                entity.bounding_box(),
                super::TRADING_INTERACTION_RANGE,
            )
    }
}
