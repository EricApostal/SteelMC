//! Merchant (villager trading) menu.
//!
//! Mirrors vanilla `MerchantMenu` / `MerchantContainer` / `MerchantResultSlot`:
//! two payment slots, a server-recomputed result slot, and the player
//! inventory. Slots 0 and 1 are returned to the player on close; slot 2 is
//! recomputed from the active offer.

use std::mem;
use std::ptr;
use std::sync::Arc;

use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_custom_stats;
use steel_registry::vanilla_menu_types;
use steel_utils::locks::IntoShared;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::npc::trading::{ItemCost, Merchant, MerchantOffer, MerchantOffers};
use crate::inventory::prelude::*;
use crate::inventory::slots::SlotStorage;
use crate::player::player_inventory::{PlayerInventory, Prediction};

/// Finds the index of `offer` inside `offers` so it can be mutated in place later.
fn offer_index(offers: &MerchantOffers, offer: &MerchantOffer) -> Option<usize> {
    offers
        .iter()
        .position(|candidate| ptr::eq(candidate, offer))
}

/// Vanilla `MerchantContainer.updateSellItem`'s offer lookup.
///
/// Prefers a valid offer matching `buy_a`/`buy_b`; otherwise retries with the
/// payment slots swapped and falls back to the (possibly out-of-stock) first
/// match, exactly as vanilla keeps it as `activeOffer`.
fn recompute_active(
    offers: &MerchantOffers,
    buy_a: &ItemStack,
    buy_b: &ItemStack,
    selection_hint: i32,
) -> Option<usize> {
    let found = offers.get_recipe_for(buy_a, buy_b, selection_hint);
    if let Some(offer) = found.filter(|offer| !offer.is_out_of_stock()) {
        return offer_index(offers, offer);
    }
    let fallback = found.and_then(|offer| offer_index(offers, offer));
    match offers.get_recipe_for(buy_b, buy_a, selection_hint) {
        Some(offer) if !offer.is_out_of_stock() => offer_index(offers, offer),
        _ => fallback,
    }
}

/// Merchant container holding the two payment slots and the recomputed result.
///
/// Mirrors vanilla `MerchantContainer`: slot 0/1 are payments, slot 2 is the
/// assembled result of the active offer.
pub struct MerchantContainer {
    merchant: Arc<dyn Merchant>,
    items: [ItemStack; 3],
    /// Index of the selected offer in the merchant's list, kept instead of a
    /// copy so `notify_trade` mutates the canonical offer.
    active_offer: Option<usize>,
    selection_hint: i32,
    future_xp: i32,
}

// SAFETY: This Steel-owned key uniquely identifies the merchant container type.
unsafe impl DowncastType for MerchantContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/merchant");
}

impl MerchantContainer {
    /// Creates a container backed by `merchant`.
    #[must_use]
    fn new(merchant: Arc<dyn Merchant>) -> Self {
        Self {
            merchant,
            items: [ItemStack::empty(), ItemStack::empty(), ItemStack::empty()],
            active_offer: None,
            selection_hint: 0,
            future_xp: 0,
        }
    }

    /// Index of the active offer in the merchant's list, if any.
    #[must_use]
    const fn active_offer(&self) -> Option<usize> {
        self.active_offer
    }

    /// The xp the active offer would grant.
    #[must_use]
    const fn get_future_xp(&self) -> i32 {
        self.future_xp
    }

    /// The current trade selection hint.
    #[must_use]
    const fn selection_hint(&self) -> i32 {
        self.selection_hint
    }

    /// Sets the trade selection hint and recomputes the result slot.
    fn set_selection_hint(&mut self, hint: i32) {
        self.selection_hint = hint;
        self.update_sell_item();
    }

    /// Sets one of the payment slots (0 or 1).
    fn set_payment(&mut self, slot: usize, stack: ItemStack) {
        self.items[slot] = stack;
    }

