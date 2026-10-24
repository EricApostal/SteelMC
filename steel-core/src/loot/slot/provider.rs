//! Vanilla `SlotProvider`: an object that can hand out its item slots.
//!
//! Mirrors `net.minecraft.world.entity.SlotProvider` and the block-container
//! side of it: vanilla's `Container` implements `SlotProvider`, so a
//! `ContainerRef` addressed by `/item ... block <pos> ...` is one too.

use std::cell::RefCell;

use steel_registry::item_stack::ItemStack;

use super::{
    access::SlotAccess,
    collection::{SlotCollection, of_many},
};
use crate::inventory::{
    container::Container,
    lock::{ContainerId, ContainerLockGuard},
};

/// Vanilla `SlotProvider`.
pub(crate) trait SlotProvider {
    /// Vanilla `SlotProvider.getSlot`: `None` is vanilla's null slot access
    /// (an index this object has no slot for).
    fn get_slot(&self, slot: i32) -> Option<Box<dyn SlotAccess + '_>>;

    /// Vanilla `SlotProvider.getSlotsFromRange`.
    fn get_slots_from_range<'s>(&'s self, slots: &[i32]) -> Box<dyn SlotCollection + 's> {
        let mut accesses = Vec::new();
        for slot in slots {
            if let Some(access) = self.get_slot(*slot) {
                accesses.push(access);
            }
        }
        of_many(accesses)
    }
}

/// Vanilla `Container` as a `SlotProvider`: its slots are the container's own
/// indices, and the container is locked for the lifetime of the collection.
pub(crate) struct ContainerSlots<'a> {
    guard: RefCell<&'a mut ContainerLockGuard>,
    id: ContainerId,
}

impl<'a> ContainerSlots<'a> {
    /// Wraps an already-locked container.
    pub(crate) const fn new(guard: &'a mut ContainerLockGuard, id: ContainerId) -> Self {
        Self {
            guard: RefCell::new(guard),
            id,
        }
    }

    /// Vanilla `Container.getContainerSize`; `None` when the container is gone.
    fn container_size(&self) -> Option<usize> {
        self.guard
            .borrow()
            .get(self.id)
            .map(Container::get_container_size)
    }

    fn get_item(&self, index: usize) -> Option<ItemStack> {
        self.guard
            .borrow()
            .get(self.id)
            .map(|container| container.get_item(index).clone())
    }

    /// Vanilla `Container.setItem` through the lock guard.
    fn set_item(&self, index: usize, stack: ItemStack) -> bool {
        self.guard.borrow_mut().set_item(self.id, index, stack)
    }
}

/// Vanilla `Container$1`: the per-slot access of a container.
struct ContainerSlotAccess<'s, 'a> {
    slots: &'s ContainerSlots<'a>,
    index: usize,
}

impl SlotAccess for ContainerSlotAccess<'_, '_> {
    fn get(&self) -> ItemStack {
        self.slots
            .get_item(self.index)
            .unwrap_or_else(ItemStack::empty)
    }

    fn set(&self, stack: ItemStack) -> bool {
        self.slots.set_item(self.index, stack)
    }
}

impl<'a> SlotProvider for ContainerSlots<'a> {
    fn get_slot(&self, slot: i32) -> Option<Box<dyn SlotAccess + '_>> {
        let index = usize::try_from(slot).ok()?;
        if index >= self.container_size()? {
            return None;
        }
        Some(Box::new(ContainerSlotAccess { slots: self, index }))
    }
}
