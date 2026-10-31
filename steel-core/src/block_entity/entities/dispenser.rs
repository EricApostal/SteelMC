//! Dispenser and dropper block entities.
//!
//! Both are nine-slot containers. Vanilla implements `DropperBlockEntity` as a
//! `DispenserBlockEntity` subclass; Rust has no inheritance, so the two block
//! entity types share [`DispenserContainer`] and the generated implementation
//! below while staying distinct concrete types (they belong to different
//! vanilla block entity types and therefore need distinct downcast keys).
//!
//! Vanilla's `DispenserBlockEntity` extends `RandomizableContainerBlockEntity`,
//! so both types persist a pending `LootTable`/`LootTableSeed` and unpack it
//! when their contents are first read.

use std::{
    mem,
    str::FromStr as _,
    sync::{Arc, Weak},
};

use rand::RngExt as _;
use rand::SeedableRng as _;
use rand::rngs::StdRng;
use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::{LootContext, LootTableRef};
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_block_entity_types};
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex,
};

use crate::behavior::advancement_hooks::raise_generate_loot;
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::{LivingEntity as _, entity_loot_ref, living_loot_equipment};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerRef, SharedContainer};
use crate::player::Player;
use crate::world::World;

/// Number of slots in a dispenser and a dropper (3x3 grid).
pub const DISPENSER_SLOTS: usize = 9;

/// The items held by a dispenser or dropper.
///
/// Shared by both block entity types; droppers only differ in how their block
/// behavior dispenses items, not in how they store them.
pub(crate) struct DispenserContainer {
    pub(crate) items: Vec<ItemStack>,
    /// Vanilla `RandomizableContainerBlockEntity.lootTable`.
    loot_table: Option<Identifier>,
    /// Vanilla `RandomizableContainerBlockEntity.lootTableSeed`.
    loot_table_seed: i64,
}

