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

use rand::SeedableRng as _;
use rand::rngs::StdRng;
use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, ChestType};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::{LootContext, LootTableRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{
    REGISTRY, RegistryExt as _, sound_events, vanilla_block_entity_types, vanilla_blocks,
};
use steel_utils::types::GameType;
use steel_utils::{
    BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex,
};

use crate::behavior::advancement_hooks::raise_generate_loot;
use crate::block_entity::entities::openers_counter::{
    ContainerOpenersCounter, ContainerViewers, count_viewers,
};
use crate::block_entity::entities::{shuffle, shuffle_and_split_items};
use crate::block_entity::{
    BlockEntity, BlockEntityBase, BlockEntityLifecycleExt as _, ContainerOpenersHost,
};
use crate::entity::{LivingEntity as _, entity_loot_ref, living_loot_equipment};
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
            .and_then(|entity| entity.container_viewers().map(ContainerViewers::open_count))
            .unwrap_or(0)
    }

    /// Vanilla `ChestBlockEntity.playSound`.
    fn play_sound(world: &Arc<World>, pos: BlockPos, state: BlockStateId, sound: SoundEventRef) {
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

    /// Vanilla `RandomizableContainer.unpackLootTable`.
    ///
    /// Rolls a pending `LootTable` into the chest's empty slots and clears the
    /// table, so an unopened container generates its loot exactly once, on the
    /// first read. Vanilla runs this from every container read (`isEmpty`,
    /// `getItem`, `removeItem`, `setItem`) and from both of `ChestBlock`'s menu
    /// providers; Steel's callers invoke it explicitly because the container
    /// capability is read through [`ContainerRef`] rather than through the
    /// block entity's own container.
    ///
    /// `player` is the container's opener the way vanilla's chest menu
    /// providers pass `Inventory.player`: the `GENERATE_LOOT` advancement fires
    /// and the opener's luck applies. Reads no player drives (a comparator,
    /// breaking the chest) pass `None`, exactly like vanilla's reads with a
    /// `null` player.
    pub fn unpack_loot_table(&self, player: Option<&Player>) {
        let Some(world) = self.get_level() else {
            return;
        };
        // Vanilla clears the table before rolling it, so it unpacks only once.
        let (loot_table_key, seed) = {
            let mut container = self.container.lock();
            let Some(loot_table_key) = container.loot_table.take() else {
                return;
            };
            (loot_table_key, container.loot_table_seed)
        };
        // Vanilla resolves the key through the reloadable registries, which
        // always yield a table (the empty one for an unknown key).
        let Some(loot_table) = REGISTRY.loot_tables.by_key(&loot_table_key) else {
            log::warn!("Chest has unknown loot table {loot_table_key}");
            return;
        };
        // Vanilla fires `GENERATE_LOOT` before the loot is rolled.
        if let Some(player) = player {
            raise_generate_loot(player, &world, &loot_table_key);
        }

        let pos = self.get_block_pos();
        // Vanilla `Vec3.atCenterOf(this.worldPosition)`.
        let origin = (
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + 0.5,
            f64::from(pos.z()) + 0.5,
        );
        // Vanilla `LootTable.fill` seeds the context with `lootTableSeed` when
        // it is non-zero and falls back to the level's random source otherwise.
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

    /// Vanilla `LootTable.fill` against the chest's empty slots.
    ///
    /// The loot context carries what vanilla's chest parameter set carries: the
    /// block centre as `ORIGIN` and, when a player opened the container, luck
    /// and `THIS_ENTITY`. The slot distribution shares the dispenser family's
    /// `Util.shuffle` + `LootTable.shuffleAndSplitItems`.
    fn fill_from_loot_table<R: rand::Rng>(
        &self,
        loot_table: LootTableRef,
        origin: (f64, f64, f64),
        player: Option<&Player>,
        rng: &mut R,
    ) {
        let equipment = player.map(living_loot_equipment);
        let items = {
            let mut context = LootContext::new(rng).with_origin(origin.0, origin.1, origin.2);
            if let (Some(player), Some(equipment)) = (player, equipment.as_ref()) {
                context = context
                    .with_luck(player.get_luck())
                    .with_this_entity(entity_loot_ref(player, Some(equipment)));
            }
            loot_table.get_random_items(&mut context)
        };

        let mut container = self.container.lock();
        let mut available_slots: Vec<usize> = (0..CHEST_SLOTS)
            .filter(|&slot| container.items[slot].is_empty())
            .collect();
        shuffle(&mut available_slots, rng);
        let items = shuffle_and_split_items(items, available_slots.len(), rng);
        let placed = !items.is_empty();
        for item in items {
            let Some(slot) = available_slots.pop() else {
                log::warn!("Tried to over-fill a chest");
                break;
            };
            container.set_item(slot, item);
        }
        drop(container);
        // Vanilla `setItem` marks the container changed; nothing placed leaves
        // the chest untouched.
        if placed {
            BlockEntity::set_changed(self);
        }
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

    /// Vanilla trapped chests share `ChestBlockEntity`'s loot table unpacking;
    /// only the block entity type differs.
    pub fn unpack_loot_table(&self, player: Option<&Player>) {
        self.inner.unpack_loot_table(player);
    }
}

impl BlockEntity for ChestBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn unpack_loot_table(&self, player: Option<&Player>) {
        ChestBlockEntity::unpack_loot_table(self, player);
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        // Vanilla `Containers.dropContents` reads the container through
        // `RandomizableContainerBlockEntity.getItem`, which unpacks a pending
        // loot table first. Dropping the contents does not fire `GENERATE_LOOT`
        // because no player drives the read.
        self.unpack_loot_table(None);
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

/// Vanilla `CopperChestBlock.getHingeSound`: copper chests use the hinge sound of
/// their oxidation stage, every other chest the wooden one.
fn hinge_sound(state: BlockStateId, open: bool) -> SoundEventRef {
    let block = state.get_block();
    if block.has_tag(&BlockTag::COPPER_CHESTS) {
        if block == &vanilla_blocks::WEATHERED_COPPER_CHEST
            || block == &vanilla_blocks::WAXED_WEATHERED_COPPER_CHEST
        {
            if open {
                &sound_events::BLOCK_COPPER_CHEST_WEATHERED_OPEN
            } else {
                &sound_events::BLOCK_COPPER_CHEST_WEATHERED_CLOSE
            }
        } else if block == &vanilla_blocks::OXIDIZED_COPPER_CHEST
            || block == &vanilla_blocks::WAXED_OXIDIZED_COPPER_CHEST
        {
            if open {
                &sound_events::BLOCK_COPPER_CHEST_OXIDIZED_OPEN
            } else {
                &sound_events::BLOCK_COPPER_CHEST_OXIDIZED_CLOSE
            }
        } else if open {
            &sound_events::BLOCK_COPPER_CHEST_OPEN
        } else {
            &sound_events::BLOCK_COPPER_CHEST_CLOSE
        }
    } else if open {
        &sound_events::BLOCK_CHEST_OPEN
    } else {
        &sound_events::BLOCK_CHEST_CLOSE
    }
}

impl ContainerOpenersHost for ChestBlockEntity {
    /// Vanilla `ChestBlockEntity.openersCounter.onOpen`.
    fn on_open(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        Self::play_sound(world, pos, state, hinge_sound(state, true));
    }

    /// Vanilla `ChestBlockEntity.openersCounter.onClose`.
    fn on_close(&self, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        Self::play_sound(world, pos, state, hinge_sound(state, false));
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
        // Vanilla passes `containerUser.getContainerInteractionRange()`.
        let transition = self
            .openers
            .lock()
            .increment(player.block_interaction_range());
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

    fn unpack_loot_table(&self, player: Option<&Player>) {
        TrappedChestBlockEntity::unpack_loot_table(self, player);
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
    use std::io::Cursor;

    use simdnbt::borrow::read_compound;
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
        let borrowed = read_compound(&mut Cursor::new(encoded.as_slice()))
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
    fn zero_loot_table_seed_is_omitted_from_saved_nbt() {
        let chest = test_chest();
        {
            let mut container = chest.container.lock();
            container.loot_table = Some(Identifier::vanilla_static("chests/simple_dungeon"));
        }

        let mut nbt = NbtCompound::new();
        chest.save_additional(&mut nbt);

        assert!(nbt.get("LootTableSeed").is_none());
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
        let borrowed = read_compound(&mut Cursor::new(encoded.as_slice()))
            .expect("saved chest NBT should reparse");
        chest.load_additional(&borrowed);

        let container = chest.container.lock();
        assert!(container.loot_table.is_none());
        assert_eq!(container.items[4].count(), 7);
    }
}
