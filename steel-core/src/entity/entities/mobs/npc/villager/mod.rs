//! Vanilla villager (`net.minecraft.world.entity.npc.villager.Villager`).
//!
//! Vanilla splits this entity across `AbstractVillager` (inventory, merchant state,
//! unhappy counter, trading plumbing) and `Villager` (data, food, gossip, levels,
//! restocking). Steel keeps both halves on one entity because there is no
//! `AbstractVillager` subclass hierarchy to share them with yet; the wandering trader
//! carries its own copy of the shared `AbstractVillager` pieces.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::{CTakeItemEntity, SoundSource};
use steel_registry::data_components::vanilla_components::VILLAGER_FOOD;
use steel_registry::entity_data::VillagerData;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::VillagerEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::villager_profession::VillagerProfessionRef;
use steel_registry::villager_type::VillagerTypeRef;
use steel_registry::{
    REGISTRY, RegistryExt, TaggedRegistryExt, sound_events, vanilla_custom_stats, vanilla_entities,
    vanilla_items, vanilla_mob_effects, vanilla_timelines,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{ChunkPos, Downcast as _, DowncastType, DowncastTypeKey};
use text_components::TextComponent;
use text_components::translation::TranslatedMessage;
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::ai::behavior::poi::{self, PoiMemory};
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::npc::villager::data::VillagerDataExt;
use crate::entity::entities::mobs::npc::villager::merchant::{
    VillagerMerchantState, add_offers_from_trade_set,
};
use crate::entity::entities::objects::items::{ExperienceOrbEntity, ItemEntity};
use crate::entity::npc::gossip::container::{
    GOSSIP_COOLDOWN, GOSSIP_DECAY_INTERVAL, MAX_GOSSIP_TOPICS,
};
use crate::entity::npc::gossip::discount;
use crate::entity::npc::gossip::{
    GossipContainer, GossipType, ReputationEventHandler, ReputationEventType,
};
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::npc::npc::Npc;
use crate::entity::npc::trading::{Merchant, MerchantOffer, MerchantOffers, MerchantOffersGuard};
use crate::entity::{
    AgeableMob, AgeableMobBase, AgeableMobGroupData, Entity, EntityBase, EntityBaseLoad,
    EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase,
    LivingEntitySyncedData, Mob, MobBase, MobEffectInstance, PathfinderMob, RemovalReason,
    SharedEntity, SpawnGroupData,
};
use crate::inventory::container::Container;
use crate::player::Player;
use crate::world::World;
use crate::world::village::golem;

mod brain;
mod data;
mod merchant;
#[cfg(test)]
mod tests;

pub use brain::{make_brain, refresh_brain};
pub use data::VillagerDataHolder;
pub use merchant::VillagerMerchantHandle;

/// Vanilla `Villager.BREEDING_FOOD_THRESHOLD`.
pub const BREEDING_FOOD_THRESHOLD: i32 = 12;
/// Vanilla `Villager.hasExcessFood` threshold (`2 * BREEDING_FOOD_THRESHOLD`).
pub const EXCESS_FOOD_THRESHOLD: i32 = 24;
/// Vanilla `Villager.SPEED_MODIFIER`.
pub const SPEED_MODIFIER: f32 = 0.5;
/// Villagers a summoning consensus needs before [`VillagerEntity::gossip`] spawns a golem.
const VILLAGERS_NEEDED_FOR_GOSSIP_GOLEM: usize = 5;
/// Vanilla `AbstractVillager` baby dimensions before entity scaling.
const BABY_WIDTH: f32 = 0.49;
const BABY_HEIGHT: f32 = 0.98;
const BABY_EYE_HEIGHT: f32 = 0.63;

/// Vanilla villager: a village NPC that trades, farms, gossips and summons golems.
#[entity_behavior(class = "Villager")]
pub struct VillagerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    entity_data: SyncMutex<VillagerEntityData>,
    brain: SyncMutex<Brain>,
    random: SyncMutex<LegacyRandom>,
    merchant: Arc<VillagerMerchantState>,
    inventory: SyncMutex<InventoryCarrier>,
    gossips: SyncMutex<GossipContainer>,
    food_level: SyncMutex<i32>,
    villager_xp: SyncMutex<i32>,
    last_gossip_time: SyncMutex<i64>,
    last_gossip_decay_time: SyncMutex<i64>,
    last_restock_game_time: SyncMutex<i64>,
    number_of_restocks_today: SyncMutex<i32>,
    last_restock_check_day: SyncMutex<i64>,
    last_traded_player: SyncMutex<Option<SharedEntity>>,
    /// Set by [`VillagerEntity::set_villager_data`] on a profession change and applied at
    /// the end of the brain tick: the caller is a behavior that already holds the brain.
    brain_refresh_requested: AtomicBool,
    /// Pending `Villager.spawnGolemIfNeeded` request, resolved after the brain tick.
    pending_golem_spawn: SyncMutex<Option<usize>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `VillagerEntity`.
unsafe impl DowncastType for VillagerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/villager");
}

impl VillagerEntity {
    /// Creates a new villager at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a villager from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        let ageable_base = AgeableMobBase::new();
        let mut entity_data = VillagerEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data
            .villager_data
            .set(data::create_default_villager_data());

