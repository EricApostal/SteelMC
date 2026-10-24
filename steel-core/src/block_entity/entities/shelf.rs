//! `ShelfBlockEntity` (`net.minecraft.world.level.block.entity.ShelfBlockEntity`).
//!
//! A shelf is a 3-slot inventory whose contents are exposed to comparators as a 3-bit mask. Its
//! `getUpdateTag`/`getUpdatePacket` carry the `Items` list so the client can render the displayed
//! stacks, exactly like vanilla.

use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::data_components::vanilla_components::CONTAINER;
use steel_registry::game_events::GameEventRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::{ItemStackTemplate, vanilla_block_entity_types};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerRef, SharedContainer};
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `ShelfBlock.SLOT_COUNT` (one row of three slots).
pub const SHELF_SLOTS: usize = 3;

/// Vanilla `ShelfBlockEntity`'s item storage (`NonNullList<ItemStack> items`) and
/// `alignItemsToBottom` flag, exposed through `ListBackedContainer`.
struct ShelfContainer {
    items: Vec<ItemStack>,
    /// Vanilla `alignItemsToBottom`: whether the displayed items rest on the bottom of the shelf.
    align_items_to_bottom: bool,
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a shelf block entity.
unsafe impl DowncastType for ShelfContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/shelf");
}

impl Container for ShelfContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    /// Storage-local by contract: the owning block entity is notified through
    /// [`ContainerRef`]'s owner after every lock is released.
    fn set_changed(&mut self) {}

    /// Vanilla `ListBackedContainer.canPlaceItem`: `acceptsItemType(itemStack) && (getItem(slot)
    /// .isEmpty() || getItem(slot).getCount() < getMaxStackSize(itemStack))`. A shelf accepts any
    /// item type.
    fn can_place_item(&self, slot: usize, stack: &ItemStack) -> bool {
        self.items.get(slot).is_some_and(|stored| {
            stored.is_empty() || stored.count() < self.get_max_stack_size_for_item(stack)
        })
    }
}

