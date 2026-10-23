//! Chest and trapped-chest block entities.
//!
//! Both hold the same 27-slot container. Vanilla's double chest is not a block
//! entity of its own: it is a `CompoundContainer` view over two neighboring
//! chests, assembled by [`crate::inventory::menu::kinds::double_chest`] from the
//! two halves' [`crate::inventory::lock::ContainerRef`]s.

use std::{
    mem,
    str::FromStr as _,
    sync::{Arc, Weak},
};

use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{sound_events, vanilla_block_entity_types};
use steel_utils::types::GameType;
use steel_utils::{
    BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex,
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
use crate::world::{LevelReader, World};

/// Number of slots in one chest half (3 rows of 9).
pub const CHEST_SLOTS: usize = 27;

/// Chest block entity.
///
/// Holds one 27-slot container. A double chest is two of these, each with its
/// own independent storage, exactly like vanilla.
pub struct ChestBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<ChestContainer>>,
    container_ref: ContainerRef,
    /// Vanilla `ChestBlockEntity.openersCounter`.
    openers: SyncMutex<ContainerOpenersCounter>,
}

/// Trapped chest block entity.
///
/// Shares the chest storage and opener accounting; only the block entity type
/// and the redstone behavior of the owning block differ.
pub struct TrappedChestBlockEntity {
    inner: ChestBlockEntity,
}

