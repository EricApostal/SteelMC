//! Crafter block entity implementation.
//!
//! The crafter is a nine-slot (3x3) container whose slots can be individually
//! disabled and whose contents are consumed one at a time by a redstone-triggered
//! crafting pulse. Vanilla keeps the disabled-slot bitset and the "triggered"
//! flag in the block entity's `ContainerData`; the menu reads both, so Steel
//! shares them through [`CrafterSlotStates`].

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU16, Ordering};
use std::sync::{Arc, Weak};

use simdnbt::ToNbtTag as _;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::locks::SyncMutex;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::inventory::container::{Container, CraftingContainer};
use crate::inventory::lock::{ContainerRef, SharedContainer};
use crate::world::World;

/// Number of slots in a crafter (3 rows of 3).
pub const CRAFTER_SLOTS: usize = 9;

/// Width and height of the crafter's crafting grid.
pub const CRAFTER_GRID_SIZE: usize = 3;

/// Bitmask of the crafter's disabled slots plus its triggered flag.
///
/// Vanilla stores this state in the block entity's `ContainerData`, which the
/// open `CrafterMenu` reads through its data slots and the slot predicate. A
/// Steel menu gates placement from a predicate that runs while the container
/// lock is already held, so this state is lock-free and shared by pointer
/// instead of living inside the locked container.
pub struct CrafterSlotStates {
    disabled: AtomicU16,
    triggered: AtomicBool,
}

impl CrafterSlotStates {
    fn new() -> Self {
        Self {
            disabled: AtomicU16::new(0),
            triggered: AtomicBool::new(false),
        }
    }

    /// Returns whether `slot` is disabled.
    #[must_use]
    pub fn is_disabled(&self, slot: usize) -> bool {
        slot < CRAFTER_SLOTS && (self.disabled.load(Ordering::Relaxed) & (1 << slot)) != 0
    }

    /// Sets whether `slot` is disabled.
    pub fn set_disabled(&self, slot: usize, disabled: bool) {
        if slot >= CRAFTER_SLOTS {
            return;
        }
        let bit = 1u16 << slot;
        if disabled {
            self.disabled.fetch_or(bit, Ordering::Relaxed);
        } else {
            self.disabled.fetch_and(!bit, Ordering::Relaxed);
        }
    }

    /// Returns the whole disabled-slot bitset, one bit per slot.
    #[must_use]
    pub fn disabled_mask(&self) -> u16 {
        self.disabled.load(Ordering::Relaxed)
    }

    fn set_disabled_mask(&self, mask: u16) {
        self.disabled.store(mask, Ordering::Relaxed);
    }

    /// Returns whether the block entity currently believes it is powered.
    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.triggered.load(Ordering::Relaxed)
    }

    /// Sets the triggered flag mirrored into the menu's powered indicator.
    pub fn set_triggered(&self, triggered: bool) {
        self.triggered.store(triggered, Ordering::Relaxed);
    }
}

impl Default for CrafterSlotStates {
    fn default() -> Self {
        Self::new()
    }
}

/// The items held by a crafter, plus the state its menu reads.
pub struct CrafterContainer {
    items: Vec<ItemStack>,
    states: Arc<CrafterSlotStates>,
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a crafter block entity.
unsafe impl DowncastType for CrafterContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/crafter");
}

impl CrafterContainer {
    fn new(states: Arc<CrafterSlotStates>) -> Self {
        Self {
            items: vec![ItemStack::empty(); CRAFTER_SLOTS],
            states,
        }
    }

    /// Vanilla `CrafterBlockEntity.canPlaceItem`.
    fn can_place_item_vanilla(&self, slot: usize, _stack: &ItemStack) -> bool {
        if self.states.is_disabled(slot) {
            return false;
        }
        let slot_stack = &self.items[slot];
        let current_stack_size = slot_stack.count();
        if current_stack_size >= slot_stack.max_stack_size() {
            return false;
        }
        if slot_stack.is_empty() {
            return true;
        }
        !self.smaller_stack_exists(current_stack_size, slot_stack, slot)
    }