    /// Recomputes the result slot and active offer from the payment slots.
    ///
    /// Mirrors vanilla `MerchantContainer.updateSellItem`.
    fn update_sell_item(&mut self) {
        let (buy_a, buy_b) = if self.items[0].is_empty() {
            (self.items[1].clone(), ItemStack::empty())
        } else {
            (self.items[0].clone(), self.items[1].clone())
        };

        let mut active = None;
        let mut result = ItemStack::empty();
        let mut future_xp = 0;

        if !buy_a.is_empty() {
            let selected = {
                let offers = self.merchant.offers();
                recompute_active(&offers, &buy_a, &buy_b, self.selection_hint)
            };
            if let Some(index) = selected {
                let offers = self.merchant.offers();
                if let Some(offer) = offers.get(index) {
                    future_xp = offer.xp();
                    result = offer.assemble();
                    active = Some(index);
                }
            }
        }

        self.items[2] = result;
        self.active_offer = active;
        self.future_xp = future_xp;
        if !buy_a.is_empty() {
            self.merchant.notify_trade_updated(&self.items[2]);
        }
    }
}

impl Container for MerchantContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_max_stack_size(&self) -> i32 {
        64
    }

    fn set_changed(&mut self) {}
}

/// Result slot of a merchant menu.
///
/// Server-authoritative: rejects placement, only allows pickup while the active
/// offer is satisfied by the payment slots, and applies the trade on take.
pub struct MerchantResultSlot {
    base: NormalSlot,
    merchant: Arc<dyn Merchant>,
    container: ContainerRef,
}

// SAFETY: This Steel-owned key uniquely identifies merchant result slots.
unsafe impl DowncastType for MerchantResultSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:slot/merchant_result");
}

impl MerchantResultSlot {
    fn new(container: ContainerRef, index: usize, merchant: Arc<dyn Merchant>) -> Self {
        Self {
            base: NormalSlot::new(container.clone(), index),
            merchant,
            container,
        }
    }
}

impl Slot for MerchantResultSlot {
    fn storage(&self) -> &SlotStorage {
        self.base.storage()
    }

    fn get_item<'a>(&self, guard: &'a ContainerLockGuard) -> &'a ItemStack {
        self.base.get_item(guard)
    }

    fn get_item_mut<'a>(&self, guard: &'a mut ContainerLockGuard) -> &'a mut ItemStack {
        self.base.get_item_mut(guard)
    }

    fn set_item(&self, guard: &mut ContainerLockGuard, stack: ItemStack) {
        self.base.set_item(guard, stack);
    }

    fn get_max_stack_size(&self, guard: &ContainerLockGuard) -> i32 {
        self.base.get_max_stack_size(guard)
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        self.base.set_changed(guard);
    }

    fn get_container_slot(&self) -> usize {
        self.base.get_container_slot()
    }

    fn is_fake(&self) -> bool {
        true
    }

    fn may_place(&self, _stack: &ItemStack) -> bool {
        false
    }

    fn allow_modification(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        false
    }

    fn may_pickup(&self, guard: &ContainerLockGuard, _player: &Player) -> bool {
        let container_id = self.container.container_id();
        let Some(container) = guard.get_typed::<MerchantContainer>(container_id) else {
            return false;
        };
        let Some(index) = container.active_offer() else {
            return false;
        };
        let offers = self.merchant.offers();
        let Some(offer) = offers.get(index) else {
            return false;
        };
        let buy_a = &container.items[0];
        let buy_b = &container.items[1];
        offer.satisfied_by(buy_a, buy_b) || offer.satisfied_by(buy_b, buy_a)
    }

    fn remove(&self, guard: &mut ContainerLockGuard, _amount: i32) -> ItemStack {
        mem::take(self.get_item_mut(guard))
    }

    fn on_take(
        &self,
        guard: &mut ContainerLockGuard,
        stack: &ItemStack,
        player: &Player,
    ) -> Option<ItemStack> {
        // Vanilla `MerchantResultSlot.checkTakeAchievements` awards the crafted
        // stat for the taken result before the trade is applied.
        if !stack.is_empty() {
            player.award_stat_with_count(
                &vanilla_stat_types::ITEM_CRAFTED,
                stack.item(),
                stack.count(),
            );
        }

        let container_id = self.container.container_id();
        let container = guard.get_typed_mut::<MerchantContainer>(container_id)?;
        let index = container.active_offer()?;

        let mut buy_a = container.items[0].clone();
        let mut buy_b = container.items[1].clone();
        let xp;
        let mut traded = false;

        {
            let mut offers = self.merchant.offers();
            let offer = offers.get_mut(index)?;
            xp = offer.xp();
            if offer.take(&mut buy_a, &mut buy_b) || offer.take(&mut buy_b, &mut buy_a) {
                // Mutates the canonical offer in place: uses, demand and price.
                self.merchant.notify_trade(offer);
                player.award_custom_stat(&vanilla_custom_stats::TRADED_WITH_VILLAGER);
                container.set_payment(0, buy_a);
                container.set_payment(1, buy_b);
                traded = true;
            }
        }

        if traded {
            container.update_sell_item();
        }
        self.merchant.override_xp(self.merchant.villager_xp() + xp);
        None
    }
}

