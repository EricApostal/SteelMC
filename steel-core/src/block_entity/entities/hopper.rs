//! Hopper block entity implementation.
//!
//! Mirrors Vanilla's `HopperBlockEntity`: a 5-slot container that every tick
//! pulls one item from the container above (or nearby dropped item entities)
//! and pushes one item into the container it faces, on an 8-tick cooldown.

use std::array::from_fn;
use std::mem;
use std::sync::{Arc, Weak};

use simdnbt::ToNbtTag as _;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::blocks::shapes::is_shape_full_block;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, WorldAabb,
    locks::SyncMutex,
};

use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext};
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::Entity as _;
use crate::entity::RemovalReason;
use crate::entity::container_of_entity;
use crate::entity::entities::ItemEntity;
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef, SharedContainer};
use crate::world::World;

/// Number of slots in a hopper.
pub const HOPPER_SLOTS: usize = 5;

/// Ticks a hopper waits after moving an item.
const MOVE_ITEM_SPEED: i32 = 8;

/// Cooldown value mirrored from Vanilla's `NO_COOLDOWN_TIME`.
const NO_COOLDOWN_TIME: i32 = -1;

const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING_HOPPER;
const ENABLED: &BoolProperty = &BlockStateProperties::ENABLED;

/// Independently lockable hopper inventory and transfer state.
pub struct HopperContainer {
    items: [ItemStack; HOPPER_SLOTS],
    cooldown_time: i32,
    ticked_game_time: i64,
}

// SAFETY: This key is owned by Steel and uniquely identifies `HopperBlockEntity`.
unsafe impl DowncastType for HopperBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/hopper");
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a hopper block entity.
unsafe impl DowncastType for HopperContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/hopper");
}

impl HopperContainer {
    fn new() -> Self {
        Self {
            items: from_fn(|_| ItemStack::empty()),
            cooldown_time: NO_COOLDOWN_TIME,
            ticked_game_time: 0,
        }
    }

    /// Returns whether any slot holds items. Capability-free emptiness check.
    fn is_empty(&self) -> bool {
        self.items.iter().all(ItemStack::is_empty)
    }

    /// Returns whether every slot is a full stack.
    fn inventory_full(&self) -> bool {
        self.items
            .iter()
            .all(|stack| !stack.is_empty() && stack.count() == stack.max_stack_size())
    }

    const fn is_on_cooldown(&self) -> bool {
        self.cooldown_time > 0
    }

    const fn is_on_custom_cooldown(&self) -> bool {
        self.cooldown_time > MOVE_ITEM_SPEED
    }
}

/// A hopper block entity.
///
/// Vanilla parity: `net.minecraft.world.level.block.entity.HopperBlockEntity`.
pub struct HopperBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<HopperContainer>>,
    container_ref: ContainerRef,
}

