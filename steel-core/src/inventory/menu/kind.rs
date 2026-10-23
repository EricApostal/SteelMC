//! Per-menu behavior hooks.

use steel_registry::item_stack::ItemStack;
use steel_registry::mob_effect::MobEffectRef;
use steel_utils::ErasedType;

use crate::inventory::menu::behavior::MenuBehavior;
use crate::{inventory::lock::ContainerLockGuard, player::Player};

use crate::inventory::click::{Click, ClickOutcome, QuickCraft};

/// Per-menu behavior that isn't shared: recompute-on-change, validity, close
/// cleanup, and the optional shift-click override.
///
/// Menu transitions requested while a hook owns mutable access to the current
/// menu are applied after that hook returns.
///
/// Concrete implementations must claim a unique
/// [`steel_utils::DowncastTypeKey`] through [`steel_utils::DowncastType`].
pub trait MenuKind: ErasedType + Send + Sync {
    /// Recompute recipe-driven slots after a click touched a real slot.
    fn slots_changed(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
    }

    /// Extra cleanup on close beyond [`Menu::removed`].
    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {}

    /// Applies a rename from the client (anvil-style text input) and recomputes
    /// any result. No-op for kinds without a rename input.
    fn on_rename(&mut self, _behavior: &mut MenuBehavior, _name: String, _player: &Player) {}

    /// Runs after initial contents are built but before they're sent, so
    /// anything populated here appears in the first render.
    fn on_open(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
    }

    /// Runs once per tick per viewer while open, before changes are synced.
    fn on_tick(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
    }

    /// Runs for every non-drag click before default handling. Return
    /// [`ClickOutcome::Consume`] to treat the slot as a button, or
    /// [`ClickOutcome::Fallthrough`] for default handling.
    fn on_slot_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _click: Click,
        _player: &Player,
    ) -> ClickOutcome {
        ClickOutcome::Fallthrough
    }

    /// Runs after a click has been fully applied and every container lock has
    /// been released.
    ///
    /// For kinds that publish menu state into entity-owned data — the mount
    /// menu mirrors its equipment slots into the entity's equipment — this is
    /// the only safe point to touch the entity: the container contract forbids
    /// world/entity access while the container mutex is held.
    fn after_click(&mut self, _player: &Player) {}

    /// Runs for each drag phase before default handling. Return
    /// [`ClickOutcome::Consume`] to cancel the drag.
    fn on_drag(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _action: QuickCraft,
        _player: &Player,
    ) -> ClickOutcome {
        ClickOutcome::Fallthrough
    }

    /// Returns true if a drag may distribute items into `slot_index`.
    fn can_drag_to(&self, _slot_index: usize) -> bool {
        true
    }

    /// Returns true if this menu is still valid for the player.
    fn still_valid(&self, _behavior: &MenuBehavior, _player: &Player) -> bool {
        true
    }

    /// Returns true if an item may be taken from `slot_index` during pickup-all.
    fn can_take_item_for_pick_all(&self, _carried: &ItemStack, _slot_index: usize) -> bool {
        true
    }

    /// Applies a recipe-book placement (vanilla `RecipeBookMenu.handlePlacement`).
    ///
    /// Returns `None` for menus that are not recipe-book menus, so the caller can
    /// reject the request the way vanilla only handles `RecipeBookMenu`s.
    fn handle_placement(
        &mut self,
        _guard: &mut ContainerLockGuard,
        _recipe: steel_registry::recipe::UntypedRecipeRef,
        _use_max_items: bool,
        _allow_dropping_items_to_clear: bool,
        _player: &Player,
    ) -> Option<crate::inventory::recipe_book_placement::PostPlaceAction> {
        None
    }

    /// Handles a container button click from the client's container-button packet.
    ///
    /// Mirrors vanilla `AbstractContainerMenu.clickMenuButton`. `button_id` carries
    /// the screen-specific selection (enchant option index, stonecutter recipe
    /// index, loom pattern index, lectern page command). Returns `true` when the
    /// button was accepted; `false` makes the caller treat it as invalid.
    fn on_button_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _button_id: i32,
        _player: &Player,
    ) -> bool {
        false
    }

    /// Handles a slot-state toggle from the client (crafter slot disabling).
    ///
    /// Mirrors vanilla `AbstractContainerMenu.setSlotState`, reached from
    /// `ServerGamePacketListenerImpl.handleContainerSlotStateChanged`. Returns
    /// `true` when the toggle was accepted.
    fn on_slot_state_changed(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _slot_id: i32,
        _new_state: bool,
        _player: &Player,
    ) -> bool {
        false
    }

    /// Handles a beacon effect update from the client's set-beacon packet.
    /// Returns `false` when the selection or payment is invalid.
    fn on_update_effects(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _primary: Option<MobEffectRef>,
        _secondary: Option<MobEffectRef>,
    ) -> bool {
        true
    }

    /// Shift-click override. Return `Some` to fully handle the quick-move, or
    /// `None` to fall back to the route table.
    fn quick_move(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _slot_index: usize,
        _player: &Player,
    ) -> Option<ItemStack> {
        None
    }
}