/// Builds the merchant menu for one trader.
#[must_use]
pub fn merchant_menu(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    merchant: Arc<dyn Merchant>,
    level: i32,
) -> Menu {
    let container = MerchantContainer::new(Arc::clone(&merchant)).into_shared();
    let container_ref = ContainerRef::from(Arc::clone(&container));
    let result_container_ref = container_ref.clone();
    let result_merchant = Arc::clone(&merchant);

    let mut builder = MenuBuilder::new(&vanilla_menu_types::MERCHANT, container_id);
    let mut slots = builder.split(&container_ref);
    let trade_a = builder.section(&mut slots, 1);
    let trade_b = builder.section(&mut slots, 1);
    let result = builder.section_with(
        &mut slots,
        1,
        SectionKind::custom(move |_container, index| {
            Box::new(MerchantResultSlot::new(
                result_container_ref.clone(),
                index,
                Arc::clone(&result_merchant),
            ))
        }),
    );
    let player = builder.player_inventory(&inventory);

    // Payment slots are handed back to the player (or dropped) on close.
    builder.drain([trade_a, trade_b]);

    builder.build(MerchantMenuKind {
        merchant,
        container,
        container_ref,
        level,
        trade_a,
        trade_b,
        result,
        player,
    })
}

/// Per-menu merchant state.
pub struct MerchantMenuKind {
    merchant: Arc<dyn Merchant>,
    container: Shared<MerchantContainer>,
    container_ref: ContainerRef,
    /// Villager level shown in the offers packet.
    level: i32,
    trade_a: Section,
    trade_b: Section,
    result: Section,
    player: PlayerInventorySections,
}

// SAFETY: This Steel-owned key uniquely identifies the merchant menu kind.
unsafe impl DowncastType for MerchantMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/merchant");
}

impl MerchantMenuKind {
    /// Recomputes the result slot from the current payment items.
    fn update_sell_item(&self, guard: &mut ContainerLockGuard) {
        let container_id = self.container_ref.container_id();
        let Some(container) = guard.get_typed_mut::<MerchantContainer>(container_id) else {
            return;
        };
        container.update_sell_item();
    }

    /// Applies a trade selection from `SSelectTrade` (vanilla `setSelectionHint`).
    pub fn set_selection_hint(&self, guard: &mut ContainerLockGuard, hint: i32) {
        let container_id = self.container_ref.container_id();
        let Some(container) = guard.get_typed_mut::<MerchantContainer>(container_id) else {
            return;
        };
        container.set_selection_hint(hint);
    }

