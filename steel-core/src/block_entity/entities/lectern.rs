//! Lectern block-entity implementation.
//!
//! A lectern holds a single written or writable book, mirroring vanilla's
//! `LecternBlockEntity`: slot 0 is the displayed book and the page counter
//! (`Page` NBT) is clamped to the book's page count. Setting a new book resets
//! the page to 0, and removing the book resets both the page and the block's
//! `HAS_BOOK` state.
//!
//! Block-state and redstone effects are requested by the container through
//! pending flags and published from the container-owner callback, so they run
//! after every container lock has been released.

use std::{
    mem,
    sync::{Arc, Weak},
};

use glam::DVec3;
use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::data_components::vanilla_components::{
    WRITABLE_BOOK_CONTENT, WRITTEN_BOOK_CONTENT,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::{
    level_events, vanilla_block_entity_types, vanilla_blocks, vanilla_game_events,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef, SharedContainer};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Container slot index of the lectern book.
pub const LECTERN_BOOK_SLOT: usize = 0;

/// Number of book slots in a lectern.
pub const LECTERN_SLOTS: usize = 1;

/// Vanilla `LecternBlock.PAGE_CHANGE_IMPULSE_TICKS`.
const PAGE_CHANGE_IMPULSE_TICKS: i32 = 2;

const HAS_BOOK: &BoolProperty = &BlockStateProperties::HAS_BOOK;
const POWERED: &BoolProperty = &BlockStateProperties::POWERED;
const FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

/// Lectern block entity.
///
/// Holds a single book (written or writable) plus the current page.
pub struct LecternBlockEntity {
    base: Arc<BlockEntityBase>,
    state: Arc<SyncMutex<LecternContainer>>,
    container_ref: ContainerRef,
}

/// Independently lockable lectern book and page storage.
pub struct LecternContainer {
    book: ItemStack,
    page: i32,
    /// Derived from the book's content; recomputed whenever the book changes.
    page_count: i32,
    /// Pending vanilla `resetBookState` request carrying the new `HAS_BOOK`.
    pending_book_state: Option<bool>,
    /// Pending vanilla `signalPageChange` request.
    pending_page_signal: bool,
}

// SAFETY: This key is owned by Steel and uniquely identifies `LecternBlockEntity`.
unsafe impl DowncastType for LecternBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/lectern");
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a lectern block entity.
unsafe impl DowncastType for LecternContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/lectern");
}

/// Returns the number of pages in a book, or 0 for non-book stacks.
#[must_use]
pub fn page_count_for_book(book: &ItemStack) -> i32 {
    if let Some(written) = book.get(WRITTEN_BOOK_CONTENT) {
        i32::try_from(written.pages().len()).unwrap_or(i32::MAX)
    } else if let Some(writable) = book.get(WRITABLE_BOOK_CONTENT) {
        i32::try_from(writable.pages().len()).unwrap_or(i32::MAX)
    } else {
        0
    }
}

impl LecternContainer {
    /// Creates an empty lectern container.
    #[must_use]
    pub fn new() -> Self {
        Self {
            book: ItemStack::empty(),
            page: 0,
            page_count: 0,
            pending_book_state: None,
            pending_page_signal: false,
        }
    }

    /// Returns the stored book.
    #[must_use]
    pub fn book(&self) -> &ItemStack {
        &self.book
    }

    /// Returns the current page.
    #[must_use]
    pub const fn page(&self) -> i32 {
        self.page
    }

    /// Returns the derived page count of the stored book.
    #[must_use]
    pub const fn page_count(&self) -> i32 {
        self.page_count
    }

    /// Sets the stored book, resetting the page to 0.
    ///
    /// Mirrors vanilla `setBook`, including recomputing the page count.
    pub fn set_book(&mut self, book: ItemStack) {
        self.page_count = page_count_for_book(&book);
        let has_book = book.get(WRITABLE_BOOK_CONTENT).is_some()
            || book.get(WRITTEN_BOOK_CONTENT).is_some();
        self.book = book;
        self.page = 0;
        self.pending_book_state = Some(has_book);
    }

    /// Removes and returns the stored book, resetting the page to 0.
    ///
    /// Mirrors vanilla `onBookItemRemove`.
    pub fn take_book(&mut self) -> ItemStack {
        self.page = 0;
        self.page_count = 0;
        self.pending_book_state = Some(false);
        mem::replace(&mut self.book, ItemStack::empty())
    }

    /// Sets the current page, clamped like vanilla `Mth.clamp(page, 0, pageCount - 1)`.
    ///
    /// Returns `true` when the page changed.
    pub fn set_page(&mut self, page: i32) -> bool {
        // `Mth.clamp` clamps to the minimum first, so an empty book yields page `-1`.
        let clamped = page.max(0).min(self.page_count - 1);
        if clamped == self.page {
            return false;
        }
        self.page = clamped;
        self.pending_page_signal = true;
        true
    }

