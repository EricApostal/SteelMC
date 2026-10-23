//! `ShelfBlockEntity` (`net.minecraft.world.level.block.entity.ShelfBlockEntity`).
//!
//! A shelf is a 3-slot inventory whose contents are exposed to comparators as a 3-bit mask. Its
//! `getUpdateTag`/`getUpdatePacket` carry the `Items` list so the client can render the displayed
//! stacks, exactly like vanilla.

use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::game_events::GameEventRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `ShelfBlock.SLOT_COUNT` (one row of three slots).
pub const SHELF_SLOTS: usize = 3;

/// Mutable shelf state.
struct ShelfState {
    items: [ItemStack; SHELF_SLOTS],
    /// Vanilla `alignItemsToBottom`: whether the displayed items rest on the bottom of the shelf.
    align_items_to_bottom: bool,
}

/// `ShelfBlockEntity`.
pub struct ShelfBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<ShelfState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ShelfBlockEntity`.
unsafe impl DowncastType for ShelfBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/shelf");
}

impl ShelfBlockEntity {
    /// Creates a shelf block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::SHELF, level, pos, state),
            state: SyncMutex::new(ShelfState {
                items: std::array::from_fn(|_| ItemStack::empty()),
                align_items_to_bottom: false,
            }),
        }
    }

    /// Returns the item displayed in `slot`.
    #[must_use]
    pub fn item(&self, slot: usize) -> ItemStack {
        self.state
            .lock()
            .items
            .get(slot)
            .cloned()
            .unwrap_or_else(ItemStack::empty)
    }

    /// Vanilla `ShelfBlockEntity.swapItem`: stores `stack` and returns the previous item.
    #[must_use]
    pub fn swap_item(&self, slot: usize, stack: ItemStack) -> ItemStack {
        let mut state = self.state.lock();
        let Some(entry) = state.items.get_mut(slot) else {
            return ItemStack::empty();
        };
        std::mem::replace(entry, stack)
    }

    /// Vanilla `ShelfBlockEntity.removeItem`: removes and returns the item in `slot`.
    #[must_use]
    pub fn remove_item(&self, slot: usize) -> ItemStack {
        let mut state = self.state.lock();
        state
            .items
            .get_mut(slot)
            .map(std::mem::take)
            .unwrap_or_else(ItemStack::empty)
    }

    /// Returns whether the shelf currently holds no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.lock().items.iter().all(ItemStack::is_empty)
    }

    /// Vanilla `ShelfBlockEntity.markChanged`: emits the game event and syncs the block.
    pub fn mark_changed(&self, world: &Arc<World>, event: Option<GameEventRef>) {
        if let Some(event) = event {
            world.game_event(
                event,
                self.get_block_pos(),
                &GameEventContext::new(None, Some(self.get_block_state())),
            );
        }
        BlockEntity::set_changed(self);
    }
}

impl BlockEntity for ShelfBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.items = std::array::from_fn(|_| ItemStack::empty());
        if let Some(items) = view.list("Items")
            && let Some(compounds) = items.compounds()
        {
            for compound in compounds {
                let Some(slot) = compound.byte("Slot").map(|slot| slot as usize) else {
                    continue;
                };
                if slot < SHELF_SLOTS
                    && let Some(item) = ItemStack::from_borrowed_compound(&compound)
                {
                    state.items[slot] = item;
                }
            }
        }
        state.align_items_to_bottom = view.byte("align_items_to_bottom").unwrap_or(0) != 0;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        let mut items: Vec<NbtCompound> = Vec::new();
        for (slot, item) in state.items.iter().enumerate() {
            if item.is_empty() {
                continue;
            }
            if let simdnbt::owned::NbtTag::Compound(mut compound) = item.to_nbt_tag_ref() {
                compound.insert("Slot", slot as i8);
                items.push(compound);
            }
        }
        nbt.insert("Items", NbtList::Compound(items));
        nbt.insert("align_items_to_bottom", state.align_items_to_bottom);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }
}