    /// Moves matching inventory items into the payment slots for the selected
    /// offer (vanilla `MerchantMenu.tryMoveItems`).
    pub fn try_move_items(
        &self,
        behavior: &MenuBehavior,
        guard: &mut ContainerLockGuard,
        new_trade_index: i32,
    ) {
        let offers_len = self.merchant.offers().len();
        if new_trade_index < 0 || offers_len <= new_trade_index as usize {
            return;
        }

        let container_id = self.container_ref.container_id();

        // Return the current payments to the player before pulling new ones.
        for slot in 0..2 {
            let old = guard
                .get_typed::<MerchantContainer>(container_id)
                .map_or_else(ItemStack::empty, |container| container.items[slot].clone());
            if old.is_empty() {
                continue;
            }
            let mut remaining = old;
            if !behavior.move_item_stack_to(
                guard,
                slot,
                &mut remaining,
                self.player.all().start(),
                self.player.all().end(),
                FillDirection::Backward,
            ) {
                return;
            }
            if let Some(container) = guard.get_typed_mut::<MerchantContainer>(container_id) {
                container.set_payment(slot, remaining);
            }
        }

        let both_empty = guard
            .get_typed::<MerchantContainer>(container_id)
            .is_some_and(|container| {
                container.items[0].is_empty() && container.items[1].is_empty()
            });
        if !both_empty {
            return;
        }

        let costs = {
            let offers = self.merchant.offers();
            offers
                .get(new_trade_index as usize)
                .map(|offer| (offer.item_cost_a().clone(), offer.item_cost_b().cloned()))
        };
        let Some((cost_a, cost_b)) = costs else {
            return;
        };

        move_from_inventory_to_payment_slot(
            behavior,
            guard,
            container_id,
            0,
            &cost_a,
            &self.player,
        );
        if let Some(cost_b) = &cost_b {
            move_from_inventory_to_payment_slot(
                behavior,
                guard,
                container_id,
                1,
                cost_b,
                &self.player,
            );
        }

        self.update_sell_item(guard);
    }
}

/// Moves matching player inventory items into a payment slot.
///
/// Mirrors vanilla `MerchantMenu.moveFromInventoryToPaymentSlot`.
fn move_from_inventory_to_payment_slot(
    behavior: &MenuBehavior,
    guard: &mut ContainerLockGuard,
    container_id: ContainerId,
    payment_slot: usize,
    cost: &ItemCost,
    player: &PlayerInventorySections,
) {
    for index in player.all().start()..player.all().end() {
        let inventory_item = behavior.slots()[index].get_item(guard).clone();
        if inventory_item.is_empty() || !cost.test(&inventory_item) {
            continue;
        }

        let current = {
            let Some(container) = guard.get_typed::<MerchantContainer>(container_id) else {
                return;
            };
            container.items[payment_slot].clone()
        };
        if current.is_empty() || ItemStack::is_same_item_same_components(&inventory_item, &current)
        {
            let max_stack_size = inventory_item.max_stack_size();
            let move_count = (max_stack_size - current.count()).min(inventory_item.count());
            let new_payment = inventory_item.copy_with_count(current.count() + move_count);
            behavior.slots()[index]
                .get_item_mut(guard)
                .shrink(move_count);
            behavior.slots()[index].set_changed(guard);
            if let Some(container) = guard.get_typed_mut::<MerchantContainer>(container_id) {
                container.set_payment(payment_slot, new_payment.clone());
            }
            if new_payment.count() >= max_stack_size {
                break;
            }
        }
    }
}

impl MenuKind for MerchantMenuKind {
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.merchant.still_valid(player)
    }

    fn on_open(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) {
        self.update_sell_item(guard);
        let offers = {
            let offers = self.merchant.offers();
            offers.copy()
        };
        player.send_merchant_offers(
            i32::from(behavior.container_id()),
            &offers,
            self.level,
            self.merchant.villager_xp(),
            self.merchant.show_progress_bar(),
            self.merchant.can_restock(),
        );
    }

    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        self.merchant.set_trading_player(None);
    }

    fn slots_changed(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_sell_item(guard);
    }

    fn can_take_item_for_pick_all(&self, _carried: &ItemStack, _slot_index: usize) -> bool {
        false
    }

    fn quick_move(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        slot_index: usize,
        player: &Player,
    ) -> Option<ItemStack> {
        let clicked = behavior.slots()[slot_index].get_item(guard).clone();
        if clicked.is_empty() {
            return Some(ItemStack::empty());
        }
        if !behavior.slots()[slot_index].may_pickup(guard, player) {
            return Some(ItemStack::empty());
        }

        let mut remaining = clicked.clone();
        let all = self.player.all();
        let moved = if self.result.contains(slot_index) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                all.start(),
                all.end(),
                FillDirection::Backward,
            )
        } else if self.trade_a.contains(slot_index) || self.trade_b.contains(slot_index) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                all.start(),
                all.end(),
                FillDirection::Forward,
            )
        } else if self.player.main().contains(slot_index) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.player.hotbar().start(),
                self.player.hotbar().end(),
                FillDirection::Forward,
            )
        } else if self.player.hotbar().contains(slot_index) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.player.main().start(),
                self.player.main().end(),
                FillDirection::Forward,
            )
        } else {
            false
        };

        if !moved {
            return Some(ItemStack::empty());
        }
        behavior.update_quick_move_source(guard, slot_index, &remaining, &clicked);
        if remaining.count() == clicked.count() {
            return Some(ItemStack::empty());
        }
        if let Some(remainder) = behavior.slots()[slot_index].on_take(guard, &remaining, player) {
            player.add_item_or_drop_with_guard(guard, remainder, Prediction::Predicted);
        }
        Some(clicked)
    }
}

