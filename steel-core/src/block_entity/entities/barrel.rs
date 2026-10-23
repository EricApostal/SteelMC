//! Barrel block entity implementation.
//!
//! Barrels are container block entities with 27 slots (3x9 grid),
//! functioning similarly to chests but without double-chest behavior.

use std::{
    mem,
    str::FromStr as _,
    sync::{Arc, Weak},
};

use glam::DVec3;
use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{sound_events, vanilla_block_entity_types};
use steel_utils::types::{GameType, UpdateFlags};
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex,
};

use crate::block_entity::entities::openers_counter::{
    ContainerOpenersCounter, ContainerViewers, count_viewers,
};
use crate::block_entity::{
    BlockEntity, BlockEntityBase, BlockEntityLifecycleExt as _, ContainerOpenersHost,
};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerRef, SharedContainer};
use crate::player::Player;
use crate::world::{LevelAccessor as _, World};

/// Number of slots in a barrel (3 rows of 9).
pub const BARREL_SLOTS: usize = 27;

/// Barrel block entity.
///
/// A simple container with 27 slots, using the same menu as chests.
pub struct BarrelBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<BarrelContainer>>,
    container_ref: ContainerRef,
    /// Vanilla `BarrelBlockEntity.openersCounter`.
    openers: SyncMutex<ContainerOpenersCounter>,
}