        let villager = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(Brain::default()),
            random: SyncMutex::new(LegacyRandom::from_seed(rand::random())),
            merchant: Arc::new(VillagerMerchantState::default()),
            inventory: SyncMutex::new(InventoryCarrier::new(8)),
            gossips: SyncMutex::new(GossipContainer::new()),
            food_level: SyncMutex::new(0),
            villager_xp: SyncMutex::new(0),
            last_gossip_time: SyncMutex::new(0),
            last_gossip_decay_time: SyncMutex::new(0),
            last_restock_game_time: SyncMutex::new(0),
            number_of_restocks_today: SyncMutex::new(0),
            last_restock_check_day: SyncMutex::new(0),
            last_traded_player: SyncMutex::new(None),
            brain_refresh_requested: AtomicBool::new(false),
            pending_golem_spawn: SyncMutex::new(None),
        };

        // Vanilla `AbstractVillager`/`Villager` constructor state.
        villager.set_pathfinding_malus(PathType::FireInNeighbor, 16.0);
        villager.set_pathfinding_malus(PathType::Fire, -1.0);
        villager.set_can_pick_up_loot(true);
        {
            let follow_range = villager
                .attributes()
                .lock()
                .get_value(steel_registry::vanilla_attributes::FOLLOW_RANGE)
                .unwrap_or(16.0);
            let mut navigation = villager.mob_base.navigation().lock();
            navigation.set_can_open_doors(true);
            navigation.set_can_float(true);
            navigation.set_required_path_length(48.0, follow_range);
        }

        *villager.brain.lock() = brain::make_brain(&villager);
        villager
    }

    // --- villager data -------------------------------------------------------------------

    /// Vanilla `Villager.getVillagerData().profession().value()`.
    #[must_use]
    pub fn profession(&self) -> VillagerProfessionRef {
        data::profession_from_index(self.villager_data().profession)
    }

    /// Vanilla `Villager.getVillagerData().type().value()`.
    #[must_use]
    pub fn villager_type(&self) -> VillagerTypeRef {
        data::villager_type_from_index(self.villager_data().villager_type)
    }

    /// Vanilla `Villager.getVillagerData().level()`.
    #[must_use]
    pub fn villager_level(&self) -> i32 {
        self.villager_data().level
    }

    /// Vanilla `Villager.refreshBrain`: rebuilds the behavior packages for the current
    /// villager data.
    ///
    /// Vanilla's `refreshBrain` also runs `stopAll` on the old brain; that needs the
    /// entity's `SharedEntity`, so it stays with the tick-side request path that
    /// [`VillagerEntity::set_villager_data`] schedules.
    pub fn refresh_brain(&self) {
        brain::refresh_brain(&mut self.brain.lock(), self);
    }

    fn request_brain_refresh(&self) {
        self.brain_refresh_requested.store(true, Ordering::Release);
    }

    fn take_brain_refresh_request(&self) -> bool {
        self.brain_refresh_requested.swap(false, Ordering::AcqRel)
    }

    /// Vanilla `Villager.createDefaultVillagerData`.
    #[must_use]
    pub fn create_default_villager_data() -> VillagerData {
        data::create_default_villager_data()
    }

    // --- merchant ------------------------------------------------------------------------

    /// Vanilla `AbstractVillager.getOffers`, which builds the list on first use.
    pub fn ensure_offers_initialized(&self) {
        if self.merchant.offers_initialized() {
            return;
        }

        self.merchant.mark_offers_initialized();
        self.update_trades();
    }

    /// Vanilla `Villager.updateTrades`: append the offers of the profession's trade set
    /// for the villager's current level.
    fn update_trades(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let level = self.villager_level();
        let Some((_, trade_set)) = self
            .profession()
            .trade_sets_by_level
            .iter()
            .find(|(set_level, _)| *set_level == level)
        else {
            return;
        };

        let mut offers = MerchantOffers::new();
        add_offers_from_trade_set(
            &world,
            self,
            trade_set,
            Some(self.villager_type()),
            &mut offers,
        );
        self.merchant.offers().extend(offers);
        self.update_special_prices_for_trading_player();
    }

    /// An owning `Merchant` handle for Steel's trading menu.
    #[must_use]
    pub fn merchant_handle(self: &Arc<Self>) -> Option<VillagerMerchantHandle> {
        let world = self.level()?;
        let entity = world.entity_manager().get_by_id(self.id())?;
        Some(VillagerMerchantHandle::new(
            entity,
            Arc::clone(&self.merchant),
        ))
    }

    /// Vanilla `Villager.restock`.
    pub fn restock(&self) {
        self.update_demand();
        {
            let mut offers = self.merchant.offers();
            for offer in offers.iter_mut() {
                offer.reset_uses();
            }
        }

        self.update_special_prices_for_trading_player();
        if let Some(world) = self.level() {
            *self.last_restock_game_time.lock() = world.game_time();
        }
        *self.number_of_restocks_today.lock() += 1;
    }

    /// Vanilla `Villager.needsToRestock`.
    #[must_use]
    pub fn needs_to_restock(&self) -> bool {
        self.merchant
            .offers()
            .iter()
            .any(MerchantOffer::needs_restock)
    }

    /// Vanilla `Villager.allowedToRestock`.
    fn allowed_to_restock(&self) -> bool {
        let restocks = *self.number_of_restocks_today.lock();
        let game_time = self.level().map_or(0, |world| world.game_time());
        restocks == 0 || (restocks < 2 && game_time > *self.last_restock_game_time.lock() + 2400)
    }

    /// Vanilla `Villager.shouldRestock`.
    #[must_use]
    pub fn should_restock(&self) -> bool {
        let game_time = self.level().map_or(0, |world| world.game_time());
        let half_day_passed = *self.last_restock_game_time.lock() + 12_000;
        let mut is_new_day = game_time > half_day_passed;

        let current_day = self.current_day();
        if *self.last_restock_check_day.lock() > 0
            && current_day > *self.last_restock_check_day.lock()
        {
            is_new_day = true;
        }
        *self.last_restock_check_day.lock() = current_day;

        if is_new_day {
            *self.last_restock_game_time.lock() = game_time;
            self.reset_number_of_restocks();
        }

        self.allowed_to_restock() && self.needs_to_restock()
    }

    /// Vanilla `Timelines.OVERWORLD_DAY` period count.
    fn current_day(&self) -> i64 {
        let Some(world) = self.level() else {
            return 0;
        };
        let Some(timeline) = REGISTRY.timelines.by_key(&vanilla_timelines::DAY.key) else {
            return 0;
        };
        i64::from(
            timeline.period_count(world.clock_total_ticks(timeline.clock).unwrap_or_default()),
        )
    }

    /// Vanilla `Villager.resetNumberOfRestocks`.
    fn reset_number_of_restocks(&self) {
        self.catch_up_demand();
        *self.number_of_restocks_today.lock() = 0;
    }

    /// Vanilla `Villager.catchUpDemand`.
    fn catch_up_demand(&self) {
        let missed_updates = 2 - *self.number_of_restocks_today.lock();
        if missed_updates > 0 {
            let mut offers = self.merchant.offers();
            for offer in offers.iter_mut() {
                offer.reset_uses();
            }
        }

        for _ in 0..missed_updates.max(0) {
            self.update_demand();
        }

        self.update_special_prices_for_trading_player();
    }

    /// Vanilla `Villager.updateDemand`.
    fn update_demand(&self) {
        let mut offers = self.merchant.offers();
        for offer in offers.iter_mut() {
            offer.update_demand();
        }
    }

    /// Vanilla `Villager.resetSpecialPrices`.
    fn reset_special_prices(&self) {
        let mut offers = self.merchant.offers();
        for offer in offers.iter_mut() {
            offer.reset_special_price_diff();
        }
    }

    /// Vanilla `Villager.updateSpecialPrices`.
    pub fn update_special_prices(&self, player: &Player) {
        self.reset_special_prices();

        let reputation = self.player_reputation(player.uuid());
        let hero_modifier = discount::hero_of_the_village_modifier(
            LivingEntity::mob_effect(player, vanilla_mob_effects::HERO_OF_THE_VILLAGE)
                .map(|effect| effect.amplifier()),
        );

        {
            let mut offers = self.merchant.offers();
            for offer in offers.iter_mut() {
                let diff = discount::special_price_diff(
                    reputation,
                    offer.price_multiplier(),
                    hero_modifier,
                    offer.base_cost_a().count(),
                );
                offer.add_to_special_price_diff(diff);
            }
        }

        if self.merchant.trading_player().is_some() {
            self.refresh_trade_menu(player);
        }
    }

    /// Vanilla `Villager.updateSpecialPrices` tail: refresh the open trade screen
    /// (`menu.updateSellItem()` + re-sending the offers with the open menu's container id).
    ///
    /// Vanilla's `updateSellItem` recompute is the menu's on-open/take hook in Steel, so
    /// only the packet re-send is needed here.
    fn refresh_trade_menu(&self, player: &Player) {
        let Some(container_id) = player.trading_container_id() else {
            return;
        };
        let offers = self.merchant.offers();
        player.send_merchant_offers(
            i32::from(container_id),
            &offers,
            self.villager_level(),
            self.villager_xp(),
            self.show_progress_bar(),
            self.can_restock(),
        );
    }

    fn update_special_prices_for_trading_player(&self) {
        let Some(trading_player) = self.merchant.trading_player() else {
            return;
        };
        if let Some(player) = trading_player.as_ref().downcast_ref::<Player>() {
            self.update_special_prices(player);
        }
    }

    /// Vanilla `AbstractVillager.notifyTrade`.
    pub fn notify_trade(&self, offer: &mut MerchantOffer) {
        offer.increase_uses();
        self.mob_base
            .set_ambient_sound_time(-self.ambient_sound_interval());
        self.reward_trade_xp(offer);
    }

    /// Vanilla `AbstractVillager.notifyTradeUpdated`.
    pub fn notify_trade_updated(&self, stack: &ItemStack) {
        if self.mob_base.ambient_sound_time() > -self.ambient_sound_interval() + 20 {
            self.mob_base
                .set_ambient_sound_time(-self.ambient_sound_interval());
            self.make_sound(Some(self.trade_updated_sound(!stack.is_empty())));
        }
    }

    /// Vanilla `AbstractVillager.getTradeUpdatedSound`.
    #[must_use]
    pub fn trade_updated_sound(&self, valid_trade: bool) -> SoundEventRef {
        if valid_trade {
            &sound_events::ENTITY_VILLAGER_YES
        } else {
            &sound_events::ENTITY_VILLAGER_NO
        }
    }

    /// Vanilla `Villager.rewardTradeXp`.
    fn reward_trade_xp(&self, offer: &MerchantOffer) {
        let mut pop_xp = 3 + self.random.lock().next_i32_bounded(4);
        self.add_villager_xp(offer.xp());
        *self.last_traded_player.lock() = self.merchant.trading_player();
        if self.should_increase_level()
            && let Some(world) = self.level()
        {
            self.increase_merchant_career(&world);
            self.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::REGENERATION,
                200,
                0,
            ));
            pop_xp += 5;
        }

        if offer.should_reward_exp()
            && let Some(world) = self.level()
        {
            ExperienceOrbEntity::award(&world, self.position() + DVec3::new(0.0, 0.5, 0.0), pop_xp);
        }
    }

    /// Vanilla `Villager.shouldIncreaseLevel`.
    #[must_use]
    pub fn should_increase_level(&self) -> bool {
        let level = self.villager_level();
        data::can_level_up(level) && self.villager_xp() >= data::max_xp_per_level(level)
    }

    /// Vanilla `Villager.increaseMerchantCareer`.
    fn increase_merchant_career(&self, _world: &Arc<World>) {
        let data = self.villager_data().with_level(self.villager_level() + 1);
        // Vanilla `increaseMerchantCareer` keeps the existing offers and appends the new
        // level's; `set_villager_data` must not drop them, so bump the level directly.
        self.set_villager_data_keeping_offers(data);
        if let Some(world) = self.level() {
            self.update_trades();
            let _ = world;
        }
    }

    /// Vanilla `Villager.getVillagerXp`.
    #[must_use]
    pub fn merchant_xp(&self) -> i32 {
        *self.villager_xp.lock()
    }

    /// Vanilla `Villager.getVillagerXp`, short for `Merchant::villager_xp`.
    #[must_use]
    pub fn villager_xp(&self) -> i32 {
        self.merchant_xp()
    }

    /// Vanilla `Villager.setVillagerXp`.
    pub fn set_villager_xp(&self, value: i32) {
        *self.villager_xp.lock() = value;
    }

    fn add_villager_xp(&self, value: i32) {
        *self.villager_xp.lock() += value;
    }

    /// Vanilla `AbstractVillager.overrideXp`: a no-op for villagers.
    ///
    /// `MerchantMenu` calls it after a trade, but `Villager.rewardTradeXp` already added
    /// the offer's xp (vanilla `AbstractVillager.overrideXp` is empty).
    pub fn override_villager_xp(&self, _xp: i32) {}

    // --- trading -------------------------------------------------------------------------

    /// Vanilla `Villager.startTrading`.
    pub fn start_trading(&self, player: &Player) {
        self.update_special_prices(player);

        let Some(world) = self.level() else {
            return;
        };
        let Some(entity) = world.entity_manager().get_by_id(self.id()) else {
            return;
        };
        self.set_trading_player(Some(Arc::clone(&entity)));

        player.open_trading_screen(
            Arc::new(VillagerMerchantHandle::new(
                entity,
                Arc::clone(&self.merchant),
            )),
            self.profession_name(),
            self.villager_level(),
        );
    }

    /// Vanilla `AbstractVillager.setTradingPlayer` plus `Villager`'s stop-on-clear rule.
    pub fn set_trading_player(&self, player: Option<SharedEntity>) {
        let should_stop = self.merchant.trading_player().is_some() && player.is_none();
        self.merchant.set_trading_player(player);
        if should_stop {
            self.stop_trading();
        }
    }

    /// Vanilla `AbstractVillager.isTrading`.
    #[must_use]
    pub fn is_trading(&self) -> bool {
        self.merchant.is_trading()
    }

    /// Vanilla `AbstractVillager.getTradingPlayer`.
    #[must_use]
    pub fn trading_player(&self) -> Option<SharedEntity> {
        self.merchant.trading_player()
    }

    /// Vanilla `AbstractVillager.stopTrading`.
    pub fn stop_trading(&self) {
        self.set_trading_player(None);
    }

    /// Vanilla `Villager.getDisplayName()` for the trade screen title.
    #[must_use]
    pub fn profession_name(&self) -> TextComponent {
        let key = self.profession().key.clone();
        TextComponent::translated(TranslatedMessage {
            key: format!("entity.{}.villager.{}", key.namespace, key.path).into(),
            fallback: None,
            args: None,
        })
    }

    /// Vanilla `AbstractVillager.stillValid`.
    #[must_use]
    pub fn still_valid_for_trade(&self, player: &Player) -> bool {
        self.merchant
            .trading_player()
            .is_some_and(|trading_player| trading_player.uuid() == player.uuid())
            && Entity::is_alive(self)
            && player.is_within_entity_interaction_range_with_buffer(self.bounding_box(), 4.0)
    }

    // --- food and breeding ---------------------------------------------------------------

    /// Vanilla `Villager.getFoodLevel` equivalent (the vanilla field is private).
    #[must_use]
    pub fn food_level(&self) -> i32 {
        *self.food_level.lock()
    }

    /// Vanilla `Villager.canBreed` food term.
    #[must_use]
    pub fn can_breed_food(&self) -> bool {
        self.food_level() + self.count_food_points_in_inventory() >= BREEDING_FOOD_THRESHOLD
            && !LivingEntity::is_sleeping(self)
            && self.get_age() == 0
    }

    /// Vanilla `Villager.canBreed` food term, caller-shaped alias of [`Self::can_breed_food`].
    #[must_use]
    pub fn has_breeding_food(&self) -> bool {
        self.can_breed_food()
    }

    /// Vanilla `Villager.hungry`.
    #[must_use]
    pub fn hungry(&self) -> bool {
        self.food_level() < BREEDING_FOOD_THRESHOLD
    }

    /// Vanilla `Villager.countFoodPointsInInventory`.
    #[must_use]
    pub fn count_food_points_in_inventory(&self) -> i32 {
        self.with_inventory_mut(|inventory| {
            (0..inventory.size())
                .filter_map(|slot| {
                    let stack = inventory.get_item(slot);
                    stack
                        .get(VILLAGER_FOOD)
                        .map(|food| stack.count() * food.nutrition())
                })
                .sum()
        })
    }

    /// Vanilla `Villager.hasExcessFood`.
    #[must_use]
    pub fn has_excess_food(&self) -> bool {
        self.count_food_points_in_inventory() >= EXCESS_FOOD_THRESHOLD
    }

    /// Vanilla `Villager.wantsMoreFood`.
    #[must_use]
    pub fn wants_more_food(&self) -> bool {
        self.count_food_points_in_inventory() < BREEDING_FOOD_THRESHOLD
    }

    /// Vanilla `Villager.eatUntilFull`.
    fn eat_until_full(&self) {
        if !self.hungry() {
            return;
        }

        self.with_inventory_mut(|inventory| {
            for slot in 0..inventory.size() {
                let Some(nutrition) = inventory
                    .get_item(slot)
                    .get(VILLAGER_FOOD)
                    .map(|food| food.nutrition())
                else {
                    continue;
                };

                let item_count = inventory.get_item(slot).count();
                let mut to_remove = 0;
                for _ in 0..item_count {
                    *self.food_level.lock() += nutrition;
                    to_remove += 1;
                    if !self.hungry() {
                        break;
                    }
                }

                inventory.remove_item(slot, to_remove);
                if !self.hungry() {
                    return;
                }
            }
        });
    }

    /// Vanilla `Villager.eatAndDigestFood`.
    pub fn eat_and_digest_food(&self) {
        self.eat_until_full();
        *self.food_level.lock() -= BREEDING_FOOD_THRESHOLD;
    }

    /// Vanilla `Villager.hasFarmSeeds`.
    #[must_use]
    pub fn has_farm_seeds(&self) -> bool {
        self.with_inventory_mut(|inventory| {
            (0..inventory.size()).any(|slot| {
                REGISTRY.items.is_in_tag(
                    inventory.get_item(slot).item(),
                    &ItemTag::VILLAGER_PLANTABLE_SEEDS,
                )
            })
        })
    }

    /// Vanilla `Villager.getBreedOffspring`.
    #[must_use]
    pub fn get_breed_offspring_for(
        &self,
        world: &Arc<World>,
        partner: &VillagerEntity,
    ) -> Option<SharedEntity> {
        let biome_roll = self.random.lock().next_f64();
        let villager_type = if biome_roll < 0.5 {
            world
                .biome_at(self.block_position())
                .map_or(data::DEFAULT_VILLAGER_TYPE, data::villager_type_by_biome)
        } else if biome_roll < 0.75 {
            self.villager_type()
        } else {
            partner.villager_type()
        };

        let child = crate::entity::registry::ENTITIES.create(
            &vanilla_entities::VILLAGER,
            crate::entity::next_entity_id(),
            self.position(),
            Arc::downgrade(world),
        )?;
        let child_villager = child.as_ref().downcast_ref::<VillagerEntity>()?;
        child_villager.set_villager_data_keeping_offers(
            child_villager
                .villager_data()
                .with_type_ref(villager_type)
                .with_profession_ref(data::none_profession()),
        );
        child_villager.set_villager_data_finalized(true);
        Some(child)
    }

    // --- gossip and reputation -----------------------------------------------------------

    /// Vanilla `Villager.getGossips().copy()`.
    #[must_use]
    pub fn gossips_copy(&self) -> GossipContainer {
        self.gossips.lock().copy()
    }

    /// Vanilla `Villager.setGossips` (`putAll`).
    pub fn set_gossips(&self, gossips: GossipContainer) {
        self.gossips.lock().put_all(&gossips);
    }

    /// Vanilla `Villager.getPlayerReputation`.
    #[must_use]
    pub fn player_reputation(&self, player: Uuid) -> i32 {
        self.gossips.lock().get_reputation(player, |_| true)
    }

    /// Vanilla `Villager.gossip`: exchange gossip with `target`.
    ///
    /// The golem summon vanilla starts from here is deferred to the end of the brain tick
    /// because this runs inside the brain (see [`VillagerEntity::brain_tick`]).
    pub fn gossip(&self, _level: &World, target: &VillagerEntity, timestamp: i64) {
        let own_last_gossip = *self.last_gossip_time.lock();
        let target_last_gossip = *target.last_gossip_time.lock();
        let own_ready =
            timestamp < own_last_gossip || timestamp >= own_last_gossip + GOSSIP_COOLDOWN;
        let target_ready =
            timestamp < target_last_gossip || timestamp >= target_last_gossip + GOSSIP_COOLDOWN;
        if !own_ready || !target_ready {
            return;
        }

        let new_gossip = {
            let mut random = self.random.lock();
            let target_gossips = target.gossips.lock();
            self.gossips
                .lock()
                .transfer_from(&target_gossips, &mut *random, MAX_GOSSIP_TOPICS)
        };
        *self.last_gossip_time.lock() = timestamp;
        *target.last_gossip_time.lock() = timestamp;

        self.request_golem_spawn(VILLAGERS_NEEDED_FOR_GOSSIP_GOLEM);
        if new_gossip > 0 {
            self.update_special_prices_for_trading_player();
        }
    }

    /// Vanilla `Villager.maybeDecayGossip`.
    fn maybe_decay_gossip(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let timestamp = world.game_time();
        let should_decay = {
            let mut last_decay = self.last_gossip_decay_time.lock();
            if *last_decay == 0 {
                *last_decay = timestamp;
                false
            } else if timestamp >= *last_decay + GOSSIP_DECAY_INTERVAL {
                *last_decay = timestamp;
                true
            } else {
                false
            }
        };

        if should_decay {
            self.gossips.lock().decay();
        }
    }

    /// Vanilla `Villager.wantsToSpawnGolem`.
    #[must_use]
    pub fn wants_to_spawn_golem(&self, game_time: i64) -> bool {
        golem::wants_to_spawn_golem(self, game_time)
    }

    /// Vanilla `Villager.spawnGolemIfNeeded`.
    ///
    /// The entity resolves its own level, and vanilla's `timestamp` argument is unused
    /// there (`wantsToSpawnGolem` reads the game time), so callers pass only the required
    /// number of agreeing villagers.
    pub fn spawn_golem_if_needed(&self, villagers_needed_to_agree: usize) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        golem::spawn_golem_if_needed(&world, self, villagers_needed_to_agree).is_some()
    }

    fn request_golem_spawn(&self, villagers_needed_to_agree: usize) {
        *self.pending_golem_spawn.lock() = Some(villagers_needed_to_agree);
    }

    fn take_golem_spawn_request(&self) -> Option<usize> {
        self.pending_golem_spawn.lock().take()
    }

    /// Vanilla `Villager.releaseAllPois`.
    pub fn release_all_pois(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let Some(brain) = self.brain.try_lock() else {
            log::warn!(
                "villager {} released its POIs while its brain was busy ticking",
                self.id()
            );
            return;
        };

        let profession = self.profession();
        for (memory, memory_type) in [
            (PoiMemory::Home, &mem::HOME),
            (PoiMemory::JobSite, &mem::JOB_SITE),
            (PoiMemory::PotentialJobSite, &mem::POTENTIAL_JOB_SITE),
            (PoiMemory::MeetingPoint, &mem::MEETING_POINT),
        ] {
            let Some(global_pos) = brain.get_memory(memory_type) else {
                continue;
            };
            if global_pos.dimension != world.key {
                continue;
            }
            let Some(poi_type) = poi::poi_type_at(&world, global_pos.pos) else {
                continue;
            };
            if memory.predicate(profession).matches(poi_type) {
                world.release_poi(global_pos.pos);
            }
        }
    }

    /// Vanilla `AssignProfessionFromJobSite`: adopt the first registered profession whose
    /// held job site matches `poi_type`.
    pub fn apply_profession_from_poi(&self, poi_type: steel_registry::poi::PoiTypeRef) -> bool {
        let Some(profession) = (0..REGISTRY.villager_professions.len())
            .filter_map(|id| REGISTRY.villager_professions.by_id(id))
            .find(|profession| profession.is_held_job_site(poi_type))
        else {
            return false;
        };

        self.set_villager_data(self.villager_data().with_profession_ref(profession));
        true
    }

    // --- sounds --------------------------------------------------------------------------

    /// Vanilla `Villager.playWorkSound`.
    pub fn play_work_sound(&self) {
        self.make_sound(self.profession().work_sound);
    }

    /// Vanilla `AbstractVillager.playCelebrateSound`.
    pub fn play_celebrate_sound(&self) {
        self.make_sound(Some(&sound_events::ENTITY_VILLAGER_CELEBRATE));
    }

    /// Vanilla `Villager.setUnhappy`.
    pub fn set_unhappy(&self) {
        self.set_unhappy_counter(40);
        self.make_sound(Some(&sound_events::ENTITY_VILLAGER_NO));
    }

    /// Vanilla `AbstractVillager.getUnhappyCounter`.
    #[must_use]
    pub fn unhappy_counter(&self) -> i32 {
        *self
            .entity_data
            .lock()
            .abstract_villager()
            .unhappy_counter
            .get()
    }

    /// Vanilla `AbstractVillager.setUnhappyCounter`.
    pub fn set_unhappy_counter(&self, value: i32) {
        self.entity_data
            .lock()
            .abstract_villager_mut()
            .unhappy_counter
            .set(value);
    }

    // --- inventory -----------------------------------------------------------------------

    /// Runs `f` on this villager's item inventory (vanilla `InventoryCarrier.getInventory`).
    pub fn with_inventory_mut<R>(&self, f: impl FnOnce(&mut InventoryCarrier) -> R) -> R {
        f(&mut self.inventory.lock())
    }

    /// Vanilla `Villager.wantsToPickUp`.
    #[must_use]
    pub fn wants_to_pick_up_item(&self, item_stack: &ItemStack) -> bool {
        let item = item_stack.item();
        let wanted = REGISTRY.items.is_in_tag(item, &ItemTag::VILLAGER_PICKS_UP)
            || item_stack.get(VILLAGER_FOOD).is_some()
            || self.profession().requested_items.contains(&item);
        wanted && self.with_inventory_mut(|inventory| inventory.can_add_item(item_stack))
    }

    fn set_villager_data_keeping_offers(&self, data: VillagerData) {
        self.entity_data.lock().villager_data.set(data);
    }

    /// Vanilla `Villager.postDataManipulated`.
    pub fn post_data_manipulated(&self) {
        self.update_special_prices_for_trading_player();
    }

    // --- brain ---------------------------------------------------------------------------

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }

        let display = self.living_base.mob_effect_display_state();
        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }

        self.entity_data.set_base_invisible_flag(display.invisible);
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }
}

