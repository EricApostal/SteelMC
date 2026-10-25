//! Chest minecart entity (`net.minecraft.world.entity.vehicle.minecart.MinecartChest`).
//!
//! Vanilla's `MinecartChest` extends `AbstractMinecartContainer`: a 27-slot
//! container cart with a deferred loot table, a chest display block, and a
//! three-row chest interaction. It rides rails through the shared
//! [`super::minecart_base`] physics.
//!
//! Deliberate differences from vanilla:
//! - Vanilla switches to the experimental `NewMinecartBehavior` when the
//!   `minecart_improvements` feature flag is enabled; Steel always uses the
//!   stable `OldMinecartBehavior` physics implemented here.
//! - `PiglinAi.angerNearbyPiglins` on open and on destroy is not modelled;
//!   Steel has no piglin AI yet.
//! - `AbstractMinecartContainer.remove` drops the contents when the removal
//!   reason destroys entities; Steel's removal path only destroys through
//!   [`Minecart::on_minecart_destroyed`] (`destroy`/`kill`), like the other
//!   container carts.

use std::mem;
use std::str::FromStr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use rand::{SeedableRng as _, rngs::StdRng};
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::loot_table::{LootContext, LootTableRef};
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, ChestMinecartEntityData};
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_items, vanilla_menu_types};
use steel_registry::{vanilla_blocks, vanilla_game_events, vanilla_game_rules};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::{IntoShared as _, Shared, SyncMutex};
use steel_utils::types::InteractionHand;
use steel_utils::{BlockStateId, DowncastType, DowncastTypeKey, Identifier, translations};
use text_components::TextComponent;

use crate::behavior::InteractionResult;
use crate::behavior::advancement_hooks::raise_generate_loot;
use crate::block_entity::entities::{shuffle, shuffle_and_split_items};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, LivingEntity as _, WeakEntity,
    entity_loot_ref, living_loot_equipment,
};
use crate::inventory::container::{
    Container, SimpleContainer, calculate_redstone_signal_from_container,
};
use crate::inventory::lock::ContainerRef;
use crate::inventory::menu::{FillDirection, Menu, MenuBehavior, MenuBuilder, MenuKind};
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;
use crate::world::World;

use super::minecart_base::{self, Minecart, MinecartState};

/// Vanilla `MinecartChest.getContainerSize`: 27 slots (three rows of nine).
pub const CHEST_MINECART_SLOTS: usize = 27;
/// Vanilla `MinecartChest.getDefaultDisplayOffset`.
const CHEST_DISPLAY_OFFSET: i32 = 8;

/// A 27-slot container chest minecart with a deferred loot table.
#[entity_behavior(class = "MinecartChest")]
pub struct ChestMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<ChestMinecartEntityData>,
    state: SyncMutex<MinecartState>,
    inventory: Shared<SimpleContainer>,
    loot: SyncMutex<ChestMinecartLoot>,
}

/// Vanilla `AbstractMinecartContainer`'s pending loot: written as
/// `LootTable`/`LootTableSeed` instead of `Items` until it is unpacked.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChestMinecartLoot {
    loot_table: Option<Identifier>,
    loot_table_seed: i64,
}

impl ChestMinecartLoot {
    const fn new() -> Self {
        Self {
            loot_table: None,
            loot_table_seed: 0,
        }
    }
}

// SAFETY: This key is owned by Steel and uniquely identifies `ChestMinecartEntity`.
unsafe impl DowncastType for ChestMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/chest_minecart");
}

// SAFETY: This Steel-owned key uniquely identifies the chest-minecart menu kind.
unsafe impl DowncastType for ChestMinecartMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/chest_minecart");
}

/// Vanilla `MinecartChest.createMenu` + `ContainerEntity.isChestVehicleStillValid`:
/// the shared three-row chest layout over the cart's container, which closes
/// when the cart is gone or the player walks out of entity interaction range.
struct ChestMinecartMenuKind {
    cart: WeakEntity,
}

impl MenuKind for ChestMinecartMenuKind {
    /// Vanilla `MinecartChest.stopOpen`: announce the close as a game event.
    fn removed(&mut self, _behavior: &mut MenuBehavior, player: &Player) {
        let Some(cart) = self.cart.upgrade() else {
            return;
        };
        cart.game_event_with_source_entity(&vanilla_game_events::CONTAINER_CLOSE, Some(player));
    }

    /// Vanilla `ContainerEntity.isChestVehicleStillValid`.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        let Some(cart) = self.cart.upgrade() else {
            return false;
        };
        !cart.is_removed() && player.is_within_entity_interaction_range(cart.bounding_box(), 4.0)
    }
}

