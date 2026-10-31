//! Contains the Menu API

mod behavior;
mod builder;
mod grid;
mod kind;
pub mod kinds;
mod layout;

use crate::player::player_inventory::Prediction;
use std::sync::Arc;

use crate::inventory::container::Container as _;
pub use behavior::MenuBehavior;
pub use builder::{
    ContainerSlots, DataSlot, FakeResultRemainderPolicy, FillDirection, IntoSections, MenuBuilder,
    PlayerInventorySections, Section, SectionKind, SectionSource, SlotFactory,
};
pub use grid::{ColSpan, GridPlacer, PlacementBuilder, Rect, Region, RowSpan, SpanBounds};
pub use kind::MenuKind;
pub(crate) use layout::MenuLayout;
#[cfg(test)]
use steel_utils::locks::Shared;

use std::fmt;
use std::mem;

use steel_registry::{item_stack::ItemStack, menu_type::MenuTypeRef, mob_effect::MobEffectRef};
use steel_utils::Downcast as _;

use crate::block_entity::ContainerViewers;
use crate::inventory::container::CraftingContainer;
use crate::inventory::menu::kinds::InventoryKind;
use crate::{
    inventory::lock::{ContainerId, ContainerLockGuard, ContainerRef},
    player::{Player, PlayerConnection},
};

use crate::behavior::ITEM_BEHAVIORS;
use crate::inventory::click::{
    Click, ClickAction, ClickOutcome, SwapTarget, can_item_quick_replace,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::inventory::slots::{ArmorSlot, Slot};

/// A menu opened by a player: the shared click machinery plus one
/// [`MenuKind`].
///
/// The single concrete menu type. It owns the [`MenuBehavior`], the
/// `MenuLayout`, and a boxed [`MenuKind`]. Click handlers are inherent methods.
pub struct Menu {
    behavior: MenuBehavior,
    layout: MenuLayout,
    kind: Box<dyn MenuKind>,
    overrides_player_slots: bool,
}

impl fmt::Debug for Menu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Menu")
            .field("behavior", &self.behavior)
            .field("kind", &self.kind.downcast_type_key())
            .finish_non_exhaustive()
    }
}

impl Menu {
    /// Assembles a menu from its parts.
    pub(super) fn from_parts(
        behavior: MenuBehavior,
        layout: MenuLayout,
        kind: Box<dyn MenuKind>,
        overrides_player_slots: bool,
    ) -> Self {
        Self {
            behavior,
            layout,
            kind,
            overrides_player_slots,
        }
    }

    /// Returns a reference to the shared menu behavior.
    #[must_use]
    pub const fn behavior(&self) -> &MenuBehavior {
        &self.behavior
    }

    /// Returns a mutable reference to the shared menu behavior.
    pub const fn behavior_mut(&mut self) -> &mut MenuBehavior {
        &mut self.behavior
    }

    /// Returns a reference to this menu's kind.
    #[must_use]
    pub fn kind(&self) -> &dyn MenuKind {
        self.kind.as_ref()
    }

    /// Returns a mutable reference to this menu's kind.
    pub fn kind_mut(&mut self) -> &mut dyn MenuKind {
        self.kind.as_mut()
    }

    /// The container ID for this menu (0 for the player inventory).
    #[must_use]
    pub const fn container_id(&self) -> u8 {
        self.behavior.container_id()
    }

    /// The menu type for the open-screen packet, or `None` for the player's own
    /// inventory.
    #[must_use]
    pub const fn menu_type(&self) -> Option<MenuTypeRef> {
        self.behavior.menu_type()
    }

    /// Returns whether this menu paints over the client's standard player slots.
    #[must_use]
    pub const fn overrides_player_slots(&self) -> bool {
        self.overrides_player_slots
    }

    /// Returns true if this menu is still valid for the player.
    #[must_use]
    pub fn still_valid(&self, player: &Player) -> bool {
        self.kind.still_valid(&self.behavior, player)
    }

    /// Returns true if the item can be taken from the slot during pickup all.
    #[must_use]
    pub fn can_take_item_for_pick_all(&self, carried: &ItemStack, slot_index: usize) -> bool {
        self.kind.can_take_item_for_pick_all(carried, slot_index)
    }