impl VillagerDataHolder for VillagerEntity {
    fn villager_data(&self) -> VillagerData {
        *self.entity_data.lock().villager_data.get()
    }

    /// Vanilla `Villager.setVillagerData`: a profession change drops the offers.
    ///
    /// Vanilla's callers (`AssignProfessionFromJobSite`, `ResetProfession`,
    /// `increaseMerchantCareer`) follow up with `refreshBrain(level)`; Steel schedules that
    /// refresh for the end of the brain tick because the callers are behaviors that
    /// already hold this villager's brain.
    fn set_villager_data(&self, data: VillagerData) {
        if self.profession().key != data::profession_from_index(data.profession).key {
            self.merchant.reset_offers();
            self.request_brain_refresh();
        }

        self.set_villager_data_keeping_offers(data);
    }

    fn villager_data_finalized(&self) -> bool {
        *self.entity_data.lock().villager_data_finalized.get()
    }

    fn set_villager_data_finalized(&self, villager_data_finalized: bool) {
        self.entity_data
            .lock()
            .villager_data_finalized
            .set(villager_data_finalized);
    }
}

impl Npc for VillagerEntity {}

impl Entity for VillagerEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn name(&self) -> TextComponent {
        self.custom_name().unwrap_or_else(|| self.profession_name())
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        let Some(variant) =
            item_stack.get(steel_registry::data_components::vanilla_components::VILLAGER_VARIANT)
        else {
            return;
        };
        self.set_villager_data(self.villager_data().with_type_ref(variant.value()));
        self.set_villager_data_finalized(true);
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        let unhappy_counter = self.unhappy_counter();
        if unhappy_counter > 0 {
            self.set_unhappy_counter(unhappy_counter - 1);
        }
        self.maybe_decay_gossip();
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            // Vanilla `EntityDimensions.scalable(0.49, 0.98).withEyeHeight(0.63)`.
            let adult = self.entity_type.dimensions;
            EntityDimensions::new_with_attachments(
                BABY_WIDTH,
                BABY_HEIGHT,
                BABY_EYE_HEIGHT,
                adult
                    .attachments
                    .scale(BABY_WIDTH / adult.width, BABY_HEIGHT / adult.height),
            )
            .scale(scale)
        } else if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);

        data::write_villager_data(&self.villager_data(), nbt);
        nbt.insert(
            "VillagerDataFinalized",
            i8::from(self.villager_data_finalized()),
        );
        nbt.insert("FoodLevel", self.food_level() as i8);
        self.gossips.lock().save_to(nbt);
        nbt.insert("Xp", self.villager_xp());
        nbt.insert("LastRestock", *self.last_restock_game_time.lock());
        nbt.insert("LastGossipDecay", *self.last_gossip_decay_time.lock());
        nbt.insert("RestocksToday", *self.number_of_restocks_today.lock());
        if self.merchant.offers_initialized() {
            nbt.insert("Offers", self.merchant.offers().to_nbt());
        }
        self.inventory.lock().write_to_tag(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);

        let villager_data = data::read_villager_data(nbt);
        if nbt
            .byte("VillagerDataFinalized")
            .is_some_and(|value| value != 0)
            || villager_data.is_some()
        {
            self.set_villager_data_finalized(true);
            self.set_villager_data_keeping_offers(
                villager_data.unwrap_or_else(data::create_default_villager_data),
            );
        }

        *self.food_level.lock() = i32::from(nbt.byte("FoodLevel").unwrap_or(0));
        self.gossips.lock().load_from(nbt);
        *self.villager_xp.lock() = nbt.int("Xp").unwrap_or(0);
        *self.last_restock_game_time.lock() = nbt.long("LastRestock").unwrap_or(0);
        *self.last_gossip_decay_time.lock() = nbt.long("LastGossipDecay").unwrap_or(0);
        *self.number_of_restocks_today.lock() = nbt.int("RestocksToday").unwrap_or(0);

        if let Some(offers) = nbt
            .compound("Offers")
            .and_then(|offers| MerchantOffers::from_nbt(&offers))
        {
            self.merchant.set_offers(offers);
        }

        self.inventory.lock().read_from_tag(&nbt);
        self.refresh_brain();
    }
}