/// Builds vanilla `ChestMenu.threeRows` over an entity-backed container.
fn build_chest_menu(
    container_id: u8,
    inventory: Shared<PlayerInventory>,
    container: ContainerRef,
    kind: ChestMinecartMenuKind,
) -> Menu {
    // Vanilla `ChestMenu.threeRows` uses the 3-row generic menu type over the
    // 27 cart slots plus the standard player inventory.
    let mut builder = MenuBuilder::new(&vanilla_menu_types::GENERIC_9X3, container_id);
    let chest = builder.section(container, CHEST_MINECART_SLOTS);
    let player = builder.player_inventory(&inventory);
    builder.route(chest, player.all(), FillDirection::Backward);
    builder.route(player.all(), chest, FillDirection::Forward);
    builder.build(kind)
}

impl ChestMinecartEntity {
    /// Creates a new chest minecart entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ChestMinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            inventory: SimpleContainer::new(CHEST_MINECART_SLOTS).into_shared(),
            loot: SyncMutex::new(ChestMinecartLoot::new()),
        }
    }

    /// Creates a chest minecart entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(ChestMinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            inventory: SimpleContainer::new(CHEST_MINECART_SLOTS).into_shared(),
            loot: SyncMutex::new(ChestMinecartLoot::new()),
        }
    }

    /// Sets the deferred loot table used when the container is first opened.
    ///
    /// Vanilla `AbstractMinecartContainer.setLootTable`: mineshaft generation
    /// stores a table here, and it is unpacked on the first container access.
    pub fn set_loot_table(&self, loot_table: Identifier, seed: i64) {
        let mut loot = self.loot.lock();
        loot.loot_table = Some(loot_table);
        loot.loot_table_seed = seed;
    }

    /// Returns the cart's container capability.
    ///
    /// Vanilla `AbstractMinecartContainer` implements `ContainerEntity`, so the
    /// cart is reachable through `HopperBlockEntity.getContainerAt` and
    /// `DetectorRailBlock.getAnalogOutputSignal`.
    #[must_use]
    pub fn container_ref(&self) -> ContainerRef {
        ContainerRef::from(&self.inventory)
    }

    /// Vanilla `ContainerEntity.unpackChestVehicleLootTable`: rolls the deferred
    /// loot table into the cart's slots the first time its contents are read.
    fn unpack_loot_table(&self, player: Option<&Player>) {
        let Some(loot_table_key) = self.loot.lock().loot_table.clone() else {
            return;
        };
        let loot_table = REGISTRY.loot_tables.by_key(&loot_table_key);

        // Vanilla triggers `GENERATE_LOOT` before the loot is rolled.
        if let Some(player) = player
            && let Some(world) = self.level()
        {
            raise_generate_loot(player, &world, &loot_table_key);
        }

        let seed = mem::take(&mut self.loot.lock().loot_table_seed);
        self.loot.lock().loot_table = None;
        if seed == 0 {
            self.fill_from_loot_table(loot_table, &mut rand::rng(), player);
        } else {
            self.fill_from_loot_table(loot_table, &mut StdRng::seed_from_u64(seed as u64), player);
        }
    }

    /// Vanilla `LootTable.fill` against the cart's empty slots. The slot
    /// distribution shares the dispenser family's `Util.shuffle` +
    /// `LootTable.shuffleAndSplitItems`.
    fn fill_from_loot_table<R: rand::Rng>(
        &self,
        loot_table: Option<LootTableRef>,
        rng: &mut R,
        player: Option<&Player>,
    ) {
        let Some(table) = loot_table else {
            return;
        };
        let origin = self.position();
        let equipment = player.map(living_loot_equipment);
        let items = {
            let mut context = LootContext::new(rng).with_origin(origin.x, origin.y, origin.z);
            if let (Some(player), Some(equipment)) = (player, equipment.as_ref()) {
                context = context
                    .with_luck(player.get_luck())
                    .with_this_entity(entity_loot_ref(player, Some(equipment)));
            }
            table.get_random_items(&mut context)
        };

        let mut inventory = self.inventory.lock();
        let mut available_slots: Vec<usize> = (0..CHEST_MINECART_SLOTS)
            .filter(|&slot| inventory.items()[slot].is_empty())
            .collect();
        shuffle(&mut available_slots, rng);
        let items = shuffle_and_split_items(items, available_slots.len(), rng);
        for item in items {
            let Some(slot) = available_slots.pop() else {
                log::warn!("Tried to over-fill a chest minecart");
                break;
            };
            inventory.set_item(slot, item);
        }
    }

    /// Writes the inventory as vanilla `Items` slot compounds.
    fn save_items(&self, nbt: &mut NbtCompound) {
        let inventory = self.inventory.lock();
        let mut items = Vec::new();
        for (slot, stack) in inventory.items().iter().enumerate() {
            if stack.is_empty() {
                continue;
            }
            if let NbtTag::Compound(mut item) = stack.to_nbt_tag_ref() {
                item.insert("Slot", slot as i8);
                items.push(item);
            }
        }
        nbt.insert("Items", NbtList::Compound(items));
    }

    /// Reads the vanilla `Items` slot compounds into the inventory.
    fn load_items(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        let mut inventory = self.inventory.lock();
        inventory.items_mut().fill(ItemStack::empty());
        let Some(items) = nbt.list("Items").and_then(|list| list.compounds()) else {
            return;
        };
        for compound in items {
            let Some(slot) = compound.byte("Slot").map(|slot| slot as usize) else {
                continue;
            };
            if slot < CHEST_MINECART_SLOTS
                && let Some(stack) = ItemStack::from_borrowed_compound(&compound)
            {
                inventory.items_mut()[slot] = stack;
            }
        }
    }

    /// Vanilla `ContainerEntity.addChestVehicleSaveData`.
    fn save_chest_vehicle_data(&self, nbt: &mut NbtCompound) {
        let loot = self.loot.lock();
        let Some(loot_table) = loot.loot_table.as_ref() else {
            drop(loot);
            self.save_items(nbt);
            return;
        };

        nbt.insert("LootTable", loot_table.to_string());
        if loot.loot_table_seed != 0 {
            nbt.insert("LootTableSeed", NbtTag::Long(loot.loot_table_seed));
        }
    }

    /// Vanilla `ContainerEntity.readChestVehicleSaveData`.
    fn load_chest_vehicle_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        {
            let mut inventory = self.inventory.lock();
            inventory.items_mut().fill(ItemStack::empty());
        }

        let loot_table = nbt
            .string("LootTable")
            .and_then(|value| Identifier::from_str(&value.to_string()).ok());
        let seed = nbt.long("LootTableSeed").unwrap_or(0);
        let deferred = loot_table.is_some();
        {
            let mut loot = self.loot.lock();
            loot.loot_table = loot_table;
            loot.loot_table_seed = seed;
        }

        // Vanilla only reads `Items` when no loot table is deferred.
        if !deferred {
            self.load_items(nbt);
        }
    }

    /// Vanilla `MinecartChest.interact` through
    /// `ContainerEntity.interactWithContainerVehicle`: unpack the loot table,
    /// open the three-row chest menu and announce the open.
    fn open_chest_menu(&self, player: &Player) -> InteractionResult {
        let Some(world) = self.level() else {
            return InteractionResult::Success;
        };
        let Some(cart) = world.get_entity_by_id(self.id()) else {
            return InteractionResult::Success;
        };
        let kind = ChestMinecartMenuKind {
            cart: Arc::downgrade(&cart),
        };

        // Vanilla `AbstractMinecartContainer.createMenu` unpacks before building.
        self.unpack_loot_table(Some(player));

        let inventory = Arc::clone(&player.inventory);
        let container = ContainerRef::from(&self.inventory);
        player.open_menu(
            TextComponent::translated(translations::CONTAINER_CHEST.msg()),
            move |context| build_chest_menu(context.container_id, inventory, container, kind),
        );
        self.game_event_with_source_entity(&vanilla_game_events::CONTAINER_OPEN, Some(player));
        InteractionResult::Success
    }
}

