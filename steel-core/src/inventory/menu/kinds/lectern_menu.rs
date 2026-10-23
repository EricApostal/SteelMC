//! Lectern menu.
//!
//! Mirrors vanilla `LecternMenu`: one take-only book slot plus a single data
//! slot carrying the current page. The menu has no player-inventory section
//! (`MenuType.LECTERN` declares exactly one slot), so shift-clicking returns
//! nothing and page changes travel through the container button channel.

use steel_registry::vanilla_menu_types;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::block_entity::entities::LecternContainer;
use crate::inventory::prelude::*;
use crate::player::player_inventory::Prediction;

/// Button id for the previous page (`LecternMenu.BUTTON_PREV_PAGE`).
pub const BUTTON_PREV_PAGE: i32 = 1;
/// Button id for the next page (`LecternMenu.BUTTON_NEXT_PAGE`).
pub const BUTTON_NEXT_PAGE: i32 = 2;
/// Button id for taking the book (`LecternMenu.BUTTON_TAKE_BOOK`).
pub const BUTTON_TAKE_BOOK: i32 = 3;
/// First button id that jumps directly to `button_id - 100`.
pub const BUTTON_PAGE_JUMP_RANGE_START: i32 = 100;

/// Builds the lectern menu over a lectern's book container.
#[must_use]
pub fn lectern(container_id: u8, container: ContainerRef, state: Shared<LecternContainer>) -> Menu {
    let mut builder = MenuBuilder::new(&vanilla_menu_types::LECTERN, container_id);
    // Vanilla's lectern slot allows pickup but never placement.
    builder.section_with(&container, 1, SectionKind::take_only());
    let page = builder.data_slot(0);

    builder.build(LecternKind {
        container,
        state,
        page,
    })
}

/// Lectern menu behavior: page buttons and the take-book action.
pub struct LecternKind {
    container: ContainerRef,
    state: Shared<LecternContainer>,
    page: DataSlot,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl DowncastType for LecternKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/lectern");
}

impl LecternKind {
    /// Copies the container's page into the menu's data slot.
    fn sync_page(&self, behavior: &mut MenuBehavior, guard: &ContainerLockGuard) {
        let page = guard
            .get_typed::<LecternContainer>(self.container.container_id())
            .map_or(0, LecternContainer::page);
        self.page.set(behavior, i16::try_from(page).unwrap_or(0));
    }

    /// Handles a container button click (vanilla `LecternMenu.clickMenuButton`).
    ///
    /// Button 1 = previous page, 2 = next page, 3 = take the book (requires
    /// `may_build`, adds it to the player's inventory or drops it), and
    /// ids >= 100 jump to `button_id - 100`. Returns `true` when accepted.
    /// [`MenuKind::on_button_clicked`] delegates here.
    pub fn handle_button(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        button_id: i32,
        player: &Player,
    ) -> bool {
        let id = self.container.container_id();
        let mut take_book = false;

        let accepted = {
            let Some(state) = guard.get_typed_mut::<LecternContainer>(id) else {
                return false;
            };
            match button_id {
                BUTTON_PREV_PAGE => {
                    let page = state.page();
                    state.set_page(page - 1);
                    true
                }
                BUTTON_NEXT_PAGE => {
                    let page = state.page();
                    state.set_page(page + 1);
                    true
                }
                BUTTON_TAKE_BOOK => {
                    // Vanilla rejects the take when the player may not build.
                    if player.get_abilities().may_build {
                        take_book = true;
                        true
                    } else {
                        false
                    }
                }
                page_button if page_button >= BUTTON_PAGE_JUMP_RANGE_START => {
                    state.set_page(page_button - BUTTON_PAGE_JUMP_RANGE_START);
                    true
                }
                _ => false,
            }
        };
        if !accepted {
            return false;
        }

        let book = if take_book {
            guard
                .get_typed_mut::<LecternContainer>(id)
                .map_or_else(ItemStack::empty, LecternContainer::take_book)
        } else {
            ItemStack::empty()
        };
        // Publishes the page pulse or the cleared `HAS_BOOK` state.
        guard.set_changed(id);
        if !book.is_empty() {
            // Give the book back with every container lock released so the
            // player inventory is locked in its own order, mirroring vanilla
            // `player.getInventory().add(book)` with the drop fallback.
            guard.run_unlocked(|| player.add_item_or_drop(book, Prediction::Predicted));
        }
        self.sync_page(behavior, guard);
        true
    }
}

impl MenuKind for LecternKind {
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.container.still_valid(player) && !self.state.lock().book().is_empty()
    }

    fn on_open(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.sync_page(behavior, guard);
    }

    fn on_tick(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.sync_page(behavior, guard);
    }

    fn quick_move(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _slot_index: usize,
        _player: &Player,
    ) -> Option<ItemStack> {
        // Vanilla `LecternMenu.quickMoveStack` always returns `ItemStack.EMPTY`.
        Some(ItemStack::empty())
    }

    fn on_button_clicked(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        button_id: i32,
        player: &Player,
    ) -> bool {
        self.handle_button(behavior, guard, button_id, player)
    }
}