impl DispenserContainer {
    fn new() -> Self {
        Self {
            items: vec![ItemStack::empty(); DISPENSER_SLOTS],
            loot_table: None,
            loot_table_seed: 0,
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

/// Vanilla `Util.shuffle`: Fisher-Yates from the end of the slice.
///
/// Shared with the entity container carts, whose loot fill is the same
/// `LootTable.fill` slot distribution.
pub(crate) fn shuffle<T>(values: &mut [T], rng: &mut impl rand::Rng) {
    for i in (2..=values.len()).rev() {
        values.swap(i - 1, rng.random_range(0..i));
    }
}

/// Vanilla `LootTable.shuffleAndSplitItems`: split oversized stacks so the
/// generated items spread over the available slots, then shuffle the result.
///
/// Shared with the entity container carts, whose loot fill is the same
/// `LootTable.fill` slot distribution.
pub(crate) fn shuffle_and_split_items(
    items: Vec<ItemStack>,
    available_slots: usize,
    rng: &mut impl rand::Rng,
) -> Vec<ItemStack> {
    let mut splittable = Vec::new();
    let mut result = Vec::new();
    for stack in items {
        if stack.is_empty() {
            continue;
        }
        if stack.count() > 1 {
            splittable.push(stack);
        } else {
            result.push(stack);
        }
    }

    while available_slots > result.len() + splittable.len() && !splittable.is_empty() {
        let mut stack = splittable.remove(rng.random_range(0..splittable.len()));
        let copy = stack.split(rng.random_range(1..=stack.count() / 2));
        if stack.count() > 1 && rng.random_bool(0.5) {
            splittable.push(stack);
        } else {
            result.push(stack);
        }
        if copy.count() > 1 && rng.random_bool(0.5) {
            splittable.push(copy);
        } else {
            result.push(copy);
        }
    }

    result.extend(splittable);
    shuffle(&mut result, rng);
    result
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
                // Vanilla `DispenserBlockEntity.getRandomSlot` unpacks the pending
                // loot table before sampling.
                self.unpack_loot_table(None);
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

            /// Vanilla `RandomizableContainer.unpackLootTable`.
            ///
            /// Generates the pending table's items and places them with
            /// `LootTable.fill`'s slot selection, then clears the table. Vanilla runs
            /// this from every container read (`getItem`, `isEmpty`, `removeItem`,
            /// `setItem`), from `getRandomSlot`, and from `createMenu` with the
            /// opening player; Steel's callers invoke it explicitly because the
            /// container capability is read through
            /// [`ContainerRef`](crate::inventory::lock::ContainerRef).
            pub fn unpack_loot_table(&self, player: Option<&Player>) {
                let world = self.get_level();
                // Vanilla clears the table before resolving and rolling it, so a
                // broken key cannot leave the container permanently pending.
                let (loot_table_key, seed) = {
                    let mut container = self.container.lock();
                    let Some(loot_table_key) = container.loot_table.take() else {
                        return;
                    };
                    (loot_table_key, container.loot_table_seed)
                };
                let Some(loot_table) = REGISTRY.loot_tables.by_key(&loot_table_key) else {
                    log::warn!("Dispenser has unknown loot table {loot_table_key}");
                    return;
                };
                // Vanilla fires `GENERATE_LOOT` before the loot is rolled.
                if let (Some(player), Some(world)) = (player, world.as_ref()) {
                    raise_generate_loot(player, world, &loot_table_key);
                }
                let pos = self.get_block_pos();
                let origin = (
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 0.5,
                    f64::from(pos.z()) + 0.5,
                );
                if seed == 0 {
                    self.fill_from_loot_table(loot_table, origin, player, &mut rand::rng());
                } else {
                    self.fill_from_loot_table(
                        loot_table,
                        origin,
                        player,
                        &mut StdRng::seed_from_u64(seed as u64),
                    );
                }
            }

            /// Vanilla `LootTable.fill` against the dispenser's empty slots.
            fn fill_from_loot_table<R: rand::Rng>(
                &self,
                loot_table: LootTableRef,
                origin: (f64, f64, f64),
                player: Option<&Player>,
                rng: &mut R,
            ) {
                let equipment = player.map(living_loot_equipment);
                let items = {
                    let mut context =
                        LootContext::new(rng).with_origin(origin.0, origin.1, origin.2);
                    if let (Some(player), Some(equipment)) = (player, equipment.as_ref()) {
                        context = context
                            .with_luck(player.get_luck())
                            .with_this_entity(entity_loot_ref(player, Some(equipment)));
                    }
                    loot_table.get_random_items(&mut context)
                };
                let mut container = self.container.lock();
                let mut available_slots: Vec<usize> = (0..DISPENSER_SLOTS)
                    .filter(|&slot| container.items[slot].is_empty())
                    .collect();
                shuffle(&mut available_slots, rng);
                let items = shuffle_and_split_items(items, available_slots.len(), rng);
                let placed = !items.is_empty();
                for item in items {
                    let Some(slot) = available_slots.pop() else {
                        break;
                    };
                    container.set_item(slot, item);
                }
                drop(container);
                if placed {
                    BlockEntity::set_changed(self);
                }
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

            fn unpack_loot_table(&self, player: Option<&Player>) {
                $name::unpack_loot_table(self, player);
            }

            fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
                // Vanilla `Containers.dropContents` reads the container through
                // `RandomizableContainerBlockEntity.getItem`, which unpacks a
                // pending loot table first.
                self.unpack_loot_table(None);
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
                container.loot_table = nbt_view
                    .string("LootTable")
                    .and_then(|value| Identifier::from_str(&value.to_str()).ok());
                container.loot_table_seed = nbt_view.long("LootTableSeed").unwrap_or(0);

                // Vanilla `tryLoadLootTable`: a pending loot table replaces the stored items.
                if container.loot_table.is_some() {
                    return;
                }

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
                // Vanilla `trySaveLootTable`: an unopened loot table persists instead of items.
                if let Some(loot_table) = &container.loot_table {
                    nbt.insert("LootTable", loot_table.to_string());
                    // Vanilla only writes the seed when it is non-zero.
                    if container.loot_table_seed != 0 {
                        nbt.insert("LootTableSeed", container.loot_table_seed);
                    }
                    return;
                }

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
    use std::io::Cursor;

    use simdnbt::borrow::read_compound;
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

    fn load_nbt(block_entity: &DispenserBlockEntity, nbt: &NbtCompound) {
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed =
            read_compound(&mut Cursor::new(bytes.as_slice())).expect("test NBT should reborrow");
        block_entity.load_additional(&borrowed);
    }

    #[test]
    fn pending_loot_table_round_trips_instead_of_items() {
        let source = test_dispenser();
        {
            let mut container = source.container.lock();
            container.items[0] = ItemStack::new(&vanilla_items::ARROW);
            container.loot_table = Some(Identifier::vanilla_static("chests/simple_dungeon"));
            container.loot_table_seed = 42;
        }

        let mut saved = NbtCompound::new();
        source.save_additional(&mut saved);

        assert_eq!(
            saved.string("LootTable").map(ToString::to_string),
            Some("minecraft:chests/simple_dungeon".to_string())
        );
        assert_eq!(saved.long("LootTableSeed"), Some(42));
        assert!(saved.list("Items").is_none());

        let loaded = test_dispenser();
        load_nbt(&loaded, &saved);
        let container = loaded.container.lock();
        assert_eq!(
            container.loot_table,
            Some(Identifier::vanilla_static("chests/simple_dungeon"))
        );
        assert_eq!(container.loot_table_seed, 42);
        assert!(container.items.iter().all(ItemStack::is_empty));
    }

    #[test]
    fn zero_loot_table_seed_is_not_persisted() {
        let dispenser = test_dispenser();
        {
            let mut container = dispenser.container.lock();
            container.loot_table = Some(Identifier::vanilla_static("chests/simple_dungeon"));
        }

        let mut saved = NbtCompound::new();
        dispenser.save_additional(&mut saved);

        assert!(saved.long("LootTableSeed").is_none());
    }

    #[test]
    fn random_slot_unpacks_the_pending_loot_table() {
        let dispenser = test_dispenser();
        {
            let mut container = dispenser.container.lock();
            container.loot_table = Some(Identifier::vanilla_static("blocks/stone"));
            container.loot_table_seed = 7;
        }

        let slot = dispenser.get_random_slot();

        let container = dispenser.container.lock();
        assert!(container.loot_table.is_none());
        assert_eq!(
            container
                .items
                .iter()
                .filter(|item| !item.is_empty())
                .count(),
            1
        );
        let slot = slot.expect("unpacked loot table should fill a slot");
        assert!(container.items[slot].is(&vanilla_items::COBBLESTONE));
    }
}