impl Minecart for ChestMinecartEntity {
    fn cart_state(&self) -> &SyncMutex<MinecartState> {
        &self.state
    }

    fn with_minecart_data<R>(&self, f: impl FnOnce(&AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart())
    }

    fn with_minecart_data_mut<R>(&self, f: impl FnOnce(&mut AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart_mut())
    }

    fn default_display_block_state(&self) -> BlockStateId {
        // Vanilla `MinecartChest.getDefaultDisplayBlockState`.
        vanilla_blocks::CHEST
            .default_state()
            .set_value(&BlockStateProperties::FACING, Direction::North)
    }

    fn default_display_offset(&self) -> i32 {
        CHEST_DISPLAY_OFFSET
    }

    fn drop_item(&self) -> Option<ItemRef> {
        Some(&vanilla_items::CHEST_MINECART)
    }

    /// Vanilla `AbstractMinecartContainer.applyNaturalSlowdown`: the emptiness
    /// term only applies once the deferred loot table has been unpacked.
    fn apply_natural_slowdown(&self, movement: DVec3) -> DVec3 {
        let mut keep = 0.98_f64;
        if self.loot.lock().loot_table.is_none() {
            let emptiness = 15 - calculate_redstone_signal_from_container(&*self.inventory.lock());
            keep += f64::from(emptiness) * 0.001;
        }

        if self.is_in_water() {
            keep *= 0.95;
        }

        DVec3::new(movement.x * keep, 0.0, movement.z * keep)
    }