    /// Returns the comparator output, mirroring vanilla `getRedstoneSignal`.
    #[must_use]
    pub fn redstone_signal(&self) -> i32 {
        let progress = if self.page_count > 1 {
            f64::from(self.page) / f64::from(self.page_count - 1)
        } else {
            1.0
        };
        (progress * 14.0).floor() as i32 + i32::from(self.has_book())
    }

    /// Vanilla `LecternBlockEntity.hasBook`: only stacks carrying a book content component count.
    fn has_book(&self) -> bool {
        self.book.get(WRITABLE_BOOK_CONTENT).is_some()
            || self.book.get(WRITTEN_BOOK_CONTENT).is_some()
    }

    /// Forgets the book without a block-state change, used when loading NBT.
    fn clear(&mut self) {
        self.book = ItemStack::empty();
        self.page = 0;
        self.page_count = 0;
        self.pending_book_state = None;
        self.pending_page_signal = false;
    }
}

impl Default for LecternContainer {
    fn default() -> Self {
        Self::new()
    }
}

impl LecternBlockEntity {
    /// Creates a new lectern block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let callback_level = Weak::clone(&level);
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::LECTERN,
            level,
            pos,
            state,
        ));
        let container = Arc::new(SyncMutex::new(LecternContainer::new()));
        let callback_container = Arc::clone(&container);
        let after_changed = Arc::new(move || {
            Self::publish_pending_state(&callback_level, pos, &callback_container);
        });
        let shared_container: SharedContainer =
            Arc::<SyncMutex<LecternContainer>>::clone(&container);
        Self {
            container_ref: ContainerRef::owned_by_block_entity_with_callback(
                shared_container,
                Arc::clone(&base),
                after_changed,
            ),
            base,
            state: container,
        }
    }

    /// Returns a handle to the lectern book/page storage.
    #[must_use]
    pub fn state_handle(&self) -> Arc<SyncMutex<LecternContainer>> {
        Arc::clone(&self.state)
    }

    /// Returns a copy of the stored book.
    #[must_use]
    pub fn book(&self) -> ItemStack {
        let state = self.state.lock();
        state.book.copy_with_count(state.book.count())
    }

    /// Returns whether a book is stored.
    #[must_use]
    pub fn has_book(&self) -> bool {
        self.state.lock().has_book()
    }

    /// Returns the current page.
    #[must_use]
    pub fn page(&self) -> i32 {
        self.state.lock().page
    }

    /// Returns the derived page count of the stored book.
    #[must_use]
    pub fn page_count(&self) -> i32 {
        self.state.lock().page_count
    }

    /// Returns the comparator output for the stored book and page.
    #[must_use]
    pub fn redstone_signal(&self) -> i32 {
        self.state.lock().redstone_signal()
    }

    /// Sets the stored book, resetting the page to 0 and updating `HAS_BOOK`.
    pub fn set_book(&self, book: ItemStack) {
        self.mutate(|state| state.set_book(book));
    }

    /// Removes and returns the stored book, resetting the page to 0.
    #[must_use]
    pub fn take_book(&self) -> ItemStack {
        let mut guard = ContainerLockGuard::lock_all(&[&self.container_ref]);
        let id = self.container_ref.container_id();
        let book = guard
            .get_typed_mut::<LecternContainer>(id)
            .map_or_else(ItemStack::empty, LecternContainer::take_book);
        guard.set_changed(id);
        book
    }

    /// Sets the current page, clamped to the book's page count.
    ///
    /// Returns `true` when the page changed.
    pub fn set_page(&self, page: i32) -> bool {
        let mut changed = false;
        self.mutate(|state| changed = state.set_page(page));
        changed
    }

    /// Applies a placed block item's `BLOCK_ENTITY_DATA` payload.
    ///
    /// Mirrors the vanilla placement path where the block item's stored book
    /// is restored into the new lectern.
    pub fn apply_item_block_entity_data(&self, payload: &NbtCompound) {
        let mut bytes = Vec::new();
        payload.write(&mut bytes);
        let Ok(borrowed) =
            simdnbt::borrow::read_compound(&mut std::io::Cursor::new(bytes.as_slice()))
        else {
            log::warn!(
                "failed to reborrow lectern block-entity data at {:?}",
                self.get_block_pos()
            );
            return;
        };
        self.load_additional(&borrowed);
        self.set_changed();
    }

    /// Runs `f` on the locked container, then publishes pending block effects.
    fn mutate(&self, f: impl FnOnce(&mut LecternContainer)) {
        let mut guard = ContainerLockGuard::lock_all(&[&self.container_ref]);
        let id = self.container_ref.container_id();
        if let Some(state) = guard.get_typed_mut::<LecternContainer>(id) {
            f(state);
        }
        guard.set_changed(id);
    }

    /// Applies pending vanilla `resetBookState` / `signalPageChange` effects.
    ///
    /// Runs from the container-owner callback, after all container locks are
    /// released.
    fn publish_pending_state(
        level: &Weak<World>,
        pos: BlockPos,
        container: &SyncMutex<LecternContainer>,
    ) {
        let (book_state, page_signal) = {
            let mut state = container.lock();
            let book_state = state.pending_book_state.take();
            let page_signal = mem::replace(&mut state.pending_page_signal, false);
            (book_state, page_signal)
        };
        if book_state.is_none() && !page_signal {
            return;
        }
        let Some(world) = level.upgrade() else {
            return;
        };
        let state = world.get_block_state(pos);
        let block = state.get_block();
        if block != &vanilla_blocks::LECTERN {
            return;
        }

        if let Some(has_book) = book_state {
            let new_state = state
                .set_value(POWERED, false)
                .set_value(HAS_BOOK, has_book);
            world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
            world.game_event(
                &vanilla_game_events::BLOCK_CHANGE,
                pos,
                &GameEventContext::new(None, Some(new_state)),
            );
            world.update_neighbors_at(pos.below(), block);
        }

        if page_signal {
            let powered = world.get_block_state(pos).set_value(POWERED, true);
            world.set_block(pos, powered, UpdateFlags::UPDATE_ALL);
            world.update_neighbors_at(pos.below(), block);
            world.schedule_block_tick_default(pos, block, PAGE_CHANGE_IMPULSE_TICKS);
            world.level_event(level_events::SOUND_PAGE_TURN, pos, 0, None);
        }
    }
}