impl LivingEntity for VillagerEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn living_synced_data(&self) -> Option<&dyn LivingEntitySyncedData> {
        Some(&self.entity_data)
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_VILLAGER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_VILLAGER_DEATH)
    }

    /// Vanilla `Villager.setLastHurtByMob`: hurting a villager costs the attacker reputation.
    fn set_last_hurt_by_mob(&self, target: Option<&SharedEntity>) {
        let Some(hurt_by) = target else {
            LivingEntity::set_last_hurt_mob(self, None);
            return;
        };

        if let Some(world) = self.level() {
            world.on_reputation_event(ReputationEventType::VILLAGER_HURT, hurt_by, self);
            if Entity::is_alive(self) && hurt_by.as_ref().is::<Player>() {
                self.broadcast_entity_event(EntityStatus::VillagerAngry);
            }
        }

        LivingEntity::set_last_hurt_mob(self, Some(hurt_by));
    }

    /// Vanilla `Villager.die`: report the murder to witnesses and release claimed POIs.
    fn on_death(&self, source: &DamageSource) {
        log::info!(
            "Villager {} died, damage type: '{}'",
            self.id(),
            source.damage_type.key
        );

        if let Some(murderer) = source.causing_entity().cloned()
            && let Some(world) = self.level()
        {
            world.report_reputation_event_to_witnesses(
                ReputationEventType::VILLAGER_KILLED,
                &murderer,
                self.block_position(),
                crate::world::village::reputation::WITNESS_SCAN_RADIUS,
            );
        }

        self.release_all_pois();
        self.stop_trading();
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<crate::physics::MoveResult> {
        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        result
    }
}