impl HopperBlockEntity {
    /// Creates a new hopper block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::HOPPER,
            level,
            pos,
            state,
        ));
        let container = Arc::new(SyncMutex::new(HopperContainer::new()));
        let shared_container: SharedContainer =
            Arc::<SyncMutex<HopperContainer>>::clone(&container);
        Self {
            container_ref: ContainerRef::owned_by_block_entity(shared_container, Arc::clone(&base)),
            base,
            container,
        }
    }

    /// Returns the direction this hopper pushes into, from its live block state.
    fn facing(&self) -> Direction {
        self.base.block_state().get_value(FACING)
    }

    /// Runs one server tick for this hopper.
    ///
    /// Vanilla parity: `HopperBlockEntity.pushItemsTick`.
    fn server_tick(&self, world: &Arc<World>) {
        let game_time = world.game_time();
        let state = self.base.block_state();

        {
            let mut container = self.container.lock();
            container.cooldown_time -= 1;
            container.ticked_game_time = game_time;
            if container.is_on_cooldown() {
                return;
            }
            container.cooldown_time = 0;
        }

        // Vanilla checks the live `ENABLED` state (redstone control).
        if !state.get_value(ENABLED) {
            return;
        }

        let mut changed = false;
        if !self.container.lock().is_empty() {
            changed = self.eject_items(world);
        }
        if !self.container.lock().inventory_full() {
            changed |= self.suck_in_items(world);
        }

        if changed {
            self.container.lock().cooldown_time = MOVE_ITEM_SPEED;
            self.base.set_changed();
        }
    }

    /// Pushes one item into the container this hopper faces.
    ///
    /// Vanilla parity: `HopperBlockEntity.ejectItems`.
    fn eject_items(&self, world: &Arc<World>) -> bool {
        let pos = self.get_block_pos();
        let facing = self.facing();
        let Some(dest_ref) = container_at(world, facing.relative(pos)) else {
            return false;
        };

        let direction = facing.opposite();
        let self_id = self.container_ref.container_id();
        let dest_id = dest_ref.container_id();
        if self_id == dest_id {
            return false;
        }

        let mut guard = ContainerLockGuard::lock_all(&[&self.container_ref, &dest_ref]);
        match guard.get(dest_id) {
            Some(dest) if !is_full_container(dest, direction) => {}
            _ => return false,
        }

        let Some([self_container, dest]) = guard.get_disjoint_mut([self_id, dest_id]) else {
            return false;
        };

        let mut moved = false;
        for slot in 0..HOPPER_SLOTS {
            let original = self_container.get_item(slot).clone();
            if original.is_empty() {
                continue;
            }

            let moving = self_container.remove_item(slot, 1);
            let remaining = add_item(
                Some(&mut *self_container),
                &mut *dest,
                moving,
                Some(direction),
            );
            if remaining.is_empty() {
                moved = true;
                break;
            }
            self_container.set_item(slot, original);
        }

        if moved {
            guard.set_changed(dest_id);
        }
        moved
    }

    /// Pulls one item either from the container above or from dropped items.
    ///
    /// Vanilla parity: `HopperBlockEntity.suckInItems`.
    fn suck_in_items(&self, world: &Arc<World>) -> bool {
        let pos = self.get_block_pos();
        let above = Direction::Up.relative(pos);
        let above_state = world.get_block_state(above);

        if let Some(source_ref) = container_at(world, above) {
            let direction = Direction::Down;
            let self_id = self.container_ref.container_id();
            let source_id = source_ref.container_id();
            if self_id == source_id {
                return false;
            }

            let mut guard = ContainerLockGuard::lock_all(&[&self.container_ref, &source_ref]);
            let Some([source, self_container]) = guard.get_disjoint_mut([source_id, self_id])
            else {
                return false;
            };

            let mut taken = false;
            for slot in container_slots(source, direction) {
                if try_take_in_item_from_slot(self_container, source, slot, direction) {
                    taken = true;
                    break;
                }
            }
            if taken {
                guard.set_changed(source_id);
            }
            return taken;
        }

        // Vanilla only sucks dropped items when the block above is not a full
        // collision block (except for the `does_not_block_hoppers` tag).
        let behavior = BLOCK_BEHAVIORS.get_behavior(above_state.get_block());
        let shape = behavior.get_collision_shape(
            above_state,
            world.as_ref(),
            above,
            BlockCollisionContext::empty(),
        );
        let blocked = is_shape_full_block(shape)
            && !above_state
                .get_block()
                .has_tag(&BlockTag::DOES_NOT_BLOCK_HOPPERS);
        if blocked {
            return false;
        }

        let aabb = suck_aabb(pos);
        let entities = world.get_entities_in_aabb_matching(&aabb, |entity| {
            !entity.is_removed() && entity.downcast_ref::<ItemEntity>().is_some()
        });

        let self_id = self.container_ref.container_id();
        let mut guard = ContainerLockGuard::lock_all(&[&self.container_ref]);
        for entity in entities {
            let Some(item_entity) = entity.downcast_ref::<ItemEntity>() else {
                continue;
            };
            let copy = item_entity.get_item();
            if copy.is_empty() {
                continue;
            }

            let Some(self_container) = guard.get_mut(self_id) else {
                break;
            };
            let remaining = add_item(None, &mut *self_container, copy, None);
            if remaining.is_empty() {
                guard.set_changed(self_id);
                item_entity.set_item(ItemStack::empty());
                item_entity.set_removed(RemovalReason::Discarded);
                return true;
            }
            item_entity.set_item(remaining);
        }

        false
    }
}

