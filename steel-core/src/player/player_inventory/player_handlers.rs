use std::{f32::consts::TAU, mem, sync::Arc};

use crate::{
    entity::{Entity, LivingEntity as _, RemovalReason, entities::ItemEntity},
    inventory::{
        click::Click,
        container::{Container, CraftingContainer, clear_or_count_matching_stack},
        lock::{ContainerId, ContainerLockGuard, ContainerRef},
        menu::{
            Menu,
            kinds::{InventoryKind, MerchantMenuKind, merchant_menu},
        },
        slots::CraftingHandler,
    },
    player::{Player, connection::NetworkConnection as _},
};
use glam::DVec3;

use crate::entity::npc::trading::{ItemCost, Merchant, MerchantOffers};
use steel_protocol::packets::game::SSeenAdvancement::Opened;
use steel_protocol::packets::game::{
    CContainerClose, CMerchantOffers, CMountScreenOpen, COpenScreen, CSetPlayerInventory,
    ClickType, MerchantItemCost, MerchantOffer as WireMerchantOffer, SContainerButtonClick,
    SContainerClick, SContainerClose, SContainerSlotStateChanged, SRenameItem, SSeenAdvancement,
    SSelectTrade, SSetBeacon, SSetCarriedItem, SSetCreativeModeSlot, SoundSource,
};
use steel_registry::data_components::vanilla_components::{BUNDLE_CONTENTS, SwingAnimation};
use steel_registry::item_stack::ItemStack;
use steel_registry::mob_effect::MobEffectRef;
use steel_registry::sound_events;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_custom_stats;
use steel_registry::{REGISTRY, RegistryExt, vanilla_entities};
use steel_utils::{
    Downcast as _, WorldAabb,
    locks::Shared,
    translations,
    types::{GameType, InteractionHand},
};
use text_components::TextComponent;

use super::{
    DeferredMenuAction, MenuItemDisposition, MenuOpenContext, MenuOpenScreen, MenuRemovalStatus,
    OpenMenuDispatch, OpenMenuUnavailable, PendingMenuOpen, PlayerInventory, Prediction,
    PreparedMenu, TerminalMenuRemoval,
};

impl Player {
    fn take_open_menu_for_callback(
        &self,
        expected_container_id: Option<i32>,
    ) -> Result<Menu, OpenMenuUnavailable> {
        let mut open_menu = self.open_menu.lock();
        if open_menu.dispatch.is_some() {
            return Err(OpenMenuUnavailable::Unavailable);
        }

        let Some(menu) = open_menu.menu.as_ref() else {
            return Err(OpenMenuUnavailable::Closed);
        };
        if expected_container_id.is_some_and(|expected| i32::from(menu.container_id()) != expected)
        {
            return Err(OpenMenuUnavailable::Unavailable);
        }

        let container_id = menu.container_id();
        let overrides_player_slots = menu.overrides_player_slots();
        let Some(menu) = open_menu.menu.take() else {
            return Err(OpenMenuUnavailable::Unavailable);
        };
        open_menu.dispatch = Some(OpenMenuDispatch {
            container_id,
            overrides_player_slots,
            actions: Vec::new(),
        });
        Ok(menu)
    }

    fn finish_open_menu_callback(&self, menu: Menu) {
        let actions = {
            let mut open_menu = self.open_menu.lock();
            let Some(dispatch) = open_menu.dispatch.take() else {
                open_menu.menu = Some(menu);
                return;
            };
            if let Some(terminal_removal) = open_menu.terminal_removal.as_mut() {
                Self::queue_deferred_menus(terminal_removal, dispatch.actions);
                drop(open_menu);
                self.finish_terminal_menu_main_cleanup(Some(menu));
                return;
            }
            open_menu.menu = Some(menu);
            open_menu.active_open_operations += 1;
            dispatch.actions
        };

        self.run_deferred_menu_actions(actions);
    }

    fn finish_open_menu_removal(&self) {
        let actions = {
            let mut open_menu = self.open_menu.lock();
            let Some(dispatch) = open_menu.dispatch.take() else {
                return;
            };
            if let Some(terminal_removal) = open_menu.terminal_removal.as_mut() {
                Self::queue_deferred_menus(terminal_removal, dispatch.actions);
                drop(open_menu);
                self.finish_terminal_menu_main_cleanup(None);
                return;
            }
            open_menu.active_open_operations += 1;
            dispatch.actions
        };

        self.run_deferred_menu_actions(actions);
    }

    fn run_deferred_menu_actions(&self, actions: Vec<DeferredMenuAction>) {
        for action in actions {
            match action {
                DeferredMenuAction::Close { send_packet } => {
                    if send_packet {
                        self.close_container();
                    } else {
                        self.do_close_container();
                    }
                }
                DeferredMenuAction::Open(prepared) => {
                    self.execute_menu_open(*prepared);
                }
                DeferredMenuAction::Install(prepared) => {
                    let PreparedMenu {
                        title,
                        menu,
                        screen,
                    } = *prepared;
                    self.open_prepared_menu(title, menu, screen);
                }
            }
        }

        self.finish_menu_open_operation();
    }

    fn queue_deferred_menus(
        terminal_removal: &mut TerminalMenuRemoval,
        actions: Vec<DeferredMenuAction>,
    ) {
        terminal_removal
            .pending_menus
            .extend(actions.into_iter().filter_map(|action| match action {
                DeferredMenuAction::Install(prepared) => Some(prepared.menu),
                DeferredMenuAction::Close { .. } | DeferredMenuAction::Open(_) => None,
            }));
    }

    fn begin_menu_open_operation(&self) -> bool {
        let mut open_menu = self.open_menu.lock();
        if open_menu.terminal_removal.is_some() {
            return false;
        }
        open_menu.active_open_operations += 1;
        true
    }

    fn finish_menu_open_operation(&self) {
        {
            let mut open_menu = self.open_menu.lock();
            debug_assert!(open_menu.active_open_operations > 0);
            open_menu.active_open_operations -= 1;
        }
        self.try_finish_terminal_menu_removal();
    }

