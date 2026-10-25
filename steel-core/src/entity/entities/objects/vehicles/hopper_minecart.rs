//! Hopper minecart entity (`net.minecraft.world.entity.vehicle.minecart.MinecartHopper`).
//!
//! A five-slot container cart that sucks up the container above it and dropped item
//! entities via vanilla's `HopperBlockEntity.suckInItems`/`tryTakeInItemFromSlot`
//! transfer rules (the dropped-item scan uses vanilla's exact `Hopper.SUCK_AABB`).
//! Unlike a hopper block, a hopper minecart never pushes items downward; items leave
//! only through a hopper below it or when the cart is destroyed.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics from [`super::minecart_base`].
//!
//! Missing dependency: pulling from an *entity* container above the cart (another
//! hopper or chest minecart) needs a container-entity lookup that Steel's shared
//! hopper helper does not provide; block containers above are handled (see
//! [`crate::block_entity::entities::hopper`]).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::blocks::properties::Direction;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, HopperMinecartEntityData};
use steel_registry::vanilla_game_rules::ENTITY_DROPS;
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::Downcast as _;
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::{IntoShared as _, Shared, SyncMutex};
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, WorldAabb};

use crate::block_entity::entities::{hopper_add_item, hopper_container_at};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData, RemovalReason};
use crate::inventory::container::{
    Container, SimpleContainer, calculate_redstone_signal_from_container,
};
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::world::World;

use super::minecart_base::{self, Minecart, MinecartState};

/// Vanilla `MinecartHopper.getContainerSize`.
pub const HOPPER_MINECART_SLOTS: usize = 5;
/// Vanilla `MinecartHopper.getDefaultDisplayOffset`.
const HOPPER_DISPLAY_OFFSET: i32 = 1;
/// Vanilla `Hopper.SUCK_AABB` (`Block.column(16, 11, 32)`) lower bound.
const SUCK_AABB_MIN_Y: f64 = 11.0 / 16.0;
/// Vanilla `Hopper.SUCK_AABB` (`Block.column(16, 11, 32)`) upper bound.
const SUCK_AABB_MAX_Y: f64 = 2.0;

/// The world-space volume a hopper minecart collects dropped items from.
///
/// Vanilla parity: `Hopper.SUCK_AABB` (`Block.column(16, 11, 32)`) moved by
/// `getLevelX() - 0.5`, `getLevelY() - 0.5`, `getLevelZ() - 0.5`. `MinecartHopper` reports the
/// cart's own position, with `getLevelY()` = `getY() + 0.5`.
fn suck_aabb(position: DVec3) -> WorldAabb {
    WorldAabb::new(
        position.x - 0.5,
        position.y + SUCK_AABB_MIN_Y,
        position.z - 0.5,
        position.x + 0.5,
        position.y + SUCK_AABB_MAX_Y,
        position.z + 0.5,
    )
}