impl MerchantMenuKind {
    /// The container backing the payment and result slots.
    #[must_use]
    pub const fn container(&self) -> &Shared<MerchantContainer> {
        &self.container
    }

    /// The xp the active offer would grant.
    #[must_use]
    pub fn future_xp(&self) -> i32 {
        self.container.lock().get_future_xp()
    }

    /// The current trade selection hint.
    #[must_use]
    pub fn selection_hint(&self) -> i32 {
        self.container.lock().selection_hint()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::data_component_predicate::DataComponentExactPredicate;
    use steel_registry::{init_vanilla_registry, item_stack::ItemStack, vanilla_items};
    use steel_utils::Downcast as _;
    use steel_utils::locks::SyncMutex;

    use super::*;
    use crate::entity::SharedEntity;
    use crate::entity::npc::trading::{
        ItemCost, Merchant, MerchantOffer, MerchantOffers, MerchantOffersGuard,
    };
    use crate::inventory::click::{Click, MouseButton};
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    /// Minimal trader whose offers live in a plain list.
    struct TestMerchant {
        offers: SyncMutex<MerchantOffers>,
        notified_uses: SyncMutex<Vec<i32>>,
        xp: SyncMutex<i32>,
    }

    impl Merchant for TestMerchant {
        fn set_trading_player(&self, _player: Option<SharedEntity>) {}

        fn get_trading_player(&self) -> Option<SharedEntity> {
            None
        }

        fn offers(&self) -> MerchantOffersGuard<'_> {
            self.offers.lock()
        }

        fn override_offers(&self, offers: MerchantOffers) {
            *self.offers.lock() = offers;
        }

        fn notify_trade(&self, offer: &mut MerchantOffer) {
            offer.increase_uses();
            self.notified_uses.lock().push(offer.uses());
        }

        fn notify_trade_updated(&self, _stack: &ItemStack) {}

        fn villager_xp(&self) -> i32 {
            *self.xp.lock()
        }

        fn override_xp(&self, xp: i32) {
            *self.xp.lock() = xp;
        }

        fn show_progress_bar(&self) -> bool {
            false
        }

        fn still_valid(&self, _player: &Player) -> bool {
            true
        }
    }

    fn emeralds(count: i32) -> ItemStack {
        ItemStack::with_count(&vanilla_items::EMERALD, count)
    }

    fn wheat(count: i32) -> ItemStack {
        ItemStack::with_count(&vanilla_items::WHEAT, count)
    }

    fn test_merchant() -> Arc<TestMerchant> {
        let mut offers = MerchantOffers::new();
        offers.push(MerchantOffer::new(
            ItemCost::new(
                &vanilla_items::WHEAT,
                20,
                DataComponentExactPredicate::EMPTY,
            ),
            emeralds(1),
            12,
            5,
            0.0,
        ));
        Arc::new(TestMerchant {
            offers: SyncMutex::new(offers),
            notified_uses: SyncMutex::new(Vec::new()),
            xp: SyncMutex::new(0),
        })
    }