    /// Attempts to pick up nearby item entities.
    ///
    /// Mirrors vanilla's `Player.aiStep()` pickup block: the pickup area covers the
    /// vehicle when riding, every non-orb entity in range is touched, and exactly
    /// one randomly chosen experience orb is touched (vanilla keeps orbs out of the
    /// per-entity loop so a co-located pile is consumed in random order).
    pub(in crate::player) fn touch_nearby_items(&self) {
        if self.game_mode() == GameType::Spectator || self.get_health() <= 0.0 {
            return;
        }

        let bounding_box = self.bounding_box();
        let pickup_area = match self.vehicle() {
            Some(vehicle) if !vehicle.is_removed() => {
                WorldAabb::encapsulating(&bounding_box, &vehicle.bounding_box())
                    .inflate_xyz(1.0, 0.0, 1.0)
            }
            _ => bounding_box.inflate_xyz(1.0, 0.5, 1.0),
        };
        let world = self.get_world();
        let entities = world.get_entities_in_aabb(&pickup_area);

        let Some(player_arc) = world.players.get_by_entity_id(self.id()) else {
            return;
        };

        let mut orbs = Vec::new();
        for entity in entities {
            if entity.id() == self.id() || entity.is_removed() {
                continue;
            }

            if entity.entity_type().key == vanilla_entities::EXPERIENCE_ORB.key {
                orbs.push(entity);
            } else {
                entity.player_touch(&player_arc);
            }
        }

        if !orbs.is_empty() {
            let index = rand::random_range(0..orbs.len());
            Arc::clone(&orbs[index]).player_touch(&player_arc);
        }
    }

    /// Resolves an optional beacon effect ID, rejecting IDs vanilla's packet codec would reject.
    pub(super) fn resolve_beacon_effect(id: Option<i32>) -> Result<Option<MobEffectRef>, ()> {
        id.map(|id| {
            let id = usize::try_from(id).map_err(|_| ())?;
            REGISTRY.mob_effects.by_id(id).ok_or(())
        })
        .transpose()
    }

    /// Handles a beacon effect selection from the set-beacon packet.
    pub fn handle_set_beacon_packet(&self, packet: SSetBeacon) {
        let (Ok(primary), Ok(secondary)) = (
            Self::resolve_beacon_effect(packet.primary),
            Self::resolve_beacon_effect(packet.secondary),
        ) else {
            log::warn!(
                "Player {} sent an unknown beacon effect id",
                self.gameprofile.name()
            );
            self.disconnect(translations::MULTIPLAYER_DISCONNECT_GENERIC.msg());
            return;
        };

        let Ok(mut menu) = self.take_open_menu_for_callback(None) else {
            return;
        };
        if !menu.still_valid(self) {
            log::debug!(
                "Player {} interacted with invalid menu",
                self.gameprofile.name()
            );
            self.finish_open_menu_callback(menu);
            return;
        }
        if !menu.update_effects(primary, secondary, &self.connection) {
            log::warn!(
                "Player {} tried to set invalid beacon effects",
                self.gameprofile.name()
            );
            self.finish_open_menu_callback(menu);
            self.disconnect(translations::MULTIPLAYER_DISCONNECT_GENERIC.msg());
            return;
        }
        self.finish_open_menu_callback(menu);
    }

    /// Handles a container button click packet (e.g., enchanting table buttons).
    ///
    /// Mirrors vanilla `ServerGamePacketListenerImpl.handleContainerButtonClick`:
    /// spectator and mismatched container ids are ignored, buttons on stale
    /// menus are dropped, and an unaccepted button is ignored without
    /// disconnecting the client.
    pub fn handle_container_button_click(&self, packet: SContainerButtonClick) {
        self.reset_last_action_time();
        log::debug!(
            "Player {} clicked button {} in container {}",
            self.gameprofile.name(),
            packet.button_id,
            packet.container_id
        );
        let Ok(mut menu) = self.take_open_menu_for_callback(Some(packet.container_id)) else {
            return;
        };
        if self.game_mode() == GameType::Spectator {
            self.finish_open_menu_callback(menu);
            return;
        }
        if !menu.still_valid(self) {
            log::debug!(
                "Player {} clicked a button in an invalid menu",
                self.gameprofile.name()
            );
            self.finish_open_menu_callback(menu);
            return;
        }
        if menu.click_menu_button(packet.button_id, self, &self.connection) {
            self.finish_open_menu_callback(menu);
            return;
        }
        log::debug!(
            "Player {} sent an unaccepted button click {} in container {}",
            self.gameprofile.name(),
            packet.button_id,
            packet.container_id
        );
        self.finish_open_menu_callback(menu);
    }

    /// Handles a trade selection from a merchant screen.
    ///
    /// Based on Java's `ServerGamePacketListenerImpl::handleSelectTrade`.
    pub fn handle_select_trade(&self, packet: SSelectTrade) {
        let Ok(mut menu) = self.take_open_menu_for_callback(None) else {
            return;
        };
        if !menu.still_valid(self) {
            log::debug!(
                "Player {} interacted with invalid menu",
                self.gameprofile.name()
            );
            self.finish_open_menu_callback(menu);
            return;
        }

        {
            let Some(kind) = menu.kind().downcast_ref::<MerchantMenuKind>() else {
                self.finish_open_menu_callback(menu);
                return;
            };
            let mut guard = menu.behavior().lock_all_containers();
            kind.set_selection_hint(&mut guard, packet.item);
            kind.try_move_items(menu.behavior(), &mut guard, packet.item);
        }

        menu.behavior_mut().broadcast_changes(&self.connection);
        self.finish_open_menu_callback(menu);
    }

    /// Sends the offers of a trader to this player.
    ///
    /// Based on Java's `ServerPlayer::sendMerchantOffers`.
    pub fn send_merchant_offers(
        &self,
        container_id: i32,
        offers: &MerchantOffers,
        villager_level: i32,
        villager_xp: i32,
        show_progress: bool,
        can_restock: bool,
    ) {
        let offers = offers
            .iter()
            .map(|offer| WireMerchantOffer {
                cost_a: wire_item_cost(offer.item_cost_a()),
                result: offer.result().clone(),
                cost_b: offer.item_cost_b().map(wire_item_cost),
                out_of_stock: offer.is_out_of_stock(),
                uses: offer.uses(),
                max_uses: offer.max_uses(),
                xp: offer.xp(),
                special_price_diff: offer.special_price_diff(),
                price_multiplier: offer.price_multiplier(),
                demand: offer.demand(),
            })
            .collect();
        self.send_packet(CMerchantOffers::new(
            container_id,
            offers,
            villager_level,
            villager_xp,
            show_progress,
            can_restock,
        ));
    }

    /// Opens the trading screen of `merchant` for this player.
    ///
    /// Based on Java's `Merchant::openTradingScreen`. The offers packet is sent
    /// from the menu's `on_open` hook once the container id is allocated.
    pub fn open_trading_screen(
        &self,
        merchant: Arc<dyn Merchant>,
        title: impl Into<TextComponent>,
        level: i32,
    ) {
        self.open_menu(title, move |context| {
            merchant_menu(
                Arc::clone(&context.player.inventory),
                context.container_id,
                Arc::clone(&merchant),
                level,
            )
        });
    }