impl BlockEntity for HopperBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        let items = {
            let mut container = self.container.lock();
            mem::replace(&mut container.items, from_fn(|_| ItemStack::empty()))
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
        if let Some(items) = nbt_view.list("Items").and_then(|list| list.compounds()) {
            for compound in items {
                let Some(slot) = compound.byte("Slot").map(|slot| slot as usize) else {
                    continue;
                };
                if slot < HOPPER_SLOTS
                    && let Some(stack) = ItemStack::from_borrowed_compound(&compound)
                {
                    container.items[slot] = stack;
                }
            }
        }
        container.cooldown_time = nbt_view.int("TransferCooldown").unwrap_or(NO_COOLDOWN_TIME);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let container = self.container.lock();
        let mut items: Vec<NbtCompound> = Vec::new();
        for (slot, stack) in container.items.iter().enumerate() {
            if !stack.is_empty()
                && let NbtTag::Compound(mut item) = stack.clone().to_nbt_tag()
            {
                item.insert("Slot", slot as i8);
                items.push(item);
            }
        }
        nbt.insert("Items", NbtList::Compound(items));
        nbt.insert("TransferCooldown", container.cooldown_time);
    }

    fn tick(&self, world: &Arc<World>) {
        self.server_tick(world);
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }
}

impl Container for HopperContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        HOPPER_SLOTS
    }

    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot < HOPPER_SLOTS {
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

/// Returns the container reachable at `pos`, if any.
///
/// Vanilla parity: `HopperBlockEntity.getContainerAt`. Vanilla first tries the
/// block container (`getBlockContainer`) and only falls back to an entity
/// container (`getEntityContainer`) when the block exposes none. The entity
/// lookup mirrors `ContainerEntity`: any alive entity intersecting the block's
/// centered unit cube whose container is exposed (chest/hopper minecarts, chest
/// boats and rafts), chosen at random when several overlap.
///
/// Blocks whose `WorldlyContainerHolder` capability is not yet modeled in Steel
/// (composters) are still not covered here.
///
/// This is a reusable container-transfer helper shared with other
/// Vanilla-style dispensers; callers must hold no container locks.
pub fn container_at(world: &Arc<World>, pos: BlockPos) -> Option<ContainerRef> {
    if let Some(container) = world
        .get_block_entity(pos)
        .and_then(ContainerRef::from_block_entity)
    {
        return Some(container);
    }

    let x = f64::from(pos.x()) + 0.5;
    let y = f64::from(pos.y()) + 0.5;
    let z = f64::from(pos.z()) + 0.5;
    let aabb = WorldAabb::new(x - 0.5, y - 0.5, z - 0.5, x + 0.5, y + 0.5, z + 0.5);
    let containers: Vec<ContainerRef> = world
        .get_entities_in_aabb(&aabb)
        .into_iter()
        .filter(|entity| entity.is_alive())
        .filter_map(|entity| container_of_entity(entity.as_ref()))
        .collect();
    if containers.is_empty() {
        return None;
    }
    let index = rand::random_range(0..containers.len());
    containers.get(index).cloned()
}

/// Returns the slots of `container` reachable through `direction`.
///
/// Vanilla parity: `HopperBlockEntity.getSlots`.
fn container_slots(container: &dyn Container, direction: Direction) -> Vec<usize> {
    container.slots_for_face(direction).map_or_else(
        || (0..container.get_container_size()).collect(),
        <[usize]>::to_vec,
    )
}

/// Returns whether `container` has no room for any item through `direction`.
///
/// Vanilla parity: `HopperBlockEntity.isFullContainer`.
pub fn is_full_container(container: &dyn Container, direction: Direction) -> bool {
    container_slots(container, direction)
        .into_iter()
        .all(|slot| {
            let stack = container.get_item(slot);
            stack.count() >= stack.max_stack_size()
        })
}

/// Inserts `item_stack` into `container`, returning the leftover.
///
/// Vanilla parity: `HopperBlockEntity.addItem`. Shared with other blocks that
/// push single items through Vanilla's hopper transfer rules (droppers).
pub fn add_item(
    from: Option<&mut dyn Container>,
    container: &mut dyn Container,
    mut item_stack: ItemStack,
    direction: Option<Direction>,
) -> ItemStack {
    let slots = match direction {
        Some(direction) => container_slots(container, direction),
        None => (0..container.get_container_size()).collect(),
    };

    let mut from = from;
    for slot in slots {
        if item_stack.is_empty() {
            break;
        }
        let from_ref: Option<&mut dyn Container> = from
            .as_mut()
            .map(|container| &mut **container as &mut dyn Container);
        item_stack = try_move_in_item(from_ref, &mut *container, item_stack, slot, direction);
    }

    item_stack
}

/// Attempts to move `item_stack` into one slot, returning the leftover.
///
/// Vanilla parity: `HopperBlockEntity.tryMoveInItem`.
fn try_move_in_item(
    from: Option<&mut dyn Container>,
    container: &mut dyn Container,
    mut item_stack: ItemStack,
    slot: usize,
    direction: Option<Direction>,
) -> ItemStack {
    if !can_place_item_in_container(container, &item_stack, slot, direction) {
        return item_stack;
    }

    let current = container.get_item(slot).clone();
    let was_empty = container.is_empty();
    let mut success = false;

    if current.is_empty() {
        container.set_item(slot, item_stack);
        item_stack = ItemStack::empty();
        success = true;
    } else if can_merge_items(&current, &item_stack) {
        let space = item_stack.max_stack_size() - current.count();
        let count = item_stack.count().min(space);
        item_stack.shrink(count);
        container.get_item_mut(slot).grow(count);
        success = count > 0;
    }

    if success {
        if was_empty
            && let Some(hopper) = container.downcast_mut::<HopperContainer>()
            && !hopper.is_on_custom_cooldown()
        {
            let mut skip_tick_count = 0;
            if let Some(from_hopper) = from.and_then(|from| from.downcast_ref::<HopperContainer>())
                && hopper.ticked_game_time >= from_hopper.ticked_game_time
            {
                skip_tick_count = 1;
            }
            hopper.cooldown_time = MOVE_ITEM_SPEED - skip_tick_count;
        }
        container.set_changed();
    }

    item_stack
}

/// Returns whether `container` may accept `item_stack` at `slot`.
///
/// Vanilla parity: `HopperBlockEntity.canPlaceItemInContainer`.
fn can_place_item_in_container(
    container: &dyn Container,
    item_stack: &ItemStack,
    slot: usize,
    direction: Option<Direction>,
) -> bool {
    match direction {
        Some(direction) => container.can_place_item_through_face(slot, item_stack, direction),
        None => container.can_place_item(slot, item_stack),
    }
}

/// Returns whether `from` may give up `item_stack` at `slot` through `direction`.
///
/// Vanilla parity: `HopperBlockEntity.canTakeItemFromContainer`.
fn can_take_item_from_container(
    into: &dyn Container,
    from: &dyn Container,
    item_stack: &ItemStack,
    slot: usize,
    direction: Direction,
) -> bool {
    from.can_take_item(into, slot, item_stack)
        && from.can_take_item_through_face(slot, item_stack, direction)
}

/// Moves one item from `source` into the hopper `dest`.
///
/// Vanilla parity: `HopperBlockEntity.tryTakeInItemFromSlot`.
fn try_take_in_item_from_slot(
    dest: &mut dyn Container,
    source: &mut dyn Container,
    slot: usize,
    direction: Direction,
) -> bool {
    let item_stack = source.get_item(slot).clone();
    if item_stack.is_empty()
        || !can_take_item_from_container(dest, source, &item_stack, slot, direction)
    {
        return false;
    }

    let original = item_stack.clone();
    let moving = source.remove_item(slot, 1);
    let remaining = add_item(Some(&mut *source), &mut *dest, moving, None);
    if remaining.is_empty() {
        source.set_changed();
        return true;
    }

    source.set_item(slot, original);
    false
}

/// Returns whether `a` and `b` are the same item and can be merged.
///
/// Vanilla parity: `HopperBlockEntity.canMergeItems`.
fn can_merge_items(a: &ItemStack, b: &ItemStack) -> bool {
    a.count() <= a.max_stack_size() && ItemStack::is_same_item_same_components(a, b)
}

/// Returns the world-space volume a hopper sucks dropped items from.
///
/// Vanilla parity: `Hopper.SUCK_AABB` (`Block.column(16, 11, 32)`) moved by the
/// hopper's centered level coordinates.
fn suck_aabb(pos: BlockPos) -> WorldAabb {
    let min_x = f64::from(pos.x());
    let min_y = f64::from(pos.y()) + 11.0 / 16.0;
    let min_z = f64::from(pos.z());
    WorldAabb::new(
        min_x,
        min_y,
        min_z,
        min_x + 1.0,
        min_y + 21.0 / 16.0,
        min_z + 1.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::container::SimpleContainer;
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::{init_vanilla_registry, vanilla_items};
    use steel_utils::locks::IntoShared as _;

    fn hopper() -> HopperBlockEntity {
        init_vanilla_registry();
        HopperBlockEntity::new(
            Weak::new(),
            BlockPos::new(0, 64, 0),
            steel_registry::vanilla_blocks::HOPPER.default_state(),
        )
    }

    #[test]
    fn add_item_merges_into_existing_stack() {
        let mut container = SimpleContainer::new(1).into_shared();
        let mut dest = container.lock();
        dest.set_item(0, ItemStack::with_count(&vanilla_items::STONE, 10));
        let leftover = add_item(
            None,
            &mut *dest,
            ItemStack::with_count(&vanilla_items::STONE, 5),
            None,
        );
        assert!(leftover.is_empty());
        assert_eq!(dest.get_item(0).count(), 15);
    }

    #[test]
    fn add_item_returns_leftover_when_full() {
        let container = SimpleContainer::new(1).into_shared();
        let mut dest = container.lock();
        dest.set_item(0, ItemStack::with_count(&vanilla_items::STONE, 64));
        let leftover = add_item(
            None,
            &mut *dest,
            ItemStack::with_count(&vanilla_items::DIRT, 1),
            None,
        );
        assert_eq!(leftover.count(), 1);
        assert!(leftover.is(&vanilla_items::DIRT));
    }

    #[test]
    fn hopper_container_limits_stack_size() {
        let hopper = hopper();
        let mut container = hopper.container.lock();
        container.set_item(0, ItemStack::with_count(&vanilla_items::STONE, 100));
        assert_eq!(container.get_item(0).count(), 64);
    }

    #[test]
    fn pre_remove_preserves_slots() {
        let hopper = hopper();
        hopper
            .container
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::STONE));
        hopper.pre_remove_side_effects(
            BlockPos::new(0, 64, 0),
            steel_registry::vanilla_blocks::HOPPER.default_state(),
        );
        let container = hopper.container.lock();
        assert_eq!(container.items.len(), HOPPER_SLOTS);
        assert!(container.items.iter().all(ItemStack::is_empty));
    }

    #[test]
    fn facing_comes_from_block_state() {
        let hopper = hopper();
        let state = hopper.base.block_state().set_value(FACING, Direction::West);
        assert!(
            hopper.base.queue_block_state_change(state),
            "state change must be observable"
        );
        assert_eq!(hopper.facing(), Direction::West);
    }

    #[test]
    fn suck_aabb_matches_vanilla_column() {
        let aabb = suck_aabb(BlockPos::new(2, 64, -3));
        assert_eq!(aabb.min_corner().x, 2.0);
        assert!((aabb.min_corner().y - 64.6875).abs() < 1e-9);
        assert_eq!(aabb.max_corner().x, 3.0);
        assert!((aabb.max_corner().y - 66.0).abs() < 1e-9);
    }

    #[test]
    fn container_at_resolves_an_entity_container_and_the_hopper_pulls_from_it() {
        use glam::DVec3;
        use steel_registry::vanilla_entities;
        use steel_utils::ChunkPos;

        use crate::entity::SharedEntity;
        use crate::entity::entities::objects::ChestMinecartEntity;
        use crate::test_support::{fresh_test_world, insert_ready_full_chunk};

        init_vanilla_registry();
        let world_fixture = fresh_test_world("hopper_entity_container");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));

        let hopper = HopperBlockEntity::new(
            Arc::downgrade(world),
            pos,
            steel_registry::vanilla_blocks::HOPPER.default_state(),
        );

        let cart = Arc::new(ChestMinecartEntity::new(
            &vanilla_entities::CHEST_MINECART,
            8_003,
            DVec3::new(8.5, 65.0, 8.5),
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&cart) as SharedEntity)
            .expect("chest minecart should enter loaded chunk");

        let cart_container = cart.container_ref();
        let cart_id = cart_container.container_id();
        {
            let mut guard = ContainerLockGuard::lock_all(&[&cart_container]);
            guard.set_item(cart_id, 0, ItemStack::with_count(&vanilla_items::STONE, 3));
        }

        // Vanilla `getContainerAt` falls back to the entity container above.
        assert!(
            container_at(world, pos.above()).is_some(),
            "the chest minecart above must be resolved as a container"
        );

        BlockEntity::tick(&hopper, world);

        assert_eq!(hopper.container.lock().get_item(0).count(), 1);
        let guard = ContainerLockGuard::lock_all(&[&cart_container]);
        assert_eq!(
            guard
                .get(cart_id)
                .expect("cart container")
                .get_item(0)
                .count(),
            2
        );
    }
}