impl BlockEntity for LecternBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, state: BlockStateId) {
        if !state.get_value(HAS_BOOK) {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        // Vanilla `LecternBlockEntity.preRemoveSideEffects` drops the book one block above the
        // lectern, offset a quarter block toward its facing direction.
        let (step_x, step_z) = state.get_value(FACING).offset_xz();
        let drop_pos = DVec3::new(
            f64::from(pos.x()) + 0.5 + 0.25 * f64::from(step_x),
            f64::from(pos.y()) + 1.0,
            f64::from(pos.z()) + 0.5 + 0.25 * f64::from(step_z),
        );
        if let Some(entity) = world.spawn_item(drop_pos, self.book()) {
            entity.set_default_pickup_delay();
        }
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt_view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.clear();

        if let Some(book_compound) = nbt_view.compound("Book")
            && let Some(book) = ItemStack::from_borrowed_compound(&book_compound)
            && !book.is_empty()
        {
            state.set_book(book);
            let page = nbt_view.int("Page").unwrap_or(0);
            // Vanilla clamps with `Mth.clamp(page, 0, pageCount - 1)`, which yields `-1` for a
            // book without pages.
            state.page = page.max(0).min(state.page_count - 1);
        }
        // Loading must not publish block-state changes: the block state is
        // restored from disk separately.
        state.pending_book_state = None;
        state.pending_page_signal = false;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        if state.book.is_empty() {
            return;
        }
        if let NbtTag::Compound(book_nbt) =
            state.book.copy_with_count(state.book.count()).to_nbt_tag()
        {
            nbt.insert("Book", book_nbt);
        }
        nbt.insert("Page", state.page);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla's default `BlockEntity.getUpdateTag` returns `saveCustomOnly`, so the chunk
        // sync carries the stored `Book` and `Page`.
        Some(self.save_custom_only())
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }
}

impl Container for LecternContainer {
    fn items(&self) -> &[ItemStack] {
        std::slice::from_ref(&self.book)
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        std::slice::from_mut(&mut self.book)
    }

    fn get_container_size(&self) -> usize {
        LECTERN_SLOTS
    }

    fn set_item(&mut self, _slot: usize, _stack: ItemStack) {
        // Vanilla's `bookAccess.setItem` is a no-op: books enter through
        // `use_item_on` and leave through pickup or button 3.
    }

    fn remove_item(&mut self, slot: usize, count: i32) -> ItemStack {
        if slot != LECTERN_BOOK_SLOT {
            return ItemStack::empty();
        }
        let result = self.book.split(count);
        if self.book.is_empty() {
            self.page = 0;
            self.page_count = 0;
            self.pending_book_state = Some(false);
        }
        result
    }

    fn remove_item_no_update(&mut self, slot: usize) -> ItemStack {
        if slot != LECTERN_BOOK_SLOT {
            return ItemStack::empty();
        }
        self.take_book()
    }

    fn clear_content(&mut self) -> i32 {
        // Vanilla `LecternBlockEntity.clearContent` delegates to `setBook(EMPTY)`, which resets
        // the page counters but leaves the `HAS_BOOK` block state untouched.
        let count = self.book.count();
        if count > 0 {
            self.book = ItemStack::empty();
            self.page = 0;
            self.page_count = 0;
        }
        count
    }

    fn get_max_stack_size(&self) -> i32 {
        1
    }

    fn can_place_item(&self, _slot: usize, _stack: &ItemStack) -> bool {
        // Vanilla's `bookAccess.canPlaceItem` rejects every stack.
        false
    }

    fn set_changed(&mut self) {}
}