/// A hopper minecart: five slots, sucks items from above, never pushes downward.
#[entity_behavior(class = "MinecartHopper")]
pub struct HopperMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<HopperMinecartEntityData>,
    state: SyncMutex<MinecartState>,
    inventory: Shared<SimpleContainer>,
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
            inventory: SimpleContainer::new(HOPPER_MINECART_SLOTS).into_shared(),
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
            inventory: SimpleContainer::new(HOPPER_MINECART_SLOTS).into_shared(),
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

    /// Returns the cart's container capability.
    ///
    /// Vanilla `AbstractMinecartContainer` implements `ContainerEntity`, so the
    /// cart is reachable through `HopperBlockEntity.getContainerAt` and
    /// `DetectorRailBlock.getAnalogOutputSignal`.
    #[must_use]
    pub fn container_ref(&self) -> ContainerRef {
        ContainerRef::from(&self.inventory)
    }

    /// Vanilla `MinecartHopper.suckInItems`.
    ///
    /// Vanilla first asks `HopperBlockEntity.suckInItems`, which pulls one item from the
    /// container at `BlockPos.containing(getX(), getLevelY() + 1.0, getZ())` through
    /// `Direction.DOWN`; that container, when present, is the only source considered. With no
    /// container there, dropped item entities are collected from `Hopper.SUCK_AABB` moved to
    /// the cart's level coordinates. The `isGridAligned` collision-shape block test cannot
    /// apply to a cart, whose `isGridAligned` is `false`.
    ///
    /// Vanilla also resolves an *entity* container at those coordinates (another hopper or
    /// chest minecart above); Steel's container lookup models block containers only, exactly
    /// like [`crate::block_entity::entities::hopper`].
    fn suck_in_items(&self, world: &Arc<World>) -> bool {
        let position = self.position();
        let above_pos = BlockPos::containing(position.x, position.y + 1.5, position.z);
        if let Some(source) = hopper_container_at(world, above_pos) {
            return self.pull_from_container(&source, Direction::Down);
        }

        let hitbox = suck_aabb(position);
        let items = world.get_entities_in_aabb_matching(&hitbox, |entity| {
            entity.id() != self.id() && entity.is::<ItemEntity>() && entity.is_alive()
        });

        for item_entity in items {
            let Some(item_entity) = item_entity.downcast_ref::<ItemEntity>() else {
                continue;
            };
            let stack = item_entity.get_item();
            if stack.is_empty() {
                continue;
            }

            let original = stack.count();
            let remaining = self.with_inventory_mut(|inventory| {
                // Vanilla `HopperBlockEntity.addItem(container, entity)`: the whole stack is
                // offered in one go, and only the leftover is written back.
                let copy = stack.copy_with_count(stack.count());
                hopper_add_item(None, inventory, copy, None).count()
            });
            if remaining == original {
                continue;
            }

            if remaining == 0 {
                item_entity.set_removed(RemovalReason::Discarded);
            } else {
                item_entity.set_item(stack.copy_with_count(remaining));
            }
            return true;
        }

        false
    }

    /// Vanilla `HopperBlockEntity.tryTakeInItemFromSlot` against the cart's own inventory.
    ///
    /// Returns `true` when one item moved from `source`.
    fn pull_from_container(&self, source: &ContainerRef, direction: Direction) -> bool {
        let source_id = source.container_id();
        let mut guard = ContainerLockGuard::lock_all(&[source]);
        let mut moved = false;

        {
            let Some(source_container) = guard.get_mut(source_id) else {
                return false;
            };

            let slots = source_container.slots_for_face(direction).map_or_else(
                || (0..source_container.get_container_size()).collect(),
                <[usize]>::to_vec,
            );
            for slot in slots {
                let stack = source_container.get_item(slot).clone();
                if stack.is_empty() {
                    continue;
                }
                // Vanilla `HopperBlockEntity.canTakeItemFromContainer`. The cart's own inventory
                // is locked after the source, matching this module's lock order.
                let takeable = {
                    let inventory = self.inventory.lock();
                    source_container.can_take_item(&*inventory, slot, &stack)
                        && source_container.can_take_item_through_face(slot, &stack, direction)
                };
                if !takeable {
                    continue;
                }

                let original = stack.clone();
                let moving = source_container.remove_item(slot, 1);
                let remaining = {
                    let mut inventory = self.inventory.lock();
                    hopper_add_item(Some(&mut *source_container), &mut *inventory, moving, None)
                };
                if remaining.is_empty() {
                    moved = true;
                    break;
                }

                source_container.set_item(slot, original);
            }
        }

        if moved {
            guard.set_changed(source_id);
        }
        moved
    }

    /// Vanilla `MinecartHopper.tryConsumeItems`.
    ///
    /// Vanilla's `consumedItemThisFrame` latch is cleared on tick and set when suction moved
    /// an item, so at most one item is pulled per tick.
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
        let emptiness = 15 - calculate_redstone_signal_from_container(&*self.inventory.lock());
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
        if !world.get_game_rule(&ENTITY_DROPS) {
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

    fn push_entity(&self, entity: &dyn Entity) {
        // Vanilla `AbstractMinecart.push`.
        self.minecart_push(entity);
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

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_registry::{
        init_vanilla_registry, item_stack::ItemStack, vanilla_blocks, vanilla_entities,
        vanilla_items,
    };
    use steel_utils::BlockPos;

    use crate::block_entity::BlockEntity;
    use crate::block_entity::entities::HopperBlockEntity;
    use crate::inventory::lock::ContainerLockGuard;

    use super::*;

    fn hopper_cart() -> HopperMinecartEntity {
        HopperMinecartEntity::new(
            &vanilla_entities::HOPPER_MINECART,
            1,
            DVec3::new(8.5, 64.0, 8.5),
            Weak::new(),
        )
    }

    /// Builds the "container above" as a real hopper block entity, exposing the same
    /// `ContainerRef` lookup vanilla's `suckInItems` resolves through.
    fn container_above() -> (HopperBlockEntity, ContainerRef) {
        let source = HopperBlockEntity::new(
            Weak::new(),
            BlockPos::new(8, 71, 8),
            vanilla_blocks::HOPPER.default_state(),
        );
        let source_ref = BlockEntity::container_ref(&source)
            .expect("a hopper block entity exposes its container");
        (source, source_ref)
    }

    #[test]
    fn suck_aabb_matches_the_vanilla_hopper_column() {
        init_vanilla_registry();
        // Vanilla `Hopper.SUCK_AABB` (`Block.column(16, 11, 32)`) moved by
        // `getLevelX() - 0.5`, `getLevelY() - 0.5`, `getLevelZ() - 0.5` with the cart's own
        // position: y + 0.6875 to y + 2.0, half a block wide in x and z.
        let aabb = suck_aabb(DVec3::new(8.5, 64.0, 8.5));
        assert!((aabb.min_corner().x - 8.0).abs() < 1e-9);
        assert!((aabb.min_corner().y - 64.6875).abs() < 1e-9);
        assert!((aabb.min_corner().z - 8.0).abs() < 1e-9);
        assert!((aabb.max_corner().x - 9.0).abs() < 1e-9);
        assert!((aabb.max_corner().y - 66.0).abs() < 1e-9);
        assert!((aabb.max_corner().z - 9.0).abs() < 1e-9);
    }

    #[test]
    fn pull_from_container_moves_exactly_one_item_like_vanilla() {
        init_vanilla_registry();
        let cart = hopper_cart();
        let (_source, source_ref) = container_above();
        let source_id = source_ref.container_id();
        {
            let mut guard = ContainerLockGuard::lock_all(&[&source_ref]);
            assert!(guard.set_item(
                source_id,
                0,
                ItemStack::with_count(&vanilla_items::STONE, 3)
            ));
        }

        assert!(cart.pull_from_container(&source_ref, Direction::Down));
        assert_eq!(
            cart.with_inventory(|inventory| inventory.get_item(0).count()),
            1
        );

        let guard = ContainerLockGuard::lock_all(&[&source_ref]);
        assert_eq!(
            guard
                .get(source_id)
                .expect("source container")
                .get_item(0)
                .count(),
            2,
            "vanilla takes one item and restores the rest of the stack"
        );
    }

    #[test]
    fn pull_from_container_reports_false_for_an_empty_source() {
        init_vanilla_registry();
        let cart = hopper_cart();
        let (_source, source_ref) = container_above();

        assert!(!cart.pull_from_container(&source_ref, Direction::Down));
        assert!(cart.with_inventory(|inventory| inventory.is_empty()));
    }
}