    fn open_test_menu(player: &Player, merchant: &Arc<TestMerchant>) -> Menu {
        let test_merchant: Arc<TestMerchant> = Arc::clone(merchant);
        let merchant: Arc<dyn Merchant> = test_merchant;
        let mut menu = merchant_menu(Arc::clone(&player.inventory), 1, merchant, 1);
        menu.on_open(player);
        menu
    }

    fn merchant_id(menu: &Menu) -> ContainerId {
        menu.kind()
            .downcast_ref::<MerchantMenuKind>()
            .expect("merchant kind")
            .container_ref
            .container_id()
    }

    fn merchant_item(menu: &Menu, slot: usize) -> ItemStack {
        menu.behavior()
            .lock_all_containers()
            .get(merchant_id(menu))
            .expect("merchant container locked")
            .get_item(slot)
            .clone()
    }

    /// Places `stack` in the payment slot through a normal left click.
    fn click_payment(menu: &mut Menu, player: &Player, slot: usize, stack: ItemStack) {
        *menu.behavior_mut().carried_mut() = stack;
        menu.clicked(
            Click::Pickup {
                slot,
                button: MouseButton::Left,
            },
            player,
        );
    }

    #[test]
    fn payments_matching_an_offer_fill_the_result_slot() {
        init_vanilla_registry();
        let world = fresh_test_world("merchant_menu_result");
        let player = TestPlayerBuilder::new(Arc::clone(&world.world), "MerchantTester", 1).build();
        let merchant = test_merchant();
        let mut menu = open_test_menu(&player, &merchant);

        assert!(merchant_item(&menu, 2).is_empty());

        click_payment(&mut menu, &player, 0, wheat(20));

        assert!(merchant_item(&menu, 0).is(&vanilla_items::WHEAT));
        assert!(merchant_item(&menu, 2).is(&vanilla_items::EMERALD));
        let kind = menu
            .kind()
            .downcast_ref::<MerchantMenuKind>()
            .expect("kind");
        assert_eq!(kind.future_xp(), 5);
    }

    #[test]
    fn taking_the_result_consumes_payments_and_records_uses() {
        init_vanilla_registry();
        let world = fresh_test_world("merchant_menu_take");
        let player = TestPlayerBuilder::new(Arc::clone(&world.world), "MerchantTaker", 1).build();
        let merchant = test_merchant();
        let mut menu = open_test_menu(&player, &merchant);

        click_payment(&mut menu, &player, 0, wheat(35));

        *menu.behavior_mut().carried_mut() = ItemStack::empty();
        menu.clicked(
            Click::Pickup {
                slot: 2,
                button: MouseButton::Left,
            },
            &player,
        );

        assert!(menu.behavior().carried().is(&vanilla_items::EMERALD));
        assert_eq!(merchant.notified_uses.lock().len(), 1);
        assert_eq!(merchant.offers.lock()[0].uses(), 1);
        assert_eq!(*merchant.xp.lock(), 5);
        assert_eq!(merchant_item(&menu, 0).count(), 15);
    }

    #[test]
    fn selection_hint_and_try_move_fill_the_payment_slots() {
        init_vanilla_registry();
        let world = fresh_test_world("merchant_menu_hint");
        let player = TestPlayerBuilder::new(Arc::clone(&world.world), "MerchantHint", 1).build();
        let merchant = test_merchant();
        let menu = open_test_menu(&player, &merchant);

        let inventory_slot = {
            let kind = menu
                .kind()
                .downcast_ref::<MerchantMenuKind>()
                .expect("kind");
            kind.player.all().start()
        };
        {
            let mut guard = menu.behavior().lock_all_containers();
            menu.behavior().slots()[inventory_slot].set_item(&mut guard, wheat(20));
        }
        {
            let kind = menu
                .kind()
                .downcast_ref::<MerchantMenuKind>()
                .expect("kind");
            let mut guard = menu.behavior().lock_all_containers();
            kind.set_selection_hint(&mut guard, 0);
            kind.try_move_items(menu.behavior(), &mut guard, 0);
        }

        assert_eq!(merchant_item(&menu, 0).count(), 20);
        assert!(!merchant_item(&menu, 2).is_empty());
    }
}