    /// Handles a container click packet (slot interaction).
    pub fn handle_container_click(&self, packet: SContainerClick) {
        self.reset_last_action_time();
        match self.take_open_menu_for_callback(Some(packet.container_id)) {
            Ok(mut menu) => {
                self.process_container_click(&mut menu, packet);
                self.finish_open_menu_callback(menu);
            }
            Err(OpenMenuUnavailable::Closed) => {
                let mut menu = self.inventory_menu.lock();
                if i32::from(menu.behavior().container_id()) == packet.container_id {
                    self.process_container_click(&mut menu, packet);
                }
            }
            Err(OpenMenuUnavailable::Unavailable) => {}
        }
    }

    /// Handles a recipe-book placement packet.
    ///
    /// Vanilla `ServerGamePacketListenerImpl.handlePlaceRecipe`.
    pub fn handle_place_recipe(&self, packet: steel_protocol::packets::game::SPlaceRecipe) {
        self.reset_last_action_time();
        if self.game_mode() == GameType::Spectator {
            return;
        }
        if self.get_health() <= 0.0 {
            return;
        }

        match self.take_open_menu_for_callback(Some(packet.container_id)) {
            Ok(mut menu) => {
                self.apply_recipe_placement(&mut menu, &packet);
                self.finish_open_menu_callback(menu);
            }
            Err(OpenMenuUnavailable::Closed) => {
                let mut menu = self.inventory_menu.lock();
                if i32::from(menu.behavior().container_id()) == packet.container_id {
                    self.apply_recipe_placement(&mut menu, &packet);
                }
            }
            Err(OpenMenuUnavailable::Unavailable) => {}
        }
    }

    /// The shared body of [`Self::handle_place_recipe`].
    fn apply_recipe_placement(
        &self,
        menu: &mut Menu,
        packet: &steel_protocol::packets::game::SPlaceRecipe,
    ) {
        use steel_protocol::packets::game::CPlaceGhostRecipe;

        use crate::inventory::recipe_book_placement::PostPlaceAction;
        use crate::player::recipe_book::{display_for_id, recipe_for_display};

        if !menu.still_valid(self) {
            log::debug!(
                "Player {} interacted with invalid menu",
                self.gameprofile.name()
            );
            return;
        }

        let Some(recipe) = recipe_for_display(packet.recipe) else {
            return;
        };
        if !self.recipe_book.lock().contains(recipe.key()) {
            return;
        }
        // Vanilla `RecipeBookMenu.handlePlacement` bails on recipes whose
        // placement info is empty before touching the grid.
        if recipe
            .data()
            .properties()
            .and_then(|properties| properties.placement.as_ref())
            .is_some_and(|placement| placement.is_impossible_to_place())
        {
            log::debug!(
                "Player {} tried to place impossible recipe {}",
                self.gameprofile.name(),
                recipe.key()
            );
            return;
        }

        let allow_dropping_items_to_clear = self.game_mode() == GameType::Creative;
        let action = {
            // Take the container set before borrowing the kind mutably: the guard
            // must outlive `kind_mut`, so it cannot borrow `menu.behavior()`.
            let refs = menu.behavior().container_refs().to_vec();
            let mut guard = ContainerLockGuard::lock_all(&refs);
            menu.kind_mut().handle_placement(
                &mut guard,
                recipe,
                packet.use_max_items,
                allow_dropping_items_to_clear,
                self,
            )
        };

        if action == Some(PostPlaceAction::PlaceGhostRecipe)
            && let Some(display) = display_for_id(packet.recipe)
        {
            self.send_packet(CPlaceGhostRecipe {
                container_id: packet.container_id,
                recipe_display: display.display.clone(),
            });
        }
    }

    /// Processes a container click on any menu implementing the Menu trait.
    ///
    /// This is the common implementation shared between inventory menu and
    /// external menus (crafting table, chest, etc.).
    fn process_container_click(&self, menu: &mut Menu, packet: SContainerClick) {
        if self.game_mode() == GameType::Spectator || self.get_health() <= 0.0 {
            menu.behavior_mut()
                .send_all_data_to_remote(&self.connection);
            return;
        }

        if !menu.still_valid(self) {
            log::debug!(
                "Player {} interacted with invalid menu",
                self.gameprofile.name()
            );
            return;
        }

        // Vanilla rejects positive out-of-range slots before applying client
        // prediction hashes or resynchronizing the menu. Its validity check
        // admits every negative slot because each is less than the slot count.
        let slot_count = menu.behavior().slot_count();
        let packet_slot_is_valid = packet.slot_num < 0
            || usize::try_from(packet.slot_num).is_ok_and(|slot| slot < slot_count);
        if !packet_slot_is_valid {
            log::debug!(
                "Player {} clicked invalid slot index: {}, available slots: {}",
                self.gameprofile.name(),
                packet.slot_num,
                slot_count
            );
            return;
        }

        // Parse and validate the remaining raw click fields once. A malformed
        // button or drag encoding is not applied, but the state sync below
        // still runs so the client's prediction gets corrected.
        let click = Click::parse(
            packet.slot_num,
            packet.button_num,
            packet.click_type,
            slot_count,
        );
        if click.is_none() {
            log::debug!(
                "Player {} sent malformed container click (slot {}, button {}, {:?})",
                self.gameprofile.name(),
                packet.slot_num,
                packet.button_num,
                packet.click_type
            );
            // Vanilla rejects positive out-of-range slots before `doClick`.
            // Once admitted, any non-QuickCraft input cancels an active drag,
            // and malformed QuickCraft headers/types reset their own state.
            let quick_craft_header = packet.button_num & 3;
            let quick_craft_type = (packet.button_num >> 2) & 3;
            if packet.click_type != ClickType::QuickCraft
                || quick_craft_header == 3
                || (quick_craft_header == 0 && quick_craft_type == 3)
            {
                menu.behavior_mut().reset_quick_craft();
            }
        }

        let full_resync_needed = packet.state_id as u32 != menu.behavior().state_id();

        menu.behavior_mut().suppress_remote_updates();

        if let Some(click) = click {
            menu.clicked(click, self);
        }

        for (slot, hash) in packet.changed_slots {
            let slot = slot as usize;
            // Result/fake slots are server-authoritative (their contents are
            // recomputed from a recipe). Don't let the client's prediction set
            // our view of what it knows, or `broadcast_changes` will think the
            // client already has the freshly-crafted result and skip syncing it
            // — leaving the slot blank until the next click forces a resend.
            if menu
                .behavior()
                .slots()
                .get(slot)
                .is_some_and(|slot| slot.is_fake())
            {
                menu.behavior_mut().mark_remote_slot_unknown(slot);
                continue;
            }
            menu.behavior_mut().set_remote_slot(slot, hash);
        }

        menu.behavior_mut().set_remote_carried(packet.carried_item);
        menu.behavior_mut().resume_remote_updates();

        if full_resync_needed {
            menu.behavior_mut()
                .send_all_data_to_remote(&self.connection);
        } else {
            menu.behavior_mut().broadcast_changes(&self.connection);
        }
    }

