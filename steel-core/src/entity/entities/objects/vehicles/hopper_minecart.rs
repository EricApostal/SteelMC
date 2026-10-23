//! Hopper minecart entity (`net.minecraft.world.entity.vehicle.minecart.MinecartHopper`).
//!
//! A five-slot container cart that sucks up item entities above the rail and
//! pushes its contents into the container below it. Vanilla drives this through
//! `HopperBlockEntity.suckInItems`/`tryMoveItems`; Steel has no hopper block
//! entity yet, so the item-entity suction and the container-below transfer are
//! implemented here directly against the container and item-entity APIs.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics from [`super::minecart_base`].

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, HopperMinecartEntityData};
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::Downcast as _;
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::inventory::container::{Container, SimpleContainer};
use crate::inventory::lock::ContainerRef;
use crate::world::{LevelReader as _, World};

use super::minecart_base::{self, Minecart, MinecartState};

/// Vanilla `MinecartHopper.getContainerSize`.
pub const HOPPER_MINECART_SLOTS: usize = 5;
/// Vanilla `MinecartHopper.getDefaultDisplayOffset`.
const HOPPER_DISPLAY_OFFSET: i32 = 1;
/// Vanilla hopper pickup radius (`getBoundingBox().inflate(0.25, 0.0, 0.25)`).
const PICKUP_INFLATE: f64 = 0.25;

/// A hopper minecart: five slots, picks up items, feeds the container below.
#[entity_behavior(class = "MinecartHopper")]
pub struct HopperMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<HopperMinecartEntityData>,
    state: SyncMutex<MinecartState>,
    inventory: SyncMutex<SimpleContainer>,
    enabled: SyncMutex<bool>,
    /// Vanilla `consumedItemThisFrame`, reset each tick.
    consumed_item_this_frame: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `HopperMinecartEntity`.
unsafe impl DowncastType for HopperMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/hopper_minecart");
}

impl HopperMinecartEntity {
    /// Creates a new hopper minecart entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(HopperMinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            inventory: SyncMutex::new(SimpleContainer::new(HOPPER_MINECART_SLOTS)),
            enabled: SyncMutex::new(true),
            consumed_item_this_frame: SyncMutex::new(false),
        }
    }

    /// Creates a hopper minecart entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(HopperMinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            inventory: SyncMutex::new(SimpleContainer::new(HOPPER_MINECART_SLOTS)),
            enabled: SyncMutex::new(true),
            consumed_item_this_frame: SyncMutex::new(false),
        }
    }

    /// Whether the cart currently collects and transfers items, vanilla `isEnabled`.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        *self.enabled.lock()
    }

    /// Sets whether the cart collects items, vanilla `setEnabled`.
    pub fn set_enabled(&self, enabled: bool) {
        *self.enabled.lock() = enabled;
    }

    /// Runs `f` against the five-slot inventory.
    pub fn with_inventory_mut<R>(&self, f: impl FnOnce(&mut SimpleContainer) -> R) -> R {
        f(&mut self.inventory.lock())
    }

    /// Runs `f` against the five-slot inventory immutably.
    pub fn with_inventory<R>(&self, f: impl FnOnce(&SimpleContainer) -> R) -> R {
        f(&self.inventory.lock())
    }

    /// Vanilla `MinecartHopper.suckInItems`.
    fn suck_in_items(&self, world: &Arc<World>) -> bool {
        let hitbox = self
            .bounding_box()
            .inflate_xyz(PICKUP_INFLATE, 0.0, PICKUP_INFLATE);
        let items = world.get_entities_in_aabb_matching(&hitbox, |entity| {
            entity.id() != self.id() && entity.is::<ItemEntity>() && entity.is_alive()
        });

        for item_entity in items {
            let Some(item_entity) = item_entity.downcast_ref::<ItemEntity>() else {
                continue;
            };
            let mut stack = item_entity.get_item();
            if stack.is_empty() {
                continue;
            }

            let original = stack.count();
            let added = self.with_inventory_mut(|inventory| {
                let mut to_add = stack.copy_with_count(stack.count());
                let complete = inventory.add(&mut to_add);
                (to_add.count(), complete)
            });
            let remaining = added.0;
            if remaining == original {
                continue;
            }

            if remaining == 0 {
                item_entity.set_removed(crate::entity::RemovalReason::Discarded);
            } else {
                item_entity.set_item(stack.copy_with_count(remaining));
            }
            return true;
        }

        false
    }

    /// Vanilla `HopperBlockEntity.tryMoveItems`: push one slot into the
    /// container directly below the cart.
    fn try_move_items(&self, world: &Arc<World>) -> bool {
        let target_pos = self.block_position().below();
        let Some(block_entity) = world.get_block_entity(target_pos) else {
            return false;
        };
        let Some(target) = ContainerRef::from_block_entity(block_entity) else {
            return false;
        };

        let mut moved = false;
        for slot in 0..HOPPER_MINECART_SLOTS {
            let mut stack = self.inventory.lock().get_item(slot).clone();
            if stack.is_empty() {
                continue;
            }

            let mut guard = crate::inventory::lock::ContainerLockGuard::lock_all(&[&target]);
            if let Some(target_container) = guard.get_mut(target.container_id())
                && target_container.add(&mut stack)
            {
                moved = true;
            }
            drop(guard);

            self.inventory.lock().set_item(slot, stack);
            if moved {
                break;
            }
        }

        moved
    }

    /// Vanilla `MinecartHopper.tryConsumeItems`.
    fn try_consume_items(&self) {
        if !self.is_enabled() || *self.consumed_item_this_frame.lock() {
            return;
        }

        let Some(world) = self.level() else {
            return;
        };
        if !self.is_alive() {
            return;
        }

        if self.suck_in_items(&world) {
            *self.consumed_item_this_frame.lock() = true;
        }
        let _ = self.try_move_items(&world);
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
            if slot < HOPPER_MINECART_SLOTS
                && let Some(stack) = ItemStack::from_borrowed_compound(&compound)
            {
                inventory.items_mut()[slot] = stack;
            }
        }
    }
}