    /// Forwards a beacon effect selection from the set-beacon packet.
    pub fn update_effects(
        &mut self,
        primary: Option<MobEffectRef>,
        secondary: Option<MobEffectRef>,
        connection: &Arc<PlayerConnection>,
    ) -> bool {
        let accepted = {
            let mut guard = self.behavior.lock_all_containers();
            self.kind
                .on_update_effects(&mut self.behavior, &mut guard, primary, secondary)
        };
        self.behavior.broadcast_changes(connection);
        accepted
    }

    /// Forwards a container button click (enchant/loom/stonecutter/lectern selection).
    ///
    /// Mirrors vanilla `AbstractContainerMenu.clickMenuButton`. Returns `false`
    /// when no kind accepted the button, so the caller can treat it as invalid.
    pub fn click_menu_button(
        &mut self,
        button_id: i32,
        player: &Player,
        connection: &Arc<PlayerConnection>,
    ) -> bool {
        let accepted = {
            let mut guard = self.behavior.lock_all_containers();
            let Self { behavior, kind, .. } = self;
            kind.on_button_clicked(behavior, &mut guard, button_id, player)
        };
        // Vanilla re-sends contents only for an accepted button.
        if accepted {
            self.behavior.broadcast_changes(connection);
        }
        accepted
    }

    /// Forwards a slot-state toggle (crafter slot disabling).
    ///
    /// Mirrors vanilla `AbstractContainerMenu.setSlotState` reached from
    /// `ServerGamePacketListenerImpl.handleContainerSlotStateChanged`. Returns
    /// `false` when no kind accepted it, matching vanilla's `instanceof`
    /// rejection.
    pub fn set_slot_state(
        &mut self,
        slot_id: i32,
        new_state: bool,
        player: &Player,
        connection: &Arc<PlayerConnection>,
    ) -> bool {
        let accepted = {
            let mut guard = self.behavior.lock_all_containers();
            let Self { behavior, kind, .. } = self;
            kind.on_slot_state_changed(behavior, &mut guard, slot_id, new_state, player)
        };
        if accepted {
            self.behavior.broadcast_changes(connection);
        }
        accepted
    }

    /// Called when the menu is closed. Hands the carried item and input sections
    /// back to the player, then runs the kind's cleanup. Items drop into the
    /// world if the player can't take them (see
    /// [`Player::returns_menu_items_to_inventory`]).
    pub fn removed(&mut self, player: &Player) {
        // Vanilla `stopOpen` runs before the menu hands items back.
        self.for_each_viewed_block_entity(player, |viewers, player| viewers.stop_open(player));

        let return_to_inventory = player.returns_menu_items_to_inventory();

        let carried = mem::take(self.behavior.carried_mut());
        if !carried.is_empty() {
            if return_to_inventory {
                player.add_item_or_drop(carried, Prediction::ServerOnly);
            } else {
                let _ = player.drop_item(carried, false, Prediction::ServerOnly);
            }
        }
        self.layout
            .return_drained_items(&self.behavior, player, return_to_inventory);

        let Self { behavior, kind, .. } = self;
        kind.removed(behavior, player);
    }

    /// Applies a client rename to the menu's kind. A no-op for kinds without a
    /// rename input.
    pub fn set_item_name(&mut self, name: impl Into<String>, player: &Player) {
        let Self { behavior, kind, .. } = self;
        kind.on_rename(behavior, name.into(), player);
    }

    /// Clears or counts crafting-grid items in the base inventory menu,
    /// returning the number cleared or counted. Returns 0 for any other menu.
    pub(crate) fn clear_or_count_crafting_items(
        &mut self,
        predicate: &dyn Fn(&ItemStack) -> bool,
        amount_to_remove: i32,
        counting_only: bool,
    ) -> i32 {
        let Some(kind) = self.kind.downcast_ref::<InventoryKind>() else {
            return 0;
        };
        let crafting_id = kind.crafting_id();
        let mut guard = self.behavior.lock_all_containers();
        let Some(crafting) = guard.get_typed_mut::<CraftingContainer>(crafting_id) else {
            return 0;
        };

        crafting.clear_or_count_matching_items(predicate, amount_to_remove, counting_only)
    }

