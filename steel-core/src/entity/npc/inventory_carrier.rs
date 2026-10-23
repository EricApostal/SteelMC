//! Vanilla `InventoryCarrier`: an item inventory carried by an entity.

use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::item_stack::ItemStack;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::inventory::container::{Container, SimpleContainer};

#[cfg(test)]
mod tests;

/// Vanilla `InventoryCarrier`: the mob inventory that is saved under `Inventory`
/// as a flat list of stacks in slot order.
///
/// Steel's [`SimpleContainer`] only stores slots, so vanilla
/// `SimpleContainer.addItem`/`canAddItem` live here next to the persistence.
pub struct InventoryCarrier {
    inventory: SimpleContainer,
}

// SAFETY: This Steel-owned key uniquely identifies `InventoryCarrier`.
unsafe impl DowncastType for InventoryCarrier {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/inventory_carrier");
}

impl InventoryCarrier {
    /// The NBT list holding the inventory, vanilla `InventoryCarrier.TAG_INVENTORY`.
    pub const TAG_INVENTORY: &'static str = "Inventory";

    /// Creates a carrier with `size` empty slots.
    #[must_use]
    pub fn new(size: usize) -> Self {
        Self {
            inventory: SimpleContainer::new(size),
        }
    }

    /// The underlying slot container.
    #[must_use]
    pub const fn inventory(&self) -> &SimpleContainer {
        &self.inventory
    }

    /// Mutable access to the underlying slot container.
    #[must_use]
    pub const fn inventory_mut(&mut self) -> &mut SimpleContainer {
        &mut self.inventory
    }

    /// The number of slots.
    #[must_use]
    pub fn size(&self) -> usize {
        self.inventory.items().len()
    }

    /// The stack in `slot`, vanilla `SimpleContainer.getItem`.
    #[must_use]
    pub fn get_item(&self, slot: usize) -> &ItemStack {
        &self.inventory.items()[slot]
    }

    /// Replaces the stack in `slot`, vanilla `SimpleContainer.setItem`.
    pub fn set_item(&mut self, slot: usize, stack: ItemStack) {
        self.inventory.items_mut()[slot] = stack;
    }

    /// Vanilla `SimpleContainer.canAddItem`.
    #[must_use]
    pub fn can_add_item(&self, stack: &ItemStack) -> bool {
        self.inventory.items().iter().any(|slot| {
            slot.is_empty()
                || (ItemStack::is_same_item_same_components(slot, stack)
                    && slot.count() < slot.max_stack_size())
        })
    }

    /// Vanilla `SimpleContainer.addItem`: merges into matching slots first, then
    /// fills empty slots, returning whatever did not fit.
    #[must_use]
    pub fn add_item(&mut self, stack: &ItemStack) -> ItemStack {
        if stack.is_empty() {
            return ItemStack::empty();
        }

        let mut remaining = stack.clone();
        for slot in self.inventory.items_mut() {
            if ItemStack::is_same_item_same_components(slot, &remaining) {
                let moved = remaining
                    .count()
                    .min(slot.max_stack_size() - slot.count())
                    .max(0);
                slot.count += moved;
                remaining.count -= moved;
                if remaining.is_empty() {
                    return remaining;
                }
            }
        }

        for slot in self.inventory.items_mut() {
            if slot.is_empty() {
                *slot = remaining;
                return ItemStack::empty();
            }
        }

        remaining
    }

    /// Vanilla `SimpleContainer.clearContent`.
    pub fn clear(&mut self) {
        self.inventory.items_mut().fill(ItemStack::empty());
    }

    /// Vanilla `InventoryCarrier.readInventoryFromTag`.
    ///
    /// The list holds stacks without slot indices, so it is re-added in order
    /// through [`InventoryCarrier::add_item`].
    pub fn read_from_tag(&mut self, nbt: &NbtCompoundView<'_, '_>) {
        let Some(compounds) = nbt
            .list(Self::TAG_INVENTORY)
            .and_then(|list| list.compounds())
        else {
            return;
        };

        self.clear();
        for compound in compounds {
            if let Some(stack) = ItemStack::from_borrowed_compound(&compound) {
                let _ = self.add_item(&stack);
            }
        }
    }

    /// Vanilla `InventoryCarrier.writeInventoryToTag`.
    pub fn write_to_tag(&self, nbt: &mut NbtCompound) {
        let items = self
            .inventory
            .items()
            .iter()
            .filter(|stack| !stack.is_empty())
            .filter_map(|stack| match stack.to_nbt_tag_ref() {
                NbtTag::Compound(compound) => Some(compound),
                _ => None,
            })
            .collect();
        nbt.insert(Self::TAG_INVENTORY, NbtList::Compound(items));
    }
}

impl Container for InventoryCarrier {
    fn items(&self) -> &[ItemStack] {
        self.inventory.items()
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        self.inventory.items_mut()
    }

    fn set_changed(&mut self) {
        self.inventory.set_changed();
    }
}