    /// Vanilla `CrafterBlockEntity.smallerStackExist`.
    ///
    /// Hoppers and other inserters fill the first slots that a later, less
    /// filled stack can still merge into, so a stack is only accepted here when
    /// no following enabled slot could take it instead.
    fn smaller_stack_exists(
        &self,
        base_size: i32,
        base_item: &ItemStack,
        base_slot: usize,
    ) -> bool {
        (base_slot + 1..CRAFTER_SLOTS).any(|slot| {
            if self.states.is_disabled(slot) {
                return false;
            }
            let slot_stack = &self.items[slot];
            slot_stack.is_empty()
                || (slot_stack.count() < base_size
                    && ItemStack::is_same_item_same_components(slot_stack, base_item))
        })
    }
}

impl Container for CrafterContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        CRAFTER_SLOTS
    }

    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot >= CRAFTER_SLOTS {
            return;
        }
        // Vanilla re-enables a slot that receives an item while disabled.
        if self.states.is_disabled(slot) {
            self.states.set_disabled(slot, false);
        }
        let max_stack_size = self.get_max_stack_size_for_item(&stack);
        if !stack.is_empty() && stack.count() > max_stack_size {
            stack.set_count(max_stack_size);
        }
        self.items[slot] = stack;
    }

    fn get_max_stack_size(&self) -> i32 {
        64
    }

    fn set_changed(&mut self) {}

    fn can_place_item(&self, slot: usize, stack: &ItemStack) -> bool {
        slot < CRAFTER_SLOTS && self.can_place_item_vanilla(slot, stack)
    }
}