    /// Vanilla `ContainerEntity.chestVehicleDestroyed`: unpack the pending loot
    /// table, then scatter the contents under `entity_drops`.
    fn on_minecart_destroyed(&self, world: &World) {
        self.unpack_loot_table(None);
        if !world.get_game_rule(&vanilla_game_rules::ENTITY_DROPS) {
            return;
        }

        let stacks: Vec<ItemStack> = self
            .inventory
            .lock()
            .items()
            .iter()
            .filter(|stack| !stack.is_empty())
            .cloned()
            .collect();
        for stack in stacks {
            self.spawn_at_location(stack, 0.0);
        }
    }
}

impl Entity for ChestMinecartEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn tick(self: Arc<Self>) {
        Arc::clone(&self).minecart_tick();
    }

    fn is_pickable(&self) -> bool {
        !self.is_removed()
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn is_on_rails(&self) -> bool {
        self.is_on_minecart_rails()
    }

    fn get_default_gravity(&self) -> f64 {
        self.minecart_default_gravity()
    }

    fn get_air_drag(&self) -> f64 {
        minecart_base::AIR_DRAG
    }

    fn dimension_changing_delay(&self) -> i32 {
        10
    }

    fn get_relative_portal_position(&self, axis: Axis, portal_area: FoundRectangle) -> DVec3 {
        minecart_base::minecart_relative_portal_position(self, axis, portal_area)
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.minecart_hurt(world, source, amount)
    }

    fn push_entity(&self, entity: &dyn Entity) {
        // Vanilla `AbstractMinecart.push`.
        self.minecart_push(entity);
    }

    fn interact(
        &self,
        player: &Player,
        _hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        self.open_chest_menu(player)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart_data(nbt);
        self.save_chest_vehicle_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
        self.load_chest_vehicle_data(nbt);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::{Arc, Weak};

    use simdnbt::borrow::read_compound as read_borrowed_compound;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_items};

    use super::*;
    use crate::entity::SharedEntity;
    use crate::test_support::fresh_test_world;

    fn test_cart(world: Weak<World>) -> ChestMinecartEntity {
        ChestMinecartEntity::new(
            &vanilla_entities::CHEST_MINECART,
            1,
            DVec3::new(8.5, 64.0, 8.5),
            world,
        )
    }

    #[test]
    fn chest_minecart_has_27_slots_and_vanilla_display_state() {
        let cart = test_cart(Weak::new());

        assert_eq!(cart.inventory.lock().get_container_size(), 27);
        assert_eq!(
            cart.display_block_state(),
            vanilla_blocks::CHEST
                .default_state()
                .set_value(&BlockStateProperties::FACING, Direction::North)
        );
        assert_eq!(cart.display_offset(), CHEST_DISPLAY_OFFSET);
        assert!(
            cart.drop_item()
                .is_some_and(|item| item.key == vanilla_items::CHEST_MINECART.key)
        );
    }

    /// Vanilla `ContainerEntity.addChestVehicleSaveData`: `Items` while nothing
    /// is deferred, `LootTable`/`LootTableSeed` instead of it afterwards.
    #[test]
    fn chest_vehicle_save_writes_items_or_the_deferred_loot_table() {
        let cart = test_cart(Weak::new());
        cart.inventory
            .lock()
            .set_item(3, ItemStack::with_count(&vanilla_items::DIAMOND, 2));

        let mut nbt = NbtCompound::new();
        cart.save_additional(&mut nbt);
        assert!(nbt.list("Items").is_some(), "items must be saved");
        assert!(nbt.string("LootTable").is_none());

        cart.set_loot_table(
            Identifier::new_static("minecraft", "chests/abandoned_mineshaft"),
            7,
        );
        let mut nbt = NbtCompound::new();
        cart.save_additional(&mut nbt);
        assert!(nbt.string("LootTable").is_some());
        assert!(nbt.long("LootTableSeed").is_some());
        assert!(
            nbt.list("Items").is_none(),
            "a deferred loot table replaces the item list"
        );
    }

    /// Vanilla `readChestVehicleSaveData` clears the container, restores the
    /// deferred table, and only reads `Items` when no table is pending.
    #[test]
    fn chest_vehicle_load_restores_items_when_no_loot_table_is_pending() {
        let cart = test_cart(Weak::new());
        cart.inventory
            .lock()
            .set_item(5, ItemStack::with_count(&vanilla_items::GOLD_INGOT, 3));

        let mut nbt = NbtCompound::new();
        cart.save_additional(&mut nbt);

        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = Cursor::new(bytes.as_slice());
        let view = read_borrowed_compound(&mut cursor)
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
        let restored = test_cart(Weak::new());
        restored.load_additional((&view).into());

        let stored = restored.inventory.lock();
        assert_eq!(stored.items().len(), CHEST_MINECART_SLOTS);
        assert_eq!(stored.get_item(5).item().key, vanilla_items::GOLD_INGOT.key);
        assert_eq!(stored.get_item(5).count(), 3);
        assert!(stored.get_item(0).is_empty());
        drop(stored);
        assert!(restored.loot.lock().loot_table.is_none());
    }

    /// Vanilla `AbstractMinecartContainer.applyNaturalSlowdown`: `0.98` plus the
    /// emptiness term, and the term is skipped while loot is still deferred.
    #[test]
    fn chest_minecart_slowdown_skips_the_empty_signal_while_loot_is_deferred() {
        let cart = test_cart(Weak::new());
        let movement = DVec3::new(1.0, 5.0, 1.0);

        let empty_keep = 0.98 + 15.0 * 0.001;
        let slowed = cart.apply_natural_slowdown(movement);
        assert!((slowed.x - empty_keep).abs() < 1.0e-9);
        assert_eq!(slowed.y, 0.0);

        cart.inventory
            .lock()
            .set_item(0, ItemStack::with_count(&vanilla_items::DIAMOND_BLOCK, 1));
        let half_full = cart.apply_natural_slowdown(movement);
        assert!(half_full.x < empty_keep);

        cart.set_loot_table(
            Identifier::new_static("minecraft", "chests/abandoned_mineshaft"),
            0,
        );
        let deferred = cart.apply_natural_slowdown(movement);
        assert!((deferred.x - 0.98).abs() < 1.0e-9);
    }

    /// Vanilla `ChestMenu.threeRows` (used by `MinecartChest.createMenu`): 27
    /// cart slots plus the standard 27+9 player inventory, closing when the
    /// cart is gone.
    #[test]
    fn chest_menu_lays_out_three_rows_and_tracks_the_cart() {
        use crate::test_support::TestPlayerBuilder;

        init_vanilla_registry();
        let fixture = fresh_test_world("chest_minecart_menu_layout");
        let cart = ChestMinecartEntity::new(
            &vanilla_entities::CHEST_MINECART,
            5,
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(&fixture.world),
        );
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "MenuProbe", 6).build();

        let inventory = Arc::clone(&player.inventory);
        let container = ContainerRef::from(&cart.inventory);
        let cart: SharedEntity = Arc::new(cart);
        let kind = ChestMinecartMenuKind {
            cart: Arc::downgrade(&cart),
        };
        let menu = build_chest_menu(3, inventory, container, kind);
        assert_eq!(
            menu.behavior().slot_count(),
            CHEST_MINECART_SLOTS + 36,
            "three cart rows plus the full player inventory"
        );
    }

    /// Vanilla `ContainerEntity.unpackChestVehicleLootTable`: the deferred table
    /// is rolled once, then cleared, and the generated stacks land in the slots.
    #[test]
    fn unpacking_the_deferred_loot_table_fills_slots_and_clears_the_table() {
        init_vanilla_registry();
        let fixture = fresh_test_world("chest_minecart_loot_unpack");
        let cart = test_cart(Arc::downgrade(&fixture.world));
        cart.set_loot_table(
            Identifier::new_static("minecraft", "chests/abandoned_mineshaft"),
            42,
        );

        cart.unpack_loot_table(None);

        assert!(
            cart.loot.lock().loot_table.is_none(),
            "the loot table must be cleared after one unpack"
        );
        let stacks: usize = cart
            .inventory
            .lock()
            .items()
            .iter()
            .filter(|stack| !stack.is_empty())
            .count();
        assert!(stacks > 0, "mineshaft loot must land in the cart");

        // A second access must not roll the table again.
        let snapshot: Vec<ItemStack> = cart.inventory.lock().items().to_vec();
        cart.unpack_loot_table(None);
        assert_eq!(cart.inventory.lock().items().to_vec(), snapshot);
    }
}