impl Minecart for HopperMinecartEntity {
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
        vanilla_blocks::HOPPER.default_state()
    }

    fn default_display_offset(&self) -> i32 {
        HOPPER_DISPLAY_OFFSET
    }

    fn drop_item(&self) -> Option<ItemRef> {
        Some(&vanilla_items::HOPPER_MINECART)
    }

    fn activate_minecart(&self, _world: &Arc<World>, _pos: BlockPos, powered: bool) {
        // Vanilla `MinecartHopper.activateMinecart`: an unpowered activator rail enables.
        let new_enabled = !powered;
        if new_enabled != self.is_enabled() {
            self.set_enabled(new_enabled);
        }
    }

    fn apply_natural_slowdown(&self, movement: DVec3) -> DVec3 {
        // Vanilla `AbstractMinecartContainer.applyNaturalSlowdown`.
        let mut keep = 0.98_f64;
        let emptiness = 15
            - crate::inventory::container::calculate_redstone_signal_from_container(
                &*self.inventory.lock(),
            );
        keep += f64::from(emptiness) * 0.001;
        if self.is_in_water() {
            keep *= 0.95;
        }
        DVec3::new(movement.x * keep, 0.0, movement.z * keep)
    }

    fn on_minecart_destroyed(&self, world: &World) {
        // Vanilla `ContainerEntity.chestVehicleDestroyed` drops the cart contents only while
        // the `entity_drops` game rule is enabled; the cart item itself is gated separately in
        // `Minecart::destroy_with_drop`.
        if !world.get_game_rule(&steel_registry::vanilla_game_rules::ENTITY_DROPS) {
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

impl Entity for HopperMinecartEntity {
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
        *self.consumed_item_this_frame.lock() = false;
        Arc::clone(&self).minecart_tick();
        self.try_consume_items();
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

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart_data(nbt);
        self.save_items(nbt);
        nbt.insert("Enabled", i8::from(self.is_enabled()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
        self.load_items(nbt);
        self.set_enabled(nbt.byte("Enabled").is_none_or(|value| value != 0));
    }
}