/// Crafter block entity.
pub struct CrafterBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<CrafterContainer>>,
    container_ref: ContainerRef,
    states: Arc<CrafterSlotStates>,
    crafting_ticks_remaining: AtomicI32,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CrafterBlockEntity`.
unsafe impl DowncastType for CrafterBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/crafter");
}

impl CrafterBlockEntity {
    /// Creates a new crafter block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::CRAFTER,
            level,
            pos,
            state,
        ));
        let states = Arc::new(CrafterSlotStates::new());
        let container = Arc::new(SyncMutex::new(CrafterContainer::new(Arc::clone(&states))));
        let shared_container: SharedContainer =
            Arc::<SyncMutex<CrafterContainer>>::clone(&container);
        Self {
            container_ref: ContainerRef::owned_by_block_entity(shared_container, Arc::clone(&base)),
            base,
            container,
            states,
            crafting_ticks_remaining: AtomicI32::new(0),
        }
    }

    /// Returns the shared disabled-slot and triggered state read by the menu.
    #[must_use]
    pub fn slot_states(&self) -> Arc<CrafterSlotStates> {
        Arc::clone(&self.states)
    }

    /// Vanilla `CrafterBlockEntity.setSlotState`.
    ///
    /// Only an empty slot can be toggled; enabling re-enables a disabled slot.
    pub fn set_slot_state(&self, slot: usize, enabled: bool) {
        let empty = self
            .container
            .lock()
            .items
            .get(slot)
            .is_some_and(ItemStack::is_empty);
        if empty {
            self.apply_slot_state(slot, enabled);
            self.set_changed();
        }
    }

    /// Applies a slot toggle after the caller verified the slot is empty.
    ///
    /// Vanilla's `setSlotState` requires an empty slot, and the open menu
    /// checks that under the container lock it already holds. This path only
    /// touches the shared slot bitset, so it never re-locks the container.
    pub fn apply_slot_state(&self, slot: usize, enabled: bool) -> bool {
        if slot >= CRAFTER_SLOTS {
            return false;
        }
        self.states.set_disabled(slot, !enabled);
        true
    }

    /// Vanilla `CrafterBlockEntity.isSlotDisabled`.
    #[must_use]
    pub fn is_slot_disabled(&self, slot: usize) -> bool {
        self.states.is_disabled(slot)
    }

    /// Vanilla `CrafterBlockEntity.setTriggered`.
    pub fn set_triggered(&self, triggered: bool) {
        self.states.set_triggered(triggered);
    }

    /// Vanilla `CrafterBlockEntity.isTriggered`.
    #[must_use]
    pub fn is_triggered(&self) -> bool {
        self.states.is_triggered()
    }

    /// Vanilla `CrafterBlockEntity.setCraftingTicksRemaining`.
    pub fn set_crafting_ticks_remaining(&self, ticks: i32) {
        self.crafting_ticks_remaining
            .store(ticks, Ordering::Relaxed);
    }

    /// Vanilla `CrafterBlockEntity.getRedstoneSignal`.
    ///
    /// The crafter's comparator output is the raw count of slots that are
    /// occupied or disabled (0-9), not the fullness formula used by ordinary
    /// containers.
    #[must_use]
    pub fn get_redstone_signal(&self) -> i32 {
        let container = self.container.lock();
        (0..CRAFTER_SLOTS)
            .filter(|&slot| !container.items[slot].is_empty() || self.states.is_disabled(slot))
            .count() as i32
    }

    /// Returns a crafting-grid snapshot of the current contents.
    ///
    /// Vanilla's `CrafterBlockEntity.asCraftInput` over the 3x3 grid; the
    /// snapshot is a [`CraftingContainer`] so it can reuse the shared crafting
    /// recipe helpers.
    #[must_use]
    pub fn crafting_input(&self) -> CraftingContainer {
        let container = self.container.lock();
        let mut grid = CraftingContainer::new(CRAFTER_GRID_SIZE, CRAFTER_GRID_SIZE);
        for (slot, item) in container.items.iter().enumerate() {
            grid.set_item(slot, item.clone());
        }
        grid
    }

    /// Shrinks every non-empty slot by one, mirroring Vanilla's
    /// `blockEntity.getItems().forEach(it -> it.shrink(1))` after a craft.
    pub fn consume_crafting_ingredients(&self) {
        let mut container = self.container.lock();
        for item in &mut container.items {
            if !item.is_empty() {
                item.shrink(1);
            }
        }
    }
}

impl BlockEntity for CrafterBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        let items = {
            let mut container = self.container.lock();
            std::mem::replace(
                &mut container.items,
                vec![ItemStack::empty(); CRAFTER_SLOTS],
            )
        };
        let Some(world) = self.get_level() else {
            return;
        };
        for item in items {
            world.drop_item_stack(pos, item);
        }
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt_view: NbtCompoundView<'_, '_> = nbt.into();
        self.crafting_ticks_remaining.store(
            nbt_view.int("crafting_ticks_remaining").unwrap_or(0),
            Ordering::Relaxed,
        );

        let mut container = self.container.lock();
        container.items.fill(ItemStack::empty());
        if let Some(items_list) = nbt_view.list("Items")
            && let Some(compounds) = items_list.compounds()
        {
            for compound in compounds {
                if let Some(slot) = compound.byte("Slot") {
                    let slot = slot as usize;
                    if slot < CRAFTER_SLOTS
                        && let Some(item) = ItemStack::from_borrowed_compound(&compound)
                    {
                        container.items[slot] = item;
                    }
                }
            }
        }

        let mut mask = 0u16;
        if let Some(disabled) = nbt_view.int_array("disabled_slots") {
            for &slot in disabled.iter() {
                if let Ok(slot) = usize::try_from(slot)
                    && slot < CRAFTER_SLOTS
                    && container.items[slot].is_empty()
                {
                    mask |= 1 << slot;
                }
            }
        }
        self.states.set_disabled_mask(mask);
        self.states
            .set_triggered(nbt_view.int("triggered").unwrap_or(0) != 0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert(
            "crafting_ticks_remaining",
            self.crafting_ticks_remaining.load(Ordering::Relaxed),
        );

        let container = self.container.lock();
        let mut items: Vec<NbtCompound> = Vec::new();
        for (slot, item) in container.items.iter().enumerate() {
            if !item.is_empty()
                && let NbtTag::Compound(mut item_nbt) = item.clone().to_nbt_tag()
            {
                item_nbt.insert("Slot", slot as i8);
                items.push(item_nbt);
            }
        }
        nbt.insert("Items", NbtList::Compound(items));
        drop(container);

        let disabled: Vec<i32> = (0..CRAFTER_SLOTS)
            .filter(|&slot| self.states.is_disabled(slot))
            .map(|slot| slot as i32)
            .collect();
        nbt.insert("disabled_slots", NbtTag::IntArray(disabled));
        nbt.insert("triggered", i32::from(self.states.is_triggered()));
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // The crafter's visible state is the CRAFTING block state and the
        // menu's data slots; it has no chunk-load block entity payload.
        None
    }

    fn tick(&self, world: &Arc<World>) {
        let remaining = self.crafting_ticks_remaining.load(Ordering::Relaxed) - 1;
        if remaining < 0 {
            return;
        }
        self.crafting_ticks_remaining
            .store(remaining, Ordering::Relaxed);
        if remaining == 0 {
            let state = self
                .get_block_state()
                .set_value(&BlockStateProperties::CRAFTING, false);
            world.set_block(self.get_block_pos(), state, UpdateFlags::UPDATE_ALL);
        }
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};

    use super::*;

    fn test_crafter() -> CrafterBlockEntity {
        init_vanilla_registry();
        CrafterBlockEntity::new(
            Weak::new(),
            BlockPos::new(1, 2, 3),
            vanilla_blocks::CRAFTER.default_state(),
        )
    }

    #[test]
    fn disabled_slots_reject_placement_until_reenabled() {
        let crafter = test_crafter();
        crafter.set_slot_state(4, false);
        assert!(crafter.is_slot_disabled(4));

        let container = crafter.container.lock();
        assert!(!container.can_place_item(4, &ItemStack::new(&vanilla_items::STONE)));
        assert!(container.can_place_item(0, &ItemStack::new(&vanilla_items::STONE)));
    }

    #[test]
    fn a_non_empty_slot_cannot_be_disabled() {
        let crafter = test_crafter();
        crafter
            .container
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::STONE));

        crafter.set_slot_state(0, false);
        assert!(!crafter.is_slot_disabled(0));
    }

    #[test]
    fn a_smaller_later_stack_blocks_insertion_into_an_earlier_slot() {
        let crafter = test_crafter();
        // Crafters fill from their last enabled slot backwards, so the assertion below
        // needs slot 1 to be the final insertion point.
        for slot in 2..CRAFTER_SLOTS {
            crafter.set_slot_state(slot, false);
        }
        {
            let mut container = crafter.container.lock();
            container.set_item(0, ItemStack::with_count(&vanilla_items::STONE, 10));
            container.set_item(1, ItemStack::with_count(&vanilla_items::STONE, 2));
        }
        let container = crafter.container.lock();
        assert!(!container.can_place_item(0, &ItemStack::new(&vanilla_items::STONE)));
        assert!(container.can_place_item(1, &ItemStack::new(&vanilla_items::STONE)));
    }

    #[test]
    fn redstone_signal_counts_occupied_and_disabled_slots() {
        let crafter = test_crafter();
        assert_eq!(crafter.get_redstone_signal(), 0);

        crafter
            .container
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::STONE));
        crafter.set_slot_state(5, false);
        crafter.set_slot_state(7, false);

        assert_eq!(crafter.get_redstone_signal(), 3);
    }

    #[test]
    fn disabled_slots_round_trip_through_nbt() {
        let crafter = test_crafter();
        crafter.set_slot_state(1, false);
        crafter.set_slot_state(8, false);
        crafter.set_triggered(true);
        crafter.set_crafting_ticks_remaining(6);

        let nbt = crafter.save_without_metadata();

        let loaded = test_crafter();
        loaded.load_with_owned_components(&nbt).expect("nbt loads");
        assert!(loaded.is_slot_disabled(1));
        assert!(loaded.is_slot_disabled(8));
        assert!(!loaded.is_slot_disabled(0));
        assert!(loaded.is_triggered());
        assert_eq!(loaded.crafting_ticks_remaining.load(Ordering::Relaxed), 6);
    }
}