    /// A shared handle to the base inventory menu's 2x2 crafting grid, or `None`
    /// for any other menu.
    #[cfg(test)]
    pub(crate) fn crafting_container(&self) -> Option<Shared<CraftingContainer>> {
        let kind = self.kind.downcast_ref::<InventoryKind>()?;
        Some(kind.crafting_container())
    }

    /// Recomputes the base inventory menu's crafting result. A no-op for any
    /// other menu.
    pub(crate) fn update_crafting_result(&mut self) {
        let Some(kind) = self.kind.downcast_mut::<InventoryKind>() else {
            return;
        };
        let mut guard = self.behavior.lock_all_containers();
        kind.update_result(&mut guard);
    }

    /// Recomputes recipe-driven slots after a change (delegates to the kind).
    fn slots_changed(&mut self, guard: &mut ContainerLockGuard, player: &Player) {
        let Self { behavior, kind, .. } = self;
        kind.slots_changed(behavior, guard, player);
    }

    /// Runs the kind's `on_open` hook, after contents are built but before they
    /// are sent to the client.
    pub fn on_open(&mut self, player: &Player) {
        {
            let mut guard = self.behavior().lock_all_containers();
            let Self { behavior, kind, .. } = self;
            kind.on_open(behavior, &mut guard, player);
        }
        // Vanilla `startOpen` runs once the menu is installed, for every
        // container the menu exposes.
        self.for_each_viewed_block_entity(player, |viewers, player| viewers.start_open(player));
    }

    /// Runs `action` for each distinct block entity backing this menu's slots.
    ///
    /// Vanilla's `AbstractContainerMenu` drives `Container.startOpen`/`stopOpen`
    /// from the menu lifecycle rather than from each menu subclass, so this
    /// covers every block-entity-backed container uniformly.
    fn for_each_viewed_block_entity(
        &self,
        player: &Player,
        mut action: impl FnMut(&dyn ContainerViewers, &Player),
    ) {
        let mut seen = Vec::new();
        for container_ref in self.behavior.container_refs() {
            let id = container_ref.container_id();
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            let Some(entity) = container_ref.owner_block_entity() else {
                continue;
            };
            let Some(viewers) = entity.container_viewers() else {
                continue;
            };
            action(viewers, player);
        }
    }

    /// Runs the kind's `on_tick` hook. Called once per server tick while open.
    pub fn on_tick(&mut self, player: &Player) {
        let mut guard = self.behavior().lock_all_containers();
        let Self { behavior, kind, .. } = self;
        kind.on_tick(behavior, &mut guard, player);
    }

    /// Shift-click (quick move) for a slot: the kind's override if any, else the
    /// declarative route table. Returns the item originally in the slot, or
    /// empty if nothing moved.
    fn quick_move_stack(
        &mut self,
        guard: &mut ContainerLockGuard,
        slot_index: usize,
        player: &Player,
    ) -> ItemStack {
        let Self {
            behavior,
            layout,
            kind,
            ..
        } = self;
        if let Some(result) = kind.quick_move(behavior, guard, slot_index, player) {
            result
        } else {
            layout.quick_move(behavior, guard, slot_index, player)
        }
    }