/// `ShelfBlockEntity`.
pub struct ShelfBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<ShelfContainer>>,
    container_ref: ContainerRef,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ShelfBlockEntity`.
unsafe impl DowncastType for ShelfBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/shelf");
}

impl ShelfBlockEntity {
    /// Creates a shelf block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let callback_level = Weak::clone(&level);
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::SHELF,
            level,
            pos,
            state,
        ));
        let container = Arc::new(SyncMutex::new(ShelfContainer {
            items: vec![ItemStack::empty(); SHELF_SLOTS],
            align_items_to_bottom: false,
        }));
        // Vanilla `ShelfBlockEntity.setChanged()` is `setChanged(GameEvent.BLOCK_ACTIVATE)`, which
        // the container capability cannot express on its own: the base owner notification only
        // marks the entity changed, so the game event and the block update are added here.
        let after_changed = Arc::new(move || {
            if let Some(world) = callback_level.upgrade() {
                world.game_event(
                    &steel_registry::vanilla_game_events::BLOCK_ACTIVATE,
                    pos,
                    &GameEventContext::new(None, Some(world.get_block_state(pos))),
                );
                world.send_block_updated(pos);
            }
        });
        let shared_container: SharedContainer = Arc::<SyncMutex<ShelfContainer>>::clone(&container);

        Self {
            container_ref: ContainerRef::owned_by_block_entity_with_callback(
                shared_container,
                Arc::clone(&base),
                after_changed,
            ),
            base,
            container,
        }
    }

    /// Returns the item displayed in `slot`.
    #[must_use]
    pub fn item(&self, slot: usize) -> ItemStack {
        self.container
            .lock()
            .items
            .get(slot)
            .cloned()
            .unwrap_or_else(ItemStack::empty)
    }

    /// Vanilla `ShelfBlockEntity.swapItem`: stores `stack` and returns the previous item.
    #[must_use]
    pub fn swap_item(&self, slot: usize, stack: ItemStack) -> ItemStack {
        let mut container = self.container.lock();
        let Some(entry) = container.items.get_mut(slot) else {
            return ItemStack::empty();
        };
        std::mem::replace(entry, stack)
    }

    /// Vanilla `ShelfBlockEntity.removeItem`: removes and returns the item in `slot`.
    #[must_use]
    pub fn remove_item(&self, slot: usize) -> ItemStack {
        let mut container = self.container.lock();
        container
            .items
            .get_mut(slot)
            .map(std::mem::take)
            .unwrap_or_else(ItemStack::empty)
    }

    /// Returns whether the shelf currently holds no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.container.lock().items.iter().all(ItemStack::is_empty)
    }

    /// Vanilla `ShelfBlockEntity.setChanged(Holder<GameEvent>)`: marks the entity changed, emits
    /// the optional game event, then sends the block update that carries the new update tag.
    pub fn mark_changed(&self, world: &Arc<World>, event: Option<GameEventRef>) {
        self.base.set_changed();
        if let Some(event) = event {
            world.game_event(
                event,
                self.get_block_pos(),
                &GameEventContext::new(None, Some(self.get_block_state())),
            );
        }
        world.send_block_updated(self.get_block_pos());
    }
}

impl BlockEntity for ShelfBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    /// Vanilla `ShelfBlockEntity.setChanged()`: `setChanged(GameEvent.BLOCK_ACTIVATE)`.
    fn set_changed(&self) {
        let Some(world) = self.get_level() else {
            return;
        };
        self.mark_changed(
            &world,
            Some(&steel_registry::vanilla_game_events::BLOCK_ACTIVATE),
        );
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }

    /// Vanilla `BlockEntity.preRemoveSideEffects` (the `Container` branch): breaking a shelf
    /// drops its three displayed stacks.
    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        let items = {
            let mut container = self.container.lock();
            std::mem::replace(&mut container.items, vec![ItemStack::empty(); SHELF_SLOTS])
        };
        let Some(world) = self.get_level() else {
            return;
        };
        for item in items {
            world.drop_item_stack(pos, item);
        }
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut container = self.container.lock();
        container.items = vec![ItemStack::empty(); SHELF_SLOTS];
        if let Some(items) = view.list("Items")
            && let Some(compounds) = items.compounds()
        {
            for compound in compounds {
                // Vanilla `ItemStackWithSlot`: `Slot` defaults to 0 and entries outside the
                // container are ignored (`isValidInContainer`).
                let slot = compound.byte("Slot").unwrap_or(0);
                let Ok(slot) = usize::try_from(slot) else {
                    continue;
                };
                if slot < SHELF_SLOTS
                    && let Some(item) = ItemStack::from_borrowed_compound(&compound)
                {
                    container.items[slot] = item;
                }
            }
        }
        container.align_items_to_bottom = view.byte("align_items_to_bottom").unwrap_or(0) != 0;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let container = self.container.lock();
        let mut items: Vec<NbtCompound> = Vec::new();
        for (slot, item) in container.items.iter().enumerate() {
            if item.is_empty() {
                continue;
            }
            if let simdnbt::owned::NbtTag::Compound(mut compound) = item.to_nbt_tag_ref() {
                compound.insert("Slot", slot as i8);
                items.push(compound);
            }
        }
        // Vanilla `ContainerHelper.saveAllItems(output, items, true)` keeps the list when empty.
        nbt.insert("Items", NbtList::Compound(items));
        nbt.insert("align_items_to_bottom", container.align_items_to_bottom);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `ShelfBlockEntity.getUpdateTag` stores only the item list and the alignment
        // flag, not the block entity's component map.
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }

    /// Vanilla `ShelfBlockEntity.applyImplicitComponents`: the placed shelf takes its contents
    /// from the `minecraft:container` component (`ItemContainerContents.copyInto`).
    fn apply_components_from_item(&self, item: &ItemStack) {
        let contents = item.get(CONTAINER);
        let mut container = self.container.lock();
        for slot in 0..SHELF_SLOTS {
            container.items[slot] = contents
                .and_then(|contents| contents.items().get(slot))
                .and_then(Option::as_ref)
                .map_or_else(ItemStack::empty, ItemStackTemplate::create);
        }
    }
}
