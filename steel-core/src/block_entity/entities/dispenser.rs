//! Dispenser and dropper block entities.
//!
//! Both are nine-slot containers. Vanilla implements `DropperBlockEntity` as a
//! `DispenserBlockEntity` subclass; Rust has no inheritance, so the two block
//! entity types share [`DispenserContainer`] and the generated implementation
//! below while staying distinct concrete types (they belong to different
//! vanilla block entity types and therefore need distinct downcast keys).

use std::mem;
use std::sync::{Arc, Weak};

use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerRef, SharedContainer};
use crate::world::World;

/// Number of slots in a dispenser and a dropper (3x3 grid).
pub const DISPENSER_SLOTS: usize = 9;

/// The items held by a dispenser or dropper.
///
/// Shared by both block entity types; droppers only differ in how their block
/// behavior dispenses items, not in how they store them.
pub(crate) struct DispenserContainer {
    pub(crate) items: Vec<ItemStack>,
}

impl DispenserContainer {
    fn new() -> Self {
        Self {
            items: vec![ItemStack::empty(); DISPENSER_SLOTS],
        }
    }
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by dispenser-family block entities.
unsafe impl DowncastType for DispenserContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/dispenser");
}

impl Container for DispenserContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        DISPENSER_SLOTS
    }

    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot < DISPENSER_SLOTS {
            let max_stack_size = self.get_max_stack_size_for_item(&stack);
            if !stack.is_empty() && stack.count() > max_stack_size {
                stack.set_count(max_stack_size);
            }
            self.items[slot] = stack;
        }
    }

    fn get_max_stack_size(&self) -> i32 {
        64
    }

    fn set_changed(&mut self) {}
}

/// Vanilla `DispenserBlockEntity.insertItem` over any dispenser-family
/// container: top up matching stacks, then fill empty slots, and return what did
/// not fit.
pub fn insert_item_into(container: &mut dyn Container, mut stack: ItemStack) -> ItemStack {
    if stack.is_empty() {
        return stack;
    }

    let size = container.get_container_size();
    let max_stack_size = container.get_max_stack_size_for_item(&stack);
    for slot in 0..size {
        let target_count = container.get_item(slot).count();
        let target_empty = container.get_item(slot).is_empty();
        if target_empty || ItemStack::is_same_item_same_components(&stack, container.get_item(slot))
        {
            let transfer = stack.count().min(max_stack_size - target_count).max(0);
            if transfer > 0 {
                if target_empty {
                    let taken = stack.split(transfer);
                    container.set_item(slot, taken);
                } else {
                    stack.shrink(transfer);
                    let mut merged = container.get_item(slot).clone();
                    merged.set_count(target_count + transfer);
                    container.set_item(slot, merged);
                }
            }
            if stack.is_empty() {
                break;
            }
        }
    }
    stack
}