struct BarrelContainer {
    items: Vec<ItemStack>,
    /// Vanilla `RandomizableContainerBlockEntity.lootTable`.
    loot_table: Option<Identifier>,
    /// Vanilla `RandomizableContainerBlockEntity.lootTableSeed`.
    loot_table_seed: i64,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BarrelBlockEntity`.
unsafe impl DowncastType for BarrelBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/barrel");
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a barrel block entity.
unsafe impl DowncastType for BarrelContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/barrel");
}

impl BarrelBlockEntity {
    /// Creates a new barrel block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::BARREL,
            level,
            pos,
            state,
        ));
        let container = Arc::new(SyncMutex::new(BarrelContainer {
            items: vec![ItemStack::empty(); BARREL_SLOTS],
            loot_table: None,
            loot_table_seed: 0,
        }));
        let shared_container: SharedContainer =
            Arc::<SyncMutex<BarrelContainer>>::clone(&container);
        Self {
            container_ref: ContainerRef::owned_by_block_entity(shared_container, Arc::clone(&base)),
            base,
            container,
            openers: SyncMutex::new(ContainerOpenersCounter::new()),
        }
    }
}

impl BlockEntity for BarrelBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        let items = {
            let mut container = self.container.lock();
            mem::replace(&mut container.items, vec![ItemStack::empty(); BARREL_SLOTS])
        };
        let Some(world) = self.get_level() else {
            return;
        };
        for item in items {
            world.drop_item_stack(pos, item);
        }
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        // Convert to NbtCompound view for accessing methods
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

        // Load items from NBT using borrowed NBT for proper ItemStack parsing
        if let Some(items_list) = nbt_view.list("Items")
            && let Some(compounds) = items_list.compounds()
        {
            for compound in compounds {
                // Each item has a "Slot" byte and item data
                if let Some(slot) = compound.byte("Slot") {
                    let slot = slot as usize;
                    if slot < BARREL_SLOTS {
                        // Parse item directly from the borrowed compound
                        if let Some(item) = ItemStack::from_borrowed_compound(&compound) {
                            container.items[slot] = item;
                        }
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

        // Save items to NBT (only non-empty slots)
        let mut items: Vec<NbtCompound> = Vec::new();
        for (slot, item) in container.items.iter().enumerate() {
            if !item.is_empty() {
                // Use ItemStack's ToNbtTag implementation for proper component serialization
                if let NbtTag::Compound(mut item_nbt) = item.clone().to_nbt_tag() {
                    item_nbt.insert("Slot", slot as i8);
                    items.push(item_nbt);
                }
            }
        }
        nbt.insert("Items", NbtList::Compound(items));
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }

    fn container_viewers(&self) -> Option<&dyn ContainerViewers> {
        Some(self)
    }
}

impl ContainerViewers for BarrelBlockEntity {
    /// Vanilla `BarrelBlockEntity.startOpen`.
    fn start_open(&self, player: &Player) {
        if self.is_removed() || player.game_mode() == GameType::Spectator {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        // Compute under the counter lock, run the world-writing hooks after.
        // Vanilla passes `containerUser.getContainerInteractionRange()`.
        let transition = self
            .openers
            .lock()
            .increment(player.block_interaction_range());
        ContainerOpenersCounter::apply(self, transition, Some(player), &world, pos, state);
    }

    /// Vanilla `BarrelBlockEntity.stopOpen`.
    fn stop_open(&self, player: &Player) {
        if self.is_removed() || player.game_mode() == GameType::Spectator {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let transition = self.openers.lock().decrement();
        ContainerOpenersCounter::apply(self, transition, Some(player), &world, pos, state);
    }

    fn open_count(&self) -> i32 {
        self.openers.lock().open_count()
    }

    /// Vanilla `BarrelBlockEntity.recheckOpen`, driven from the block's
    /// scheduled tick.
    fn recheck_open(&self) {
        if self.is_removed() {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let max_interaction_range = self.openers.lock().max_interaction_range();
        let (open_count, max_interaction_range) =
            count_viewers(&world, pos, self, max_interaction_range);
        let transition = self
            .openers
            .lock()
            .recheck(open_count, max_interaction_range);
        ContainerOpenersCounter::apply(self, transition, None, &world, pos, state);
    }
}

impl ContainerOpenersHost for BarrelBlockEntity {
    /// Vanilla `BarrelBlockEntity.openersCounter.onOpen`.
    fn on_open(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        self.play_sound(world, pos, state, &sound_events::BLOCK_BARREL_OPEN);
        world.set_block_state(
            pos,
            state.set_value(&BlockStateProperties::OPEN, true),
            UpdateFlags::UPDATE_ALL,
        );
    }

    /// Vanilla `BarrelBlockEntity.openersCounter.onClose`.
    fn on_close(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        self.play_sound(world, pos, state, &sound_events::BLOCK_BARREL_CLOSE);
        world.set_block_state(
            pos,
            state.set_value(&BlockStateProperties::OPEN, false),
            UpdateFlags::UPDATE_ALL,
        );
    }

    fn opener_count_changed(
        &self,
        _world: &Arc<World>,
        _pos: BlockPos,
        _state: BlockStateId,
        _previous: i32,
        _current: i32,
    ) {
        // Vanilla's barrel counter has no opener-count callback.
    }

    fn is_own_container(&self, player: &Player) -> bool {
        player.has_open_container(&self.container_ref)
    }
}

impl BarrelBlockEntity {
    /// Vanilla `BarrelBlockEntity.playSound`: emits from the faced side.
    fn play_sound(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        sound: SoundEventRef,
    ) {
        let direction = state.get_value(&BlockStateProperties::FACING);
        let (dx, dy, dz) = direction.offset();
        let position = DVec3::new(
            f64::from(pos.x()) + 0.5 + f64::from(dx) / 2.0,
            f64::from(pos.y()) + 0.5 + f64::from(dy) / 2.0,
            f64::from(pos.z()) + 0.5 + f64::from(dz) / 2.0,
        );
        world.play_sound_at(
            sound,
            SoundSource::Blocks,
            position,
            0.5,
            rand::random::<f32>() * 0.1 + 0.9,
            None,
        );
    }
}

impl Container for BarrelContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        BARREL_SLOTS
    }

    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot < BARREL_SLOTS {
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

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};

    use super::*;

    fn test_barrel() -> BarrelBlockEntity {
        init_vanilla_registry();
        BarrelBlockEntity::new(
            Weak::new(),
            BlockPos::new(1, 2, 3),
            vanilla_blocks::BARREL.default_state(),
        )
    }

    #[test]
    fn set_item_limits_stack_to_vanilla_container_maximum() {
        let barrel = test_barrel();
        barrel
            .container
            .lock()
            .set_item(0, ItemStack::with_count(&vanilla_items::STONE, 100));

        assert_eq!(barrel.container.lock().get_item(0).count(), 64);
    }

    #[test]
    fn loot_table_round_trips_instead_of_items() {
        let barrel = test_barrel();
        {
            let mut container = barrel.container.lock();
            container.items[0] = ItemStack::with_count(&vanilla_items::STONE, 3);
            container.loot_table = Some(Identifier::vanilla_static("chests/simple_dungeon"));
            container.loot_table_seed = 42;
        }

        let mut nbt = NbtCompound::new();
        barrel.save_additional(&mut nbt);

        let mut encoded = Vec::new();
        nbt.write(&mut encoded);
        let borrowed =
            simdnbt::borrow::read_compound(&mut std::io::Cursor::new(encoded.as_slice()))
                .expect("saved barrel NBT should reparse");
        barrel.load_additional(&borrowed);

        let container = barrel.container.lock();
        assert_eq!(
            container.loot_table,
            Some(Identifier::vanilla_static("chests/simple_dungeon"))
        );
        assert_eq!(container.loot_table_seed, 42);
        assert!(container.items.iter().all(ItemStack::is_empty));
    }

    #[test]
    fn zero_loot_table_seed_is_omitted_from_saved_nbt() {
        let barrel = test_barrel();
        {
            let mut container = barrel.container.lock();
            container.loot_table = Some(Identifier::vanilla_static("chests/simple_dungeon"));
        }

        let mut nbt = NbtCompound::new();
        barrel.save_additional(&mut nbt);

        assert!(nbt.get("LootTableSeed").is_none());
    }

    #[test]
    fn pre_remove_preserves_slots_for_existing_menu_references() {
        let barrel = test_barrel();
        barrel
            .container
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::STONE));

        barrel.pre_remove_side_effects(
            BlockPos::new(1, 2, 3),
            vanilla_blocks::BARREL.default_state(),
        );

        let container = barrel.container.lock();
        assert_eq!(container.items.len(), BARREL_SLOTS);
        assert!(container.items.iter().all(ItemStack::is_empty));
    }
}