    /// Handles a click action in this menu. Packet clicks are validated via
    /// [`Click::parse`]; invalid programmatically constructed clicks are ignored.
    pub fn clicked(&mut self, click: Click, player: &Player) {
        if !click.is_valid_for(self.behavior().slot_count()) {
            log::debug!(
                "Ignoring programmatic container click that violates parsed-click invariants: \
                 {click:?}"
            );
            return;
        }

        let armor_before = self.armor_snapshot();

        // Vanilla's `doClick` consults the ability flag, not the game mode.
        let has_infinite_materials = player.has_infinite_materials();
        if let Click::QuickCraft(action) = click {
            let outcome = {
                let mut guard = self.behavior().lock_all_containers();
                let Self { behavior, kind, .. } = self;
                kind.on_drag(behavior, &mut guard, action, player)
            };
            if outcome == ClickOutcome::Consume {
                self.behavior_mut().reset_quick_craft();
            } else {
                let Self { behavior, kind, .. } = self;
                behavior.do_quick_craft(action, has_infinite_materials, player, &|slot| {
                    kind.can_drag_to(slot)
                });
            }
        } else {
            // Vanilla's `doClick` chain: a non-drag click that arrives while a
            // drag is active only clears the drag state — the click itself is
            // consumed and never applied.
            if self.behavior().quickcraft().is_some() {
                self.behavior_mut().reset_quick_craft();
                return;
            }

            // Menu-defined click hook. A consumed click skips default handling.
            // The guard is dropped before the default arms re-lock the same containers.
            let outcome = {
                let mut guard = self.behavior().lock_all_containers();
                let Self { behavior, kind, .. } = self;
                kind.on_slot_clicked(behavior, &mut guard, click, player)
            };

            if outcome == ClickOutcome::Fallthrough
                && !self.try_item_click_behaviour_override(click, player)
            {
                match click {
                    Click::Pickup { slot, button } => {
                        self.behavior_mut().do_pickup(slot, button, player);
                    }
                    Click::DropCarried { button } => {
                        self.behavior_mut().drop_carried(button, player);
                    }
                    Click::QuickMove { slot } => {
                        self.do_quick_move(slot, player);
                    }
                    Click::Swap { slot, with } => {
                        self.do_swap(slot, with, player);
                    }
                    Click::Clone { slot } => {
                        self.behavior_mut().do_clone(slot, has_infinite_materials);
                    }
                    Click::Throw { slot, whole_stack } => {
                        self.behavior_mut().do_throw(slot, whole_stack, player);
                    }
                    Click::PickupAll { slot, direction } => {
                        self.do_pickup_all(slot, direction, player);
                    }
                    Click::QuickCraft(_) => unreachable!(),
                }
            }
        }
        // Recompute recipe-driven slots after the click. A QuickCraft drag has
        // no slot on its end phase, so recompute on any non-empty menu.
        let should_recompute = match click {
            Click::DropCarried { .. } => false,
            Click::QuickCraft(_) => !self.behavior().slots().is_empty(),
            _ => true,
        };
        if should_recompute {
            let mut guard = self.behavior().lock_all_containers();
            self.slots_changed(&mut guard, player);
        }

        // Vanilla `ArmorSlot.setByPlayer` runs `onEquipItem` for every
        // player-driven armor write. Steel keeps the click machinery slot-
        // agnostic, so the armor slots are diffed once after the action.
        self.publish_equipment_changes(&armor_before, player);

        // Every container lock is released here, so kinds that publish menu
        // state into entity-owned data (the mount menu's equipment mirrors)
        // can safely touch the entity.
        self.kind_mut().after_click(player);
    }