macro_rules! dispenser_block_entity {
    ($name:ident, $type:ident, $key:literal) => {
        /// A dispenser-family block entity: nine slots plus the vanilla
        /// `getRandomSlot`/`insertItem` helpers its block behavior uses.
        pub struct $name {
            base: Arc<BlockEntityBase>,
            container: Arc<SyncMutex<DispenserContainer>>,
            container_ref: ContainerRef,
        }

        // SAFETY: Each key is owned by Steel and uniquely identifies its
        // concrete block entity implementation. A dispenser and a dropper share
        // `DispenserContainer`, but they are different vanilla block entity
        // types with different block behaviors, so each gets its own key.
        unsafe impl DowncastType for $name {
            const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new($key);
        }

        impl $name {
            /// Creates a new block entity for its vanilla block entity type.
            #[must_use]
            pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
                let base = Arc::new(BlockEntityBase::new(
                    &vanilla_block_entity_types::$type,
                    level,
                    pos,
                    state,
                ));
                let container = Arc::new(SyncMutex::new(DispenserContainer::new()));
                let shared_container: SharedContainer =
                    Arc::<SyncMutex<DispenserContainer>>::clone(&container);
                Self {
                    container_ref: ContainerRef::owned_by_block_entity(
                        shared_container,
                        Arc::clone(&base),
                    ),
                    base,
                    container,
                }
            }

            /// Returns the dispenser's backing block entity type.
            #[must_use]
            pub fn entity_type(&self) -> BlockEntityTypeRef {
                self.get_type()
            }

            /// Vanilla `DispenserBlockEntity.getRandomSlot`.
            ///
            /// Picks a non-empty slot with reservoir sampling, so a random
            /// non-empty slot is chosen without collecting the non-empty slots
            /// first. Returns `None` when every slot is empty.
            #[must_use]
            pub fn get_random_slot(&self) -> Option<usize> {
                let container = self.container.lock();
                let mut replace_slot = None;
                let mut replace_odds = 1;
                for (slot, item) in container.items.iter().enumerate() {
                    if !item.is_empty() {
                        if rand::random_range(0..replace_odds) == 0 {
                            replace_slot = Some(slot);
                        }
                        replace_odds += 1;
                    }
                }
                replace_slot
            }

            /// Vanilla `DispenserBlockEntity.insertItem`: merges `stack` into
            /// existing matching stacks, then fills empty slots, and returns
            /// whatever did not fit.
            #[must_use]
            pub fn insert_item(&self, stack: ItemStack) -> ItemStack {
                let mut container = self.container.lock();
                insert_item_into(&mut *container, stack)
            }
        }

        impl BlockEntity for $name {
            fn base(&self) -> &BlockEntityBase {
                &self.base
            }

            fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
                let items = {
                    let mut container = self.container.lock();
                    mem::replace(
                        &mut container.items,
                        vec![ItemStack::empty(); DISPENSER_SLOTS],
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
                let mut container = self.container.lock();
                container.items.fill(ItemStack::empty());

                if let Some(items_list) = nbt_view.list("Items")
                    && let Some(compounds) = items_list.compounds()
                {
                    for compound in compounds {
                        if let Some(slot) = compound.byte("Slot") {
                            let slot = slot as usize;
                            if slot < DISPENSER_SLOTS
                                && let Some(item) = ItemStack::from_borrowed_compound(&compound)
                            {
                                container.items[slot] = item;
                            }
                        }
                    }
                }
            }

            fn save_additional(&self, nbt: &mut NbtCompound) {
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
            }

            fn get_update_tag(&self) -> Option<NbtCompound> {
                // Dispensers don't send their inventory to clients on chunk load.
                None
            }

            fn container_ref(&self) -> Option<ContainerRef> {
                Some(self.container_ref.clone())
            }
        }
    };
}

dispenser_block_entity!(
    DispenserBlockEntity,
    DISPENSER,
    "steel:block_entity/dispenser"
);
dispenser_block_entity!(DropperBlockEntity, DROPPER, "steel:block_entity/dropper");

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};

    use super::*;

    fn test_dispenser() -> DispenserBlockEntity {
        init_vanilla_registry();
        DispenserBlockEntity::new(
            Weak::new(),
            BlockPos::new(1, 2, 3),
            vanilla_blocks::DISPENSER.default_state(),
        )
    }

    #[test]
    fn random_slot_is_none_when_every_slot_is_empty() {
        assert_eq!(test_dispenser().get_random_slot(), None);
    }

    #[test]
    fn random_slot_only_returns_non_empty_slots() {
        let dispenser = test_dispenser();
        dispenser
            .container
            .lock()
            .set_item(4, ItemStack::new(&vanilla_items::ARROW));

        assert_eq!(dispenser.get_random_slot(), Some(4));
    }

    #[test]
    fn insert_item_tops_up_matching_stacks_before_using_empty_slots() {
        let dispenser = test_dispenser();
        dispenser
            .container
            .lock()
            .set_item(0, ItemStack::with_count(&vanilla_items::ARROW, 60));

        let leftover = dispenser.insert_item(ItemStack::with_count(&vanilla_items::ARROW, 8));

        assert!(leftover.is_empty());
        let container = dispenser.container.lock();
        assert_eq!(container.get_item(0).count(), 64);
        assert_eq!(container.get_item(1).count(), 4);
    }

    #[test]
    fn insert_item_returns_what_does_not_fit() {
        let dispenser = test_dispenser();
        for slot in 0..DISPENSER_SLOTS {
            dispenser
                .container
                .lock()
                .set_item(slot, ItemStack::with_count(&vanilla_items::STONE, 64));
        }

        let leftover = dispenser.insert_item(ItemStack::with_count(&vanilla_items::STONE, 3));

        assert_eq!(leftover.count(), 3);
    }

    #[test]
    fn dropper_is_a_distinct_block_entity_type() {
        init_vanilla_registry();
        let dropper = DropperBlockEntity::new(
            Weak::new(),
            BlockPos::new(1, 2, 3),
            vanilla_blocks::DROPPER.default_state(),
        );

        assert_eq!(dropper.entity_type().key.path, "dropper");
        assert_eq!(test_dispenser().entity_type().key.path, "dispenser");
    }
}