struct ChestContainer {
    items: Vec<ItemStack>,
    /// Vanilla `RandomizableContainerBlockEntity.lootTable`.
    loot_table: Option<Identifier>,
    /// Vanilla `RandomizableContainerBlockEntity.lootTableSeed`.
    loot_table_seed: i64,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ChestBlockEntity`.
unsafe impl DowncastType for ChestBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/chest");
}

// SAFETY: This key is owned by Steel and uniquely identifies `TrappedChestBlockEntity`.
unsafe impl DowncastType for TrappedChestBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/trapped_chest");
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a chest block entity.
unsafe impl DowncastType for ChestContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/chest");
}

impl ChestBlockEntity {
    /// Creates a new chest block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self::with_type(&vanilla_block_entity_types::CHEST, level, pos, state)
    }

    fn with_type(
        block_entity_type: BlockEntityTypeRef,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Self {
        let base = Arc::new(BlockEntityBase::new(block_entity_type, level, pos, state));
        let container = Arc::new(SyncMutex::new(ChestContainer {
            items: vec![ItemStack::empty(); CHEST_SLOTS],
            loot_table: None,
            loot_table_seed: 0,
        }));
        let shared_container: SharedContainer = Arc::<SyncMutex<ChestContainer>>::clone(&container);
        Self {
            container_ref: ContainerRef::owned_by_block_entity(shared_container, Arc::clone(&base)),
            base,
            container,
            openers: SyncMutex::new(ContainerOpenersCounter::new()),
        }
    }

    /// Vanilla `ChestBlockEntity.getOpenCount`: the opener count of the chest at
    /// `pos`, or zero when that block has no chest block entity.
    #[must_use]
    pub fn open_count(level: &dyn LevelReader, pos: BlockPos) -> i32 {
        level
            .get_block_entity(pos)
            .and_then(|entity| {
                entity
                    .container_viewers()
                    .map(|viewers| viewers.open_count())
            })
            .unwrap_or(0)
    }

    /// Vanilla `ChestBlockEntity.playSound`.
    fn play_sound(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        sound: SoundEventRef,
    ) {
        let chest_type = state.get_value(&BlockStateProperties::CHEST_TYPE);
        // Vanilla skips the left half so a double chest only sounds once.
        if chest_type == ChestType::Left {
            return;
        }

        let mut x = f64::from(pos.x()) + 0.5;
        let mut z = f64::from(pos.z()) + 0.5;
        if chest_type == ChestType::Right {
            let (dx, _, dz) = connected_direction(state).offset();
            x += f64::from(dx) * 0.5;
            z += f64::from(dz) * 0.5;
        }

        world.play_sound_at(
            sound,
            SoundSource::Blocks,
            glam::DVec3::new(x, f64::from(pos.y()) + 0.5, z),
            0.5,
            rand::random::<f32>() * 0.1 + 0.9,
            None,
        );
    }
}

/// Vanilla `ChestBlock.getConnectedDirection`: the direction of the other half.
fn connected_direction(state: BlockStateId) -> Direction {
    let facing = state.get_value(&BlockStateProperties::FACING);
    if state.get_value(&BlockStateProperties::CHEST_TYPE) == ChestType::Left {
        facing.rotate_y_clockwise()
    } else {
        facing.rotate_y_counter_clockwise()
    }
}

impl TrappedChestBlockEntity {
    /// Creates a new trapped chest block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            inner: ChestBlockEntity::with_type(
                &vanilla_block_entity_types::TRAPPED_CHEST,
                level,
                pos,
                state,
            ),
        }
    }
}

impl BlockEntity for ChestBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        let items = {
            let mut container = self.container.lock();
            mem::replace(&mut container.items, vec![ItemStack::empty(); CHEST_SLOTS])
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
                    if slot < CHEST_SLOTS
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
            nbt.insert("LootTableSeed", container.loot_table_seed);
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

    fn trigger_event(&self, param_a: i32, _param_b: i32) -> bool {
        // Vanilla's lid animation (`ChestLidController`) only runs on the client,
        // where the broadcast block event drives it. Accepting event 1 here is
        // what makes the server publish it.
        param_a == 1
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }

    fn container_viewers(&self) -> Option<&dyn ContainerViewers> {
        Some(self)
    }
}

impl ContainerOpenersHost for ChestBlockEntity {
    /// Vanilla `ChestBlockEntity.openersCounter.onOpen`.
    fn on_open(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        self.play_sound(world, pos, state, &sound_events::BLOCK_CHEST_OPEN);
    }

    /// Vanilla `ChestBlockEntity.openersCounter.onClose`.
    fn on_close(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        self.play_sound(world, pos, state, &sound_events::BLOCK_CHEST_CLOSE);
    }

    /// Vanilla `ChestBlockEntity.signalOpenCount`.
    fn opener_count_changed(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        _previous: i32,
        current: i32,
    ) {
        world.block_event(pos, state.get_block(), 1, current);
    }

    fn is_own_container(&self, player: &Player) -> bool {
        player.has_open_container(&self.container_ref)
    }
}

impl ContainerViewers for ChestBlockEntity {
    /// Vanilla `ChestBlockEntity.startOpen`.
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
        let transition = self.openers.lock().increment(4.0);
        ContainerOpenersCounter::apply(self, transition, Some(player), &world, pos, state);
    }

    /// Vanilla `ChestBlockEntity.stopOpen`.
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

    /// Vanilla `ChestBlockEntity.recheckOpen`, driven from the block's
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
        let open_count = count_viewers(&world, pos, self);
        let transition = self.openers.lock().recheck(open_count);
        ContainerOpenersCounter::apply(self, transition, None, &world, pos, state);
    }
}

impl BlockEntity for TrappedChestBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        self.inner.base()
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, state: BlockStateId) {
        self.inner.pre_remove_side_effects(pos, state);
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        self.inner.load_additional(nbt);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.inner.save_additional(nbt);
    }

    fn trigger_event(&self, param_a: i32, param_b: i32) -> bool {
        self.inner.trigger_event(param_a, param_b)
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        self.inner.container_ref()
    }

    fn container_viewers(&self) -> Option<&dyn ContainerViewers> {
        Some(&self.inner)
    }
}

impl Container for ChestContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        CHEST_SLOTS
    }

    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot < CHEST_SLOTS {
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
    use simdnbt::owned::NbtCompound;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};

    use super::*;

    fn test_chest() -> ChestBlockEntity {
        init_vanilla_registry();
        ChestBlockEntity::new(
            Weak::new(),
            BlockPos::new(1, 2, 3),
            vanilla_blocks::CHEST.default_state(),
        )
    }

    #[test]
    fn set_item_limits_stack_to_vanilla_container_maximum() {
        let chest = test_chest();
        chest
            .container
            .lock()
            .set_item(0, ItemStack::with_count(&vanilla_items::STONE, 100));

        assert_eq!(chest.container.lock().get_item(0).count(), 64);
    }

    #[test]
    fn loot_table_round_trips_instead_of_items() {
        let chest = test_chest();
        {
            let mut container = chest.container.lock();
            container.items[0] = ItemStack::with_count(&vanilla_items::STONE, 3);
            container.loot_table = Some(Identifier::vanilla_static("chests/simple_dungeon"));
            container.loot_table_seed = 42;
        }

        let mut nbt = NbtCompound::new();
        chest.save_additional(&mut nbt);

        let mut encoded = Vec::new();
        nbt.write(&mut encoded);
        let borrowed =
            simdnbt::borrow::read_compound(&mut std::io::Cursor::new(encoded.as_slice()))
                .expect("saved chest NBT should reparse");
        chest.load_additional(&borrowed);

        let container = chest.container.lock();
        assert_eq!(
            container.loot_table,
            Some(Identifier::vanilla_static("chests/simple_dungeon"))
        );
        assert_eq!(container.loot_table_seed, 42);
        assert!(container.items.iter().all(ItemStack::is_empty));
    }

    #[test]
    fn items_survive_save_and_load_when_no_loot_table_is_pending() {
        let chest = test_chest();
        chest
            .container
            .lock()
            .set_item(4, ItemStack::with_count(&vanilla_items::STONE, 7));

        let mut nbt = NbtCompound::new();
        chest.save_additional(&mut nbt);
        let mut encoded = Vec::new();
        nbt.write(&mut encoded);
        let borrowed =
            simdnbt::borrow::read_compound(&mut std::io::Cursor::new(encoded.as_slice()))
                .expect("saved chest NBT should reparse");
        chest.load_additional(&borrowed);

        let container = chest.container.lock();
        assert!(container.loot_table.is_none());
        assert_eq!(container.items[4].count(), 7);
    }
}