impl AgeableMob for VillagerEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        &self.ageable_base
    }

    fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().ageable_mob().age_locked.get()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.entity_data
            .lock()
            .ageable_mob_mut()
            .age_locked
            .set(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.entity_data.lock().ageable_mob_mut().baby.set(baby);
    }

    fn age_boundary_changed(&self, _baby: bool) {
        self.refresh_dimensions();
        self.refresh_brain();
    }

    /// Vanilla `Villager.getBreedOffspring`.
    fn get_breed_offspring(
        &self,
        world: &Arc<World>,
        partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        let partner_entity: &dyn Entity = partner;
        let partner_villager = partner_entity.downcast_ref::<VillagerEntity>()?;
        self.get_breed_offspring_for(world, partner_villager)
    }
}

impl PathfinderMob for VillagerEntity {}

impl Mob for VillagerEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        if LivingEntity::is_sleeping(self) {
            return None;
        }

        Some(if self.is_trading() {
            &sound_events::ENTITY_VILLAGER_TRADE
        } else {
            &sound_events::ENTITY_VILLAGER_AMBIENT
        })
    }

    /// Vanilla `AbstractVillager.canBeLeashed`.
    fn mob_can_be_leashed(&self) -> bool {
        false
    }

    /// Vanilla `Villager.removeWhenFarAway`.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        false
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    fn as_reputation_event_handler(&self) -> Option<&dyn ReputationEventHandler> {
        Some(self)
    }

    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        {
            let mut brain = self.brain.lock();
            brain::tick(&mut brain, &world, entity);
            if self.take_brain_refresh_request() {
                brain::refresh_brain(&mut brain, self);
            }
        }

        // Resolved after the brain tick: the consensus helper locks villager brains.
        if let Some(villagers_needed_to_agree) = self.take_golem_spawn_request() {
            self.spawn_golem_if_needed(villagers_needed_to_agree);
        }
    }

    fn custom_server_ai_step(&self) {
        if let Some(player) = self.last_traded_player.lock().take() {
            if let Some(world) = self.level() {
                world.on_reputation_event(ReputationEventType::TRADE, &player, self);
            }
            self.broadcast_entity_event(EntityStatus::VillagerHappy);
        }

        if !self.is_no_ai() && self.random.lock().next_i32_bounded(100) == 0 {
            let raid_is_active = self.level().is_some_and(|world| {
                world
                    .get_raid_at(self.block_position())
                    .is_some_and(|raid| raid.is_active() && !raid.is_over())
            });
            if raid_is_active {
                self.broadcast_entity_event(EntityStatus::VillagerSweat);
            }
        }

        if self.merchant.offers().is_empty() && self.is_trading() {
            self.stop_trading();
        }
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if spawn_reason == EntitySpawnReason::Breeding {
            self.set_villager_data(
                self.villager_data()
                    .with_profession_ref(data::none_profession()),
            );
        }

        // Vanilla `AbstractVillager.finalizeSpawn` defaults to `AgeableMobGroupData(false)`.
        let group_data = group_data.or(Some(SpawnGroupData::AgeableMob(
            AgeableMobGroupData::with_should_spawn_baby(false),
        )));
        self.finalize_villager_type(world, self.block_position());
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held_stack = {
            let inventory = player.inventory.lock();
            let held = inventory.get_item_in_hand(hand);
            held.copy_with_count(held.count())
        };

        if held_stack.is(&vanilla_items::VILLAGER_SPAWN_EGG)
            || !Entity::is_alive(self)
            || self.is_trading()
            || LivingEntity::is_sleeping(self)
        {
            return AgeableMob::mob_interact_ageable(self, player, hand);
        }

        if AgeableMob::is_baby(self) {
            self.set_unhappy();
            return InteractionResult::Success;
        }

        let no_offers = self.merchant.offers().is_empty();
        if hand == InteractionHand::MainHand {
            if no_offers {
                self.set_unhappy();
            }

            player.award_custom_stat(&vanilla_custom_stats::TALKED_TO_VILLAGER);
        }

        if no_offers {
            return InteractionResult::Consume;
        }

        self.start_trading(player);
        InteractionResult::Success
    }

    /// Vanilla `Villager.pickUpItem` through `InventoryCarrier.pickUpItem`.
    fn pick_up_item(&self, world: &Arc<World>, item_entity: &ItemEntity) {
        let item_stack = item_entity.get_item();
        if !self.wants_to_pick_up_item(&item_stack) {
            return;
        }

        let count = item_stack.count();
        let remainder = self.with_inventory_mut(|inventory| {
            if !inventory.can_add_item(&item_stack) {
                return None;
            }

            Some(inventory.add_item(&item_stack))
        });
        let Some(remainder) = remainder else {
            return;
        };

        world.broadcast_to_nearby(
            ChunkPos::from_entity_pos(item_entity.position()),
            CTakeItemEntity::new(item_entity.id(), self.id(), count - remainder.count()),
            None,
        );

        if remainder.is_empty() {
            item_entity.set_removed(RemovalReason::Discarded);
        } else {
            item_entity.set_item(remainder);
        }
    }

    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        self.wants_to_pick_up_item(item_stack)
    }
}