    /// Handles a container close packet.
    ///
    /// Based on Java's `ServerGamePacketListenerImpl::handleContainerClose`,
    /// which ignores the packet's container id and closes whatever menu is
    /// currently open. With no external menu open, the always-open inventory
    /// menu is the one being removed.
    pub fn handle_container_close(&self, packet: SContainerClose) {
        log::debug!(
            "Player {} closed container {}",
            self.gameprofile.name(),
            packet.container_id
        );

        if self.has_container_open() {
            self.do_close_container();
            return;
        }

        self.inventory_menu.lock().removed(self);
    }

    /// Handles an anvil rename packet.
    pub fn handle_rename_item(self: &Arc<Self>, packet: SRenameItem) {
        match self.take_open_menu_for_callback(None) {
            Ok(mut menu) => {
                if menu.still_valid(self) {
                    menu.set_item_name(packet.name, self);
                }
                self.finish_open_menu_callback(menu);
            }
            Err(OpenMenuUnavailable::Closed) => {
                log::debug!("rename item without an open menu");
            }
            Err(OpenMenuUnavailable::Unavailable) => {}
        }
    }

    /// Handles a bundle item selection packet, mirroring
    /// `AbstractContainerMenu.setSelectedBundleItemIndex` behind
    /// `ServerGamePacketListenerImpl#handleBundleItemSelectedPacket`.
    ///
    /// Vanilla toggles the selection on the stack in that slot of the currently open menu: an
    /// out-of-range slot or a stack without bundle contents is left untouched, and the selection
    /// is runtime-only state, so nothing is resent.
    pub fn set_selected_bundle_item_index(&self, slot_id: i32, selected_item_index: i32) {
        let Ok(slot_index) = usize::try_from(slot_id) else {
            return;
        };

        match self.take_open_menu_for_callback(None) {
            Ok(mut menu) => {
                Self::toggle_slot_bundle_selection(&mut menu, slot_index, selected_item_index);
                self.finish_open_menu_callback(menu);
            }
            Err(OpenMenuUnavailable::Closed) => {
                let mut menu = self.inventory_menu.lock();
                Self::toggle_slot_bundle_selection(&mut menu, slot_index, selected_item_index);
            }
            Err(OpenMenuUnavailable::Unavailable) => {}
        }
    }

    /// Applies vanilla `BundleItem.toggleSelectedItem` to one menu slot's stack.
    fn toggle_slot_bundle_selection(menu: &mut Menu, slot_index: usize, selected_item_index: i32) {
        let mut guard = menu.behavior().lock_all_containers();
        let Some(slot) = menu.behavior().slots().get(slot_index) else {
            return;
        };
        let mut stack = slot.get_item(&guard).clone();
        let Some(bundle) = stack.get(BUNDLE_CONTENTS) else {
            return;
        };
        stack.set(
            BUNDLE_CONTENTS,
            bundle.with_toggled_selected_item(selected_item_index),
        );
        slot.set_item(&mut guard, stack);
    }

    /// Handles a container slot state changed packet (e.g., crafter slot toggle).
    ///
    /// Mirrors vanilla `ServerGamePacketListenerImpl.handleContainerSlotStateChanged`:
    /// spectator and mismatched container ids are ignored, slots on stale menus
    /// are dropped, and a kind that rejects the toggle leaves the client state
    /// untouched.
    pub fn handle_container_slot_state_changed(&self, packet: SContainerSlotStateChanged) {
        log::debug!(
            "Player {} changed slot {} state to {} in container {}",
            self.gameprofile.name(),
            packet.slot_id,
            packet.new_state,
            packet.container_id
        );
        let Ok(mut menu) = self.take_open_menu_for_callback(Some(packet.container_id)) else {
            return;
        };
        if self.game_mode() == GameType::Spectator {
            self.finish_open_menu_callback(menu);
            return;
        }
        if !menu.still_valid(self) {
            log::debug!(
                "Player {} toggled a slot in an invalid menu",
                self.gameprofile.name()
            );
            self.finish_open_menu_callback(menu);
            return;
        }
        menu.set_slot_state(packet.slot_id, packet.new_state, self, &self.connection);
        self.finish_open_menu_callback(menu);
    }

    /// Handles a creative mode slot set packet.
    pub fn handle_set_creative_mode_slot(&self, packet: SSetCreativeModeSlot) {
        // Vanilla gates on `hasInfiniteMaterials()` (the instabuild ability), not
        // the game mode, so ability edits and restored player data are honored.
        if !self.has_infinite_materials() {
            return;
        }

        let drop = packet.slot_num < 0;
        let item_stack = packet.item_stack;

        let valid_slot = packet.slot_num >= 1 && packet.slot_num <= 45;
        let valid_data = item_stack.is_empty() || item_stack.count <= item_stack.max_stack_size();

        if valid_slot && valid_data {
            let mut menu = self.inventory_menu.lock();
            let slot_index = packet.slot_num as usize;

            {
                let mut guard = menu.behavior().lock_all_containers();
                if let Some(slot) = menu.behavior().slots().get(slot_index) {
                    let previous = slot.get_item(&guard).clone();
                    slot.set_by_player(&mut guard, item_stack.clone(), &previous);
                }
            }
            if (1..=4).contains(&slot_index) {
                menu.update_crafting_result();
            }
            menu.behavior_mut()
                .set_remote_slot_known(slot_index, &item_stack);
            menu.behavior_mut().broadcast_changes(&self.connection);
        } else if drop && valid_data {
            {
                let mut throttler = self.session.drop_spam_throttler.lock();
                if throttler.is_under_threshold() {
                    throttler.increment();
                } else {
                    log::warn!(
                        "Player {} was dropping items too fast in creative mode; ignoring",
                        self.gameprofile.name(),
                    );
                    return;
                }
            }
            let _ = self.drop_item(item_stack, true, Prediction::Predicted);
        }
    }