    /// The armor slots' current equipment, captured before a player action.
    fn armor_snapshot(&self) -> Vec<(usize, EquipmentSlot, ItemStack)> {
        let guard = self.behavior().lock_all_containers();
        self.behavior()
            .slots()
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let armor = slot.downcast_ref::<ArmorSlot>()?;
                Some((
                    index,
                    armor.equipment_slot(),
                    armor.get_item(&guard).clone(),
                ))
            })
            .collect()
    }

    /// Runs vanilla `ArmorSlot.setByPlayer`'s `onEquipItem` for armor slots
    /// whose stack changed during the action.
    fn publish_equipment_changes(
        &self,
        snapshot: &[(usize, EquipmentSlot, ItemStack)],
        player: &Player,
    ) {
        if snapshot.is_empty() {
            return;
        }
        let guard = self.behavior().lock_all_containers();
        for (index, equipment_slot, previous) in snapshot {
            let current = self.behavior().slots()[*index].get_item(&guard);
            player.on_equip_item(*equipment_slot, previous, current);
        }
    }

    /// Vanilla `AbstractContainerMenu.tryItemClickBehaviourOverride`.
    ///
    /// Only mouse clicks carry a `PRIMARY`/`SECONDARY` action; every other
    /// click kind maps to a vanilla action no item override observes, so it is
    /// skipped here.
    fn try_item_click_behaviour_override(&mut self, click: Click, player: &Player) -> bool {
        let Click::Pickup { slot, button } = click else {
            return false;
        };
        let action = ClickAction::from_button(button);
        let mut guard = self.behavior().lock_all_containers();
        let slot_ref = &self.behavior().slots()[slot];

        // Vanilla asks the carried stack first, then the stack in the slot.
        let mut carried = self.behavior().carried().clone();
        let carried_behavior = ITEM_BEHAVIORS.get_behavior(carried.item());
        if carried_behavior.override_stacked_on_other(
            &mut carried,
            slot_ref.as_ref(),
            &mut guard,
            action,
            player,
        ) {
            *self.behavior_mut().carried_mut() = carried;
            return true;
        }

        let slot_stack = slot_ref.get_item(&guard).clone();
        let mut next_slot_stack = slot_stack.clone();
        let slot_behavior = ITEM_BEHAVIORS.get_behavior(slot_stack.item());
        let handled = slot_behavior.override_other_stacked_on_me(
            &mut next_slot_stack,
            &mut carried,
            slot_ref.as_ref(),
            &mut guard,
            action,
            player,
        );
        if handled {
            slot_ref.set_item(&mut guard, next_slot_stack);
        }
        // The slot override may only clear the bundle selection, so the carried
        // stack is written back even when the click fell through.
        *self.behavior_mut().carried_mut() = carried;
        handled
    }

    /// Handles quick move (shift-click).
    fn do_quick_move(&mut self, slot_index: usize, player: &Player) {
        let mut guard = self.behavior().lock_all_containers();

        if !self.behavior().slots()[slot_index].may_pickup(&guard, player) {
            return;
        }

        let initial_item = self.behavior().slots()[slot_index].get_item(&guard).clone();
        if initial_item.is_empty() {
            return;
        }

        // Loop while the slot still holds the same item type.
        let mut result = self.quick_move_stack(&mut guard, slot_index, player);

        while !result.is_empty() {
            let current_item = self.behavior().slots()[slot_index].get_item(&guard).clone();
            if !ItemStack::is_same_item(&current_item, &result) {
                break;
            }
            result = self.quick_move_stack(&mut guard, slot_index, player);
        }
    }

    /// Handles swap (number keys for a hotbar slot, or swap-hands for the
    /// offhand).
    fn do_swap(&mut self, slot_index: usize, with: SwapTarget, player: &Player) {
        let player_inventory = ContainerRef::from(Arc::clone(&player.inventory));
        let player_inv_id = ContainerId::from_arc(&player.inventory);
        let mut guard = self.behavior().lock_all_containers_with(player_inventory);

        let behavior = self.behavior();
        let target_slot = &behavior.slots()[slot_index];
        let inventory_slot = with.inventory_slot();

        let target_item = target_slot.get_item(&guard).clone();
        let Some(inventory) = guard.get(player_inv_id) else {
            unreachable!("the explicitly locked player inventory must be present");
        };
        let source_item = inventory.get_item(inventory_slot).clone();

        if source_item.is_empty() && target_item.is_empty() {
            return;
        }

        if source_item.is_empty() {
            // Move target -> inventory.
            if target_slot.may_pickup(&guard, player) {
                let Some(inventory) = guard.get_mut(player_inv_id) else {
                    unreachable!("the explicitly locked player inventory must be present");
                };
                inventory.set_item(inventory_slot, target_item.clone());
                target_slot.set_by_player(&mut guard, ItemStack::empty(), &target_item);
                if let Some(remainder) = target_slot.on_take(&mut guard, &target_item, player) {
                    player.add_item_or_drop_with_guard(
                        &mut guard,
                        remainder,
                        Prediction::Predicted,
                    );
                }
            }
        } else if target_item.is_empty() {
            // Move inventory -> target.
            if target_slot.may_place(&source_item) {
                let max_size = target_slot.get_max_stack_size_for_item(&guard, &source_item);
                if source_item.count > max_size {
                    let Some(inv) = guard.get_mut(player_inv_id) else {
                        unreachable!("the explicitly locked player inventory must be present");
                    };
                    let to_place = inv.get_item_mut(inventory_slot).split(max_size);
                    target_slot.set_by_player(&mut guard, to_place, &ItemStack::empty());
                } else {
                    let Some(inventory) = guard.get_mut(player_inv_id) else {
                        unreachable!("the explicitly locked player inventory must be present");
                    };
                    inventory.set_item(inventory_slot, ItemStack::empty());
                    target_slot.set_by_player(&mut guard, source_item, &ItemStack::empty());
                }
            }
        } else {
            // Swap target <-> inventory.
            if target_slot.may_pickup(&guard, player) && target_slot.may_place(&source_item) {
                let max_size = target_slot.get_max_stack_size_for_item(&guard, &source_item);
                if source_item.count > max_size {
                    // Source too big: place a partial stack, return target to inventory.
                    let Some(inv) = guard.get_mut(player_inv_id) else {
                        unreachable!("the explicitly locked player inventory must be present");
                    };
                    let to_place = inv.get_item_mut(inventory_slot).split(max_size);
                    target_slot.set_by_player(&mut guard, to_place, &target_item);
                    if let Some(remainder) = target_slot.on_take(&mut guard, &target_item, player) {
                        player.add_item_or_drop_with_guard(
                            &mut guard,
                            remainder,
                            Prediction::Predicted,
                        );
                    }
                    let mut displaced = target_item;
                    let Some(inventory) = guard.get_mut(player_inv_id) else {
                        unreachable!("the explicitly locked player inventory must be present");
                    };
                    let added = inventory.add(&mut displaced);
                    // Vanilla's Inventory::add consumes uninserted stacks in creative mode.
                    if !added && !player.has_infinite_materials() {
                        let _ = guard.run_unlocked(|| {
                            player.drop_item(displaced, true, Prediction::Predicted)
                        });
                    }
                } else {
                    let Some(inventory) = guard.get_mut(player_inv_id) else {
                        unreachable!("the explicitly locked player inventory must be present");
                    };
                    inventory.set_item(inventory_slot, target_item.clone());
                    target_slot.set_by_player(&mut guard, source_item, &target_item);
                    if let Some(remainder) = target_slot.on_take(&mut guard, &target_item, player) {
                        player.add_item_or_drop_with_guard(
                            &mut guard,
                            remainder,
                            Prediction::Predicted,
                        );
                    }
                }
            }
        }
    }

    /// Handles pickup all (double-click): collects matching items from all slots
    /// into the carried stack.
    fn do_pickup_all(&mut self, slot_index: usize, direction: FillDirection, player: &Player) {
        let mut guard = self.behavior().lock_all_containers();

        let behavior = self.behavior();
        let slot = &behavior.slots()[slot_index];
        let slot_has_item = !slot.get_item(&guard).is_empty();
        let slot_may_pickup = slot.may_pickup(&guard, player);

        if behavior.carried().is_empty() || (slot_has_item && slot_may_pickup) {
            return;
        }

        let max_stack = behavior.carried().max_stack_size();
        let carried_item = behavior.carried().clone();
        let slot_count = behavior.slots().len();

        let (start, step): (i32, i32) = match direction {
            FillDirection::Forward => (0, 1),
            FillDirection::Backward => (slot_count as i32 - 1, -1),
        };

        // First pass collects non-full stacks, second pass the full ones.
        for pass in 0..2 {
            let mut i = start;
            while i >= 0 && i < slot_count as i32 && self.behavior().carried().count < max_stack {
                let target_slot = &self.behavior().slots()[i as usize];
                let target_item = target_slot.get_item(&guard).clone();

                if !target_item.is_empty()
                    && can_item_quick_replace(&target_item, &carried_item, true)
                    && target_slot.may_pickup(&guard, player)
                    && self.can_take_item_for_pick_all(&carried_item, i as usize)
                {
                    // First pass skips full stacks, second pass includes them.
                    if pass != 0 || target_item.count != target_item.max_stack_size() {
                        let can_take = max_stack - self.behavior().carried().count;
                        let to_take = target_item.count.min(can_take);
                        let removed = target_slot.safe_take(&mut guard, to_take, can_take, player);
                        self.behavior_mut()
                            .carried_mut()
                            .grow(removed.count.min(can_take));
                    }
                }

                i += step;
            }
        }
    }
}

#[cfg(test)]
mod tests;