impl ReputationEventHandler for VillagerEntity {
    /// Vanilla `Villager.onReputationEventFrom`.
    fn on_reputation_event_from(&self, event_type: ReputationEventType, source: SharedEntity) {
        let source_uuid = source.uuid();
        let gossips_update: Option<&[(GossipType, i32)]> =
            if event_type == ReputationEventType::ZOMBIE_VILLAGER_CURED {
                Some(&[
                    (
                        GossipType::MajorPositive,
                        GossipType::REPUTATION_CHANGE_PER_EVERLASTING_MEMORY,
                    ),
                    (
                        GossipType::MinorPositive,
                        GossipType::REPUTATION_CHANGE_PER_EVENT,
                    ),
                ])
            } else if event_type == ReputationEventType::TRADE {
                Some(&[(GossipType::Trading, GossipType::REPUTATION_CHANGE_PER_TRADE)])
            } else if event_type == ReputationEventType::VILLAGER_HURT {
                Some(&[(
                    GossipType::MinorNegative,
                    GossipType::REPUTATION_CHANGE_PER_EVENT,
                )])
            } else if event_type == ReputationEventType::VILLAGER_KILLED {
                Some(&[(
                    GossipType::MajorNegative,
                    GossipType::REPUTATION_CHANGE_PER_EVENT,
                )])
            } else {
                None
            };

        if let Some(updates) = gossips_update {
            let mut gossips = self.gossips.lock();
            for (gossip_type, amount) in updates {
                gossips.add(source_uuid, *gossip_type, *amount);
            }
        }

        if self
            .merchant
            .trading_player()
            .is_some_and(|trading_player| trading_player.uuid() == source_uuid)
        {
            self.update_special_prices_for_trading_player();
        }
    }
}

impl Merchant for VillagerEntity {
    fn set_trading_player(&self, player: Option<SharedEntity>) {
        VillagerEntity::set_trading_player(self, player);
    }

    fn get_trading_player(&self) -> Option<SharedEntity> {
        self.merchant.trading_player()
    }

    fn offers(&self) -> MerchantOffersGuard<'_> {
        self.ensure_offers_initialized();
        self.merchant.offers()
    }

    fn override_offers(&self, offers: MerchantOffers) {
        self.merchant.set_offers(offers);
    }

    fn notify_trade(&self, offer: &mut MerchantOffer) {
        VillagerEntity::notify_trade(self, offer);
    }

    fn notify_trade_updated(&self, stack: &ItemStack) {
        VillagerEntity::notify_trade_updated(self, stack);
    }

    fn villager_xp(&self) -> i32 {
        self.merchant_xp()
    }

    fn override_xp(&self, xp: i32) {
        self.override_villager_xp(xp);
    }

    fn show_progress_bar(&self) -> bool {
        true
    }

    fn can_restock(&self) -> bool {
        true
    }

    fn still_valid(&self, player: &Player) -> bool {
        self.still_valid_for_trade(player)
    }
}