    /// Sets selected slot
    pub fn handle_set_carried_item(&self, packet: SSetCarriedItem) {
        if self
            .inventory
            .lock()
            .try_set_selected_slot_from_packet(packet.slot)
            .is_err()
        {
            log::warn!(
                "{} tried to set an invalid carried item",
                self.gameprofile.name()
            );
        } else {
            self.reset_last_action_time();
        }
    }

    /// Set the selected advancement
    pub fn handle_seen_advancement(&self, packet: SSeenAdvancement) {
        if let Opened(tab_id) = packet {
            let advancement = REGISTRY.advancements.value_by_key(&tab_id);
            if advancement.is_some() {
                self.advancements.lock().set_selected_tab(self, advancement);
            }
        }
    }

    /// Sends all inventory slots to the client (full sync).
    /// This should be called when the player first joins.
    pub fn send_inventory_to_remote(&self) {
        self.inventory_menu
            .lock()
            .behavior_mut()
            .send_all_data_to_remote(&self.connection);
    }

    /// Generates the next container ID (1-100, wrapping around).
    ///
    /// Based on Java's `ServerPlayer::nextContainerCounter`.
    fn next_container_counter(&self) -> u8 {
        self.container_counter.lock().next()
    }

    /// Opens a menu for this player.
    ///
    /// Based on Java's `ServerPlayer::openMenu`.
    ///
    /// # Arguments
    /// * `title` - The display title shown in the open-screen packet.
    /// * `create` - Factory invoked with the allocated container id, player,
    ///   and current world. If called by a menu hook, the factory runs after
    ///   that hook releases its container locks.
    ///
    /// # Panics
    /// Panics if the created menu uses a different container id than the one
    /// allocated for it, or has no menu type (i.e. the player's own inventory
    /// menu, which must never be opened via `open_menu`).
    pub fn open_menu(
        &self,
        title: impl Into<TextComponent>,
        create: impl for<'a> FnOnce(MenuOpenContext<'a>) -> Menu + Send + 'static,
    ) {
        if !self.begin_menu_open_operation() {
            return;
        }
        self.open_menu_inner(PendingMenuOpen {
            title: title.into(),
            create: Box::new(create),
            screen: MenuOpenScreen::Menu,
        });
        self.finish_menu_open_operation();
    }

    /// Opens a mount inventory screen for this player.
    ///
    /// Based on Java's `ServerPlayer::openHorseInventory`: vanilla 26.3 has no
    /// `horse` menu type, so the client is told the container id, the mount
    /// inventory's column count and the entity id through
    /// `ClientboundMountScreenOpenPacket`, and builds `HorseInventoryMenu`
    /// itself. The factory receives the same [`MenuOpenContext`] as
    /// [`Player::open_menu`].
    ///
    /// # Panics
    /// Panics if the created menu uses a different container id than the one
    /// allocated for it.
    pub fn open_mount_menu(
        &self,
        entity_id: i32,
        inventory_columns: i32,
        create: impl for<'a> FnOnce(MenuOpenContext<'a>) -> Menu + Send + 'static,
    ) {
        if !self.begin_menu_open_operation() {
            return;
        }
        self.open_menu_inner(PendingMenuOpen {
            // The mount packet carries no title; the client uses the entity's
            // display name.
            title: TextComponent::new(),
            create: Box::new(create),
            screen: MenuOpenScreen::Mount {
                entity_id,
                inventory_columns,
            },
        });
        self.finish_menu_open_operation();
    }

    fn open_menu_inner(&self, pending: PendingMenuOpen) {
        self.do_close_container();

        let mut open_menu = self.open_menu.lock();
        if open_menu.terminal_removal.is_some() {
            return;
        }
        if let Some(dispatch) = open_menu.dispatch.as_mut() {
            dispatch
                .actions
                .push(DeferredMenuAction::Open(Box::new(pending)));
            return;
        }
        drop(open_menu);

        self.execute_menu_open(pending);
    }

    fn execute_menu_open(&self, pending: PendingMenuOpen) {
        {
            let mut open_menu = self.open_menu.lock();
            if open_menu.terminal_removal.is_some() {
                return;
            }
            if let Some(dispatch) = open_menu.dispatch.as_mut() {
                dispatch
                    .actions
                    .push(DeferredMenuAction::Open(Box::new(pending)));
                return;
            }
        }

        let PendingMenuOpen {
            title,
            create,
            screen,
        } = pending;
        let container_id = self.next_container_counter();
        let world = self.get_world();
        let menu = create(MenuOpenContext {
            container_id,
            player: self,
            world: &world,
        });
        assert_eq!(
            menu.container_id(),
            container_id,
            "open_menu factory returned container id {}, but {} was allocated",
            menu.container_id(),
            container_id,
        );
        self.open_prepared_menu(title, menu, screen);
    }

    fn open_prepared_menu(&self, title: TextComponent, mut menu: Menu, screen: MenuOpenScreen) {
        loop {
            {
                let mut open_menu = self.open_menu.lock();
                if let Some(terminal_removal) = open_menu.terminal_removal.as_mut() {
                    terminal_removal.pending_menus.push(menu);
                    return;
                }
            }

            // A removal hook may have opened another menu while the initiating
            // open call was closing its predecessor.
            self.do_close_container();

            let mut open_menu = self.open_menu.lock();
            if let Some(terminal_removal) = open_menu.terminal_removal.as_mut() {
                terminal_removal.pending_menus.push(menu);
                return;
            }
            if let Some(dispatch) = open_menu.dispatch.as_mut() {
                dispatch
                    .actions
                    .push(DeferredMenuAction::Install(Box::new(PreparedMenu {
                        title,
                        menu,
                        screen,
                    })));
                return;
            }
            if open_menu.menu.is_some() {
                continue;
            }
            open_menu.dispatch = Some(OpenMenuDispatch {
                container_id: menu.container_id(),
                overrides_player_slots: menu.overrides_player_slots(),
                actions: Vec::new(),
            });
            break;
        }

        match screen {
            MenuOpenScreen::Menu => self.send_packet(COpenScreen {
                container_id: i32::from(menu.container_id()),
                menu_type: menu
                    .menu_type()
                    .expect("a menu opened via open_menu must declare a menu type"),
                title,
            }),
            MenuOpenScreen::Mount {
                entity_id,
                inventory_columns,
            } => self.send_packet(CMountScreenOpen::new(
                i32::from(menu.container_id()),
                inventory_columns,
                entity_id,
            )),
        }

        // Fire on_open before the full sync so anything the menu populates here
        // is included in the first render sent below.
        menu.on_open(self);

        menu.behavior_mut()
            .send_all_data_to_remote(&self.connection);

        self.finish_open_menu_callback(menu);
    }

    /// A shared handle to the 2x2 crafting grid of the always-open inventory
    /// menu.
    pub fn crafting_container(&self) -> Shared<CraftingContainer> {
        let menu = self.inventory_menu.lock();
        let Some(kind) = menu.kind().downcast_ref::<InventoryKind>() else {
            unreachable!("a player's inventory_menu is always the Inventory kind");
        };
        kind.crafting_container()
    }

    /// A shared handler for the 2x2 crafting grid of the always-open inventory
    /// menu and its result.
    pub(crate) fn inventory_crafting_handler(&self) -> CraftingHandler {
        let menu = self.inventory_menu.lock();
        let Some(kind) = menu.kind().downcast_ref::<InventoryKind>() else {
            unreachable!("a player's inventory_menu is always the Inventory kind");
        };
        kind.crafting_handler()
    }

    /// Closes the currently open container and returns to the inventory menu.
    ///
    /// Based on Java's `ServerPlayer::closeContainer`.
    /// This sends a close packet to the client.
    pub fn close_container(&self) {
        self.close_open_menu(true);
    }

    /// Internal close container logic without sending a packet.
    ///
    /// Based on Java's `ServerPlayer::doCloseContainer`.
    /// Called when the client sends a close packet or when opening a new menu.
    pub fn do_close_container(&self) {
        self.close_open_menu(false);
    }

    /// Removes both the base inventory menu and any external menu.
    ///
    /// This mirrors `Player::remove`: base crafting and carried items are
    /// handled before the external menu, and menu hooks cannot install a
    /// replacement while removal is in progress. The inventory menu remains
    /// reusable because Steel keeps one `Player` across world changes.
    pub fn remove_all_menus(&self) -> MenuRemovalStatus {
        self.remove_all_menus_with_disposition(self.default_menu_item_disposition())
    }

    pub(in crate::player) fn remove_all_menus_with_disposition(
        &self,
        disposition: MenuItemDisposition,
    ) -> MenuRemovalStatus {
        let menu = {
            let mut open_menu = self.open_menu.lock();
            if let Some(terminal_removal) = open_menu.terminal_removal.as_mut() {
                terminal_removal.disposition = terminal_removal.disposition.combine(disposition);
                return MenuRemovalStatus::Pending;
            }

            open_menu.terminal_removal = Some(TerminalMenuRemoval {
                disposition,
                main_cleanup_complete: false,
                pending_cleanup_in_progress: false,
                pending_menus: Vec::new(),
            });
            if open_menu.dispatch.is_some() {
                return MenuRemovalStatus::Pending;
            }

            open_menu.menu.take()
        };

        self.finish_terminal_menu_main_cleanup(menu);
        if self.open_menu.lock().terminal_removal.is_none() {
            MenuRemovalStatus::Complete
        } else {
            MenuRemovalStatus::Pending
        }
    }

    fn finish_terminal_menu_main_cleanup(&self, mut menu: Option<Menu>) {
        self.inventory_menu.lock().removed(self);
        if let Some(menu) = menu.as_mut() {
            self.remove_open_menu(menu);
        }

        {
            let mut open_menu = self.open_menu.lock();
            let Some(terminal_removal) = open_menu.terminal_removal.as_mut() else {
                return;
            };
            terminal_removal.main_cleanup_complete = true;
        }
        self.try_finish_terminal_menu_removal();
    }

    fn try_finish_terminal_menu_removal(&self) {
        loop {
            let pending_menus = {
                let mut open_menu = self.open_menu.lock();
                if open_menu.active_open_operations != 0 {
                    return;
                }
                let Some(terminal_removal) = open_menu.terminal_removal.as_mut() else {
                    return;
                };
                if !terminal_removal.main_cleanup_complete {
                    return;
                }
                if terminal_removal.pending_cleanup_in_progress {
                    return;
                }
                if terminal_removal.pending_menus.is_empty() {
                    open_menu.terminal_removal = None;
                    debug_assert!(open_menu.menu.is_none());
                    return;
                }
                terminal_removal.pending_cleanup_in_progress = true;
                mem::take(&mut terminal_removal.pending_menus)
            };

            for mut pending_menu in pending_menus {
                pending_menu.removed(self);
            }

            let mut open_menu = self.open_menu.lock();
            let Some(terminal_removal) = open_menu.terminal_removal.as_mut() else {
                return;
            };
            terminal_removal.pending_cleanup_in_progress = false;
        }
    }

    #[cfg(test)]
    pub(in crate::player) fn retry_terminal_menu_removal_for_test(&self) {
        self.try_finish_terminal_menu_removal();
    }

    fn close_open_menu(&self, send_packet: bool) {
        let menu = {
            let mut open_menu = self.open_menu.lock();
            if open_menu.terminal_removal.is_some() {
                return;
            }
            if let Some(dispatch) = open_menu.dispatch.as_mut() {
                dispatch
                    .actions
                    .push(DeferredMenuAction::Close { send_packet });
                return;
            }
            let Some(menu) = open_menu.menu.take() else {
                return;
            };
            open_menu.dispatch = Some(OpenMenuDispatch {
                container_id: menu.container_id(),
                overrides_player_slots: menu.overrides_player_slots(),
                actions: Vec::new(),
            });
            menu
        };

        let mut menu = menu;
        if send_packet {
            self.send_packet(CContainerClose {
                container_id: i32::from(menu.container_id()),
            });
        }
        self.remove_open_menu(&mut menu);
        self.finish_open_menu_removal();
    }

    fn remove_open_menu(&self, menu: &mut Menu) {
        let overrides_player_slots = menu.overrides_player_slots();
        menu.removed(self);
        if overrides_player_slots {
            self.request_inventory_resync(0..PlayerInventory::INVENTORY_SIZE);
        } else {
            self.inventory_menu
                .lock()
                .behavior_mut()
                .transfer_state(menu.behavior());
        }
    }

    /// Returns true if the player has an external menu open (not the inventory).
    #[must_use]
    pub fn has_container_open(&self) -> bool {
        let open_menu = self.open_menu.lock();
        open_menu.menu.is_some() || open_menu.dispatch.is_some()
    }

    /// Returns whether this player currently has `container` open.
    ///
    /// Mirrors the vanilla `player.containerMenu` container identity check used by
    /// `ContainerOpenersCounter.isOwnContainer`. A combined (double-chest) menu
    /// reports both halves.
    #[must_use]
    pub fn has_open_container(&self, container: &ContainerRef) -> bool {
        let container_id = container.container_id();
        let open_menu = self.open_menu.lock();
        if let Some(menu) = open_menu.menu.as_ref() {
            return menu
                .behavior()
                .container_refs()
                .iter()
                .any(|candidate| candidate.container_id() == container_id);
        }
        if open_menu.dispatch.is_some() {
            return false;
        }
        drop(open_menu);
        self.inventory_menu
            .lock()
            .behavior()
            .container_refs()
            .iter()
            .any(|candidate| candidate.container_id() == container_id)
    }

    /// Returns the container id of the open merchant menu, if one is open.
    ///
    /// Vanilla `ServerPlayer.containerMenu instanceof MerchantMenu` check used by
    /// `Villager.updateSpecialPrices` to refresh the open trade screen.
    #[must_use]
    pub fn trading_container_id(&self) -> Option<u8> {
        let open_menu = self.open_menu.lock();
        let menu = open_menu.menu.as_ref()?;
        menu.kind()
            .downcast_ref::<MerchantMenuKind>()
            .map(|_| menu.container_id())
    }

    /// Runs the open menu's per-tick hook, if an external menu is open.
    ///
    /// Scoped to the opened menu; the base inventory menu is not ticked. Called
    /// once per player tick, before syncing inventory changes to the client.
    pub fn tick_open_menu(&self) {
        let Ok(mut menu) = self.take_open_menu_for_callback(None) else {
            return;
        };
        if !menu.still_valid(self) {
            self.close_container();
            self.finish_open_menu_callback(menu);
            return;
        }
        menu.on_tick(self);
        self.finish_open_menu_callback(menu);
    }

    /// Broadcasts inventory changes to the client (incremental sync).
    /// This is called every tick to sync only changed slots.
    pub fn broadcast_inventory_changes(&self) {
        let mut open_menu = self.open_menu.lock();
        if let Some(menu) = open_menu.menu.as_mut() {
            menu.behavior_mut().broadcast_changes(&self.connection);
            return;
        }
        if open_menu.dispatch.is_none() {
            drop(open_menu);
            self.inventory_menu
                .lock()
                .behavior_mut()
                .broadcast_changes(&self.connection);
        }
    }

    /// Requests direct synchronization of logical player-inventory slots.
    pub(crate) fn request_inventory_resync(&self, slots: impl IntoIterator<Item = usize>) {
        self.inventory_sync.lock().request(slots);
    }

    /// Sends the latest values for requested logical inventory slots.
    pub(in crate::player) fn flush_inventory_resync(&self) {
        let overrides_player_slots = {
            let open_menu = self.open_menu.lock();
            open_menu
                .menu
                .as_ref()
                .is_some_and(Menu::overrides_player_slots)
                || open_menu
                    .dispatch
                    .as_ref()
                    .is_some_and(|dispatch| dispatch.overrides_player_slots)
        };
        let slots = self
            .inventory_sync
            .lock()
            .take_ready(overrides_player_slots);
        if slots.is_empty() {
            return;
        }

        let packets = {
            let inventory = self.inventory.lock();
            slots
                .into_iter()
                .map(|slot| CSetPlayerInventory {
                    slot: slot as i32,
                    item_stack: inventory.get_item(slot).clone(),
                })
                .collect::<Vec<_>>()
        };
        for packet in packets {
            self.send_packet(packet);
        }
    }

    /// Removes or counts matching stacks across every location used by vanilla `/clear`.
    pub(crate) fn clear_or_count_matching_items(
        &self,
        predicate: &dyn Fn(&ItemStack) -> bool,
        amount_to_remove: i32,
    ) -> i32 {
        let counting_only = amount_to_remove == 0;
        let mut count = self.inventory.lock().clear_or_count_matching_items(
            predicate,
            amount_to_remove,
            counting_only,
        );

        count += self.inventory_menu.lock().clear_or_count_crafting_items(
            predicate,
            amount_to_remove - count,
            counting_only,
        );

        let has_open_menu = {
            let mut open_menu = self.open_menu.lock();
            if let Some(menu) = open_menu.menu.as_mut() {
                let behavior = menu.behavior_mut();
                count += clear_or_count_matching_stack(
                    behavior.carried_mut(),
                    predicate,
                    amount_to_remove - count,
                    counting_only,
                );
                if behavior.carried().is_empty() {
                    *behavior.carried_mut() = ItemStack::empty();
                }
                true
            } else {
                open_menu.dispatch.is_some()
            }
        };
        if !has_open_menu {
            let mut inventory_menu = self.inventory_menu.lock();
            let behavior = inventory_menu.behavior_mut();
            count += clear_or_count_matching_stack(
                behavior.carried_mut(),
                predicate,
                amount_to_remove - count,
                counting_only,
            );
            if behavior.carried().is_empty() {
                *behavior.carried_mut() = ItemStack::empty();
            }
        }

        self.inventory_menu.lock().update_crafting_result();
        self.broadcast_inventory_changes();
        count
    }

    /// Drops an item from the player's selected hotbar slot.
    ///
    /// Based on Java's `ServerPlayer.drop(boolean all)`.
    ///
    /// - `all`: If true, drops the entire stack (Ctrl+Q). If false, drops one item (Q).
    pub fn drop_from_selected(&self, all: bool) {
        if !self.can_drop_items() {
            return;
        }

        let removed = {
            let mut inventory = self.inventory.lock();
            let selected_count = inventory.get_selected_item().count();
            inventory.split_item_in_hand(
                InteractionHand::MainHand,
                if all { selected_count } else { 1 },
            )
        };

        let _ = self.drop_item(removed, true, Prediction::Predicted);
        self.reset_attack_strength_ticker();
    }

    /// Swings the main hand and throws `item` in the facing direction.
    ///
    /// Based on Java's `LivingEntity.drop(ItemStack, boolean thrownFromHand, Prediction)`.
    /// `prediction` decides whether the swing is also sent to this player.
    #[must_use]
    pub fn drop_item(
        &self,
        item: ItemStack,
        thrown_from_hand: bool,
        prediction: Prediction,
    ) -> Option<Arc<ItemEntity>> {
        if item.is_empty() {
            return None;
        }

        self.swing(
            InteractionHand::MainHand,
            SwingAnimation::DEFAULT,
            prediction == Prediction::ServerOnly,
        );
        self.spawn_dropped_item(item, false, thrown_from_hand)
    }

    /// Spawns `item` as a dropped entity without swinging.
    ///
    /// Based on Java's `LivingEntity.createItemStackToDrop`, used directly by
    /// death drops and `/give` overflow.
    ///
    /// - `throw_randomly`: If true, the item is thrown in a random direction.
    ///   If false, it's thrown in the direction the player is facing.
    /// - `thrown_from_hand`: If true, sets the thrower and uses a longer pickup delay.
    #[must_use]
    pub fn spawn_dropped_item(
        &self,
        item: ItemStack,
        throw_randomly: bool,
        thrown_from_hand: bool,
    ) -> Option<Arc<ItemEntity>> {
        if item.is_empty() {
            return None;
        }

        let pos = self.position();
        let (yaw, pitch) = self.rotation();

        let spawn_y = self.get_eye_y() - 0.3;

        let velocity = if throw_randomly {
            let power = rand::random_range(0.0..0.5);
            let angle = rand::random_range(0.0..TAU);
            DVec3::new(
                f64::from(-angle.sin() * power),
                0.2,
                f64::from(angle.cos() * power),
            )
        } else {
            let pitch_rad = pitch.to_radians();
            let yaw_rad = yaw.to_radians();

            let sin_pitch = pitch_rad.sin();
            let cos_pitch = pitch_rad.cos();
            let sin_yaw = yaw_rad.sin();
            let cos_yaw = yaw_rad.cos();

            let angle_offset = rand::random_range(0.0..TAU);
            let power_offset = rand::random_range(0.0..0.02);

            DVec3::new(
                f64::from(-sin_yaw * cos_pitch * 0.3)
                    + f64::from(angle_offset.cos() * power_offset),
                f64::from(-sin_pitch * 0.3 + 0.1)
                    + f64::from((rand::random::<f32>() - rand::random::<f32>()) * 0.1),
                f64::from(cos_yaw * cos_pitch * 0.3) + f64::from(angle_offset.sin() * power_offset),
            )
        };

        let spawn_pos = DVec3::new(pos.x, spawn_y, pos.z);

        let item_ref = item.item;
        let item_count = item.count;

        let entity = self
            .get_world()
            .spawn_item_with_velocity(spawn_pos, item, velocity)?;
        entity.set_pickup_delay(40);
        if thrown_from_hand {
            entity.set_thrower(self.gameprofile.id());
            self.award_stat_with_count(&vanilla_stat_types::ITEM_DROPPED, item_ref, item_count);
            self.award_custom_stat(&vanilla_custom_stats::DROP);
        }
        Some(entity)
    }

    /// Returns true if the player can drop items.
    ///
    /// Based on Java's `Player.canDropItems()`.
    /// Returns false if the player is dead, removed, or has a flag preventing item drops.
    #[must_use]
    pub fn can_drop_items(&self) -> bool {
        !self.is_removed()
        // TODO: Check if player is alive (health > 0)
    }

    /// Returns whether items from a closing menu (crafting grid, anvil inputs,
    /// cursor) should be placed back into the inventory instead of dropped into
    /// the world.
    ///
    /// Matches vanilla's `AbstractContainerMenu.dropOrPlaceInInventory`: a
    /// disconnected player or one removed for any reason except a world change
    /// drops the items.
    #[must_use]
    pub fn returns_menu_items_to_inventory(&self) -> bool {
        let terminal_disposition = self
            .open_menu
            .lock()
            .terminal_removal
            .as_ref()
            .map(|terminal_removal| terminal_removal.disposition);
        if let Some(disposition) = terminal_disposition {
            return disposition == MenuItemDisposition::ReturnToInventory;
        }

        self.default_menu_item_disposition() == MenuItemDisposition::ReturnToInventory
    }

    fn default_menu_item_disposition(&self) -> MenuItemDisposition {
        let removed_outside_world_change =
            self.is_removed() && self.removal_reason() != Some(RemovalReason::ChangedWorld);
        if removed_outside_world_change || self.connection.closed() || self.get_health() <= 0.0 {
            MenuItemDisposition::Drop
        } else {
            MenuItemDisposition::ReturnToInventory
        }
    }

    /// Tries to add an item to the player's inventory, playing the vanilla item-pickup
    /// sound when the entire stack fits.
    ///
    /// Returns true if the whole stack was added; otherwise the stack keeps the remainder.
    /// Mirrors the shared vanilla `Inventory.add` + `SoundEvents.ITEM_PICKUP` flow used by
    /// `/give` and advancement rewards: the sound only plays once the stack is fully consumed.
    pub fn add_item_with_sound(&self, stack: &mut ItemStack) -> bool {
        let added = self.inventory.lock().add(stack);
        if added {
            let pitch = ((rand::random::<f32>() - rand::random::<f32>()) * 0.7 + 1.0) * 2.0;
            self.get_world().play_sound_at(
                &sound_events::ENTITY_ITEM_PICKUP,
                SoundSource::Players,
                self.position(),
                0.2,
                pitch,
                None,
            );
        }
        added
    }

    /// Tries to add an item to the player's inventory, dropping it if it doesn't fit.
    ///
    /// Based on Java's `Inventory.placeItemBackInInventory`.
    pub fn add_item_or_drop(&self, mut item: ItemStack, prediction: Prediction) {
        if item.is_empty() {
            return;
        }

        let added = self.inventory.lock().add(&mut item);
        if !added || !item.is_empty() {
            let _ = self.drop_item(item, false, prediction);
        }
    }

    /// Tries to add an item to the player's inventory using an existing lock guard,
    /// dropping it if it doesn't fit.
    ///
    /// Use this variant when you already hold a `ContainerLockGuard` that includes
    /// the player's inventory to avoid deadlocks.
    pub fn add_item_or_drop_with_guard(
        &self,
        guard: &mut ContainerLockGuard,
        mut item: ItemStack,
        prediction: Prediction,
    ) {
        if item.is_empty() {
            return;
        }

        let inv_id = ContainerId::from_arc(&self.inventory);
        let should_drop = if let Some(inv) = guard.get_mut(inv_id) {
            let added = inv.add(&mut item);
            !added || !item.is_empty()
        } else {
            true
        };
        if should_drop {
            let _ = guard.run_unlocked(|| self.drop_item(item, false, prediction));
        }
    }
}

/// Converts a trading `ItemCost` into its wire representation.
fn wire_item_cost(cost: &ItemCost) -> MerchantItemCost {
    MerchantItemCost {
        item: cost.item(),
        count: cost.count(),
        components: cost.components().clone(),
    }
}
