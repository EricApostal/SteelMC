//! Vanilla `Entity implements SlotProvider`.
//!
//! Mirrors `net.minecraft.world.entity.Entity.getSlot` and the overrides Steel
//! can address:
//!
//! * `LivingEntity.getSlot`: equipment slots 98-106 through
//!   `createEquipmentSlotAccess`, including its write gate.
//! * `Player.getSlot`: the 36 main inventory slots (0-35), the 2x2 crafting
//!   grid (500-503) and the ender chest (200-226).
//! * `AbstractVillager.getSlot` and `Piglin.getSlot`: the mob inventory (300+).
//! * `AbstractHorse.getSlot` and `AbstractChestedHorse.getSlot`: the mount
//!   inventory (500+) and the chest pseudo-slot (499).
//! * `ItemEntity.getSlot`: the dropped stack (0).
//! * `Container.getSlot`: entities that are containers (chest and hopper
//!   minecarts, chest boats and chest rafts) expose their own
//!   `0..getContainerSize()` slots.
//!
//! Every [`SlotAccess`] locks its backing storage for the duration of one
//! `get`/`set` and drops the guard before returning, so a slot handle never
//! holds a Steel mutex across another `get_slot` call. That matters because the
//! player inventory lock is not reentrant and `/item` locks several slot
//! collections at once.
//!
//! Vanilla returns `null` for an index the entity has no slot for, so the
//! indices below return `None` rather than inventing a slot. These vanilla
//! slots have no Steel accessor yet:
//!
//! * `player.cursor` (499): the carried stack lives in `Player`'s private
//!   `inventory_menu: SyncMutex<Menu>` (`MenuBehavior::carried`), and `Player`
//!   exposes no accessor for it.
//! * `AbstractVillager` 300+ on a wandering trader: its 8-slot inventory lives
//!   in the private `TraderState` inside `WanderingTraderEntity`, which exposes
//!   no inventory accessor.
//! * `AbstractHorse` 500+ and `AbstractChestedHorse` 499 on a trader llama:
//!   `TraderLlamaEntity` wraps a private `LlamaEntity` and exposes no
//!   inventory or chest accessor.
//! * `AbstractHorse` 500+ on a camel: `CamelEntity` keeps its
//!   `AbstractHorseParts` private and never rebuilds the mount storage, so no
//!   mount inventory is addressable.

use std::slice;
use std::sync::Arc;

use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use steel_utils::Downcast as _;
use steel_utils::locks::{Shared, SyncMutex};

use crate::entity::entities::{
    DonkeyEntity, HorseEntity, ItemEntity, LlamaEntity, MuleEntity, PiglinEntity,
    SkeletonHorseEntity, VillagerEntity, ZombieHorseEntity,
};
use crate::entity::npc::InventoryCarrier;
use crate::entity::{Entity, LivingEntity, container_of_entity};
use crate::inventory::container::{Container, CraftingContainer};
use crate::inventory::equipment::EquipmentSlot;
use crate::inventory::lock::{ContainerLockGuard, ContainerRef};
use crate::player::Player;

use super::{FnSlotAccess, SlotAccess, SlotProvider};

/// Vanilla `player.cursor` slot id.
const PLAYER_CURSOR_SLOT: i32 = 499;
/// Vanilla `player.crafting.*` offset (`Player.getSlot`'s `slot - 500`).
const PLAYER_CRAFTING_SLOT_OFFSET: i32 = 500;
/// Vanilla `enderchest.*` offset (`Player.getSlot`'s `slot - 200`).
const ENDER_CHEST_SLOT_OFFSET: i32 = 200;
/// Vanilla `Player.getSlot`'s `inventory.getNonEquipmentItems().size()`.
const PLAYER_INVENTORY_SLOTS: usize = 36;
/// Vanilla `mob.inventory.*` offset for `AbstractVillager` and `Piglin`.
const MOB_INVENTORY_SLOT_OFFSET: i32 = 300;
/// Vanilla `horse.*` offset for `AbstractHorse.getSlot`.
const HORSE_INVENTORY_SLOT_OFFSET: i32 = 500;
/// Vanilla `horse.chest` slot id for `AbstractChestedHorse.getSlot`.
const HORSE_CHEST_SLOT: i32 = 499;

/// Vanilla `Entity implements SlotProvider` for a Steel entity.
///
/// Wraps the entity instead of implementing [`SlotProvider`] for `dyn Entity`
/// so the command layer can hand out a provider only where vanilla would.
pub(crate) struct EntitySlots<'a> {
    entity: &'a dyn Entity,
}

impl<'a> EntitySlots<'a> {
    /// Creates the provider for `entity`.
    pub(crate) const fn new(entity: &'a dyn Entity) -> Self {
        Self { entity }
    }
}

impl SlotProvider for EntitySlots<'_> {
    fn get_slot(&self, slot: i32) -> Option<Box<dyn SlotAccess + '_>> {
        let entity = self.entity;

        // Vanilla `Player.getSlot` replaces the whole chain for players.
        if let Some(player) = entity.as_player()
            && let Some(access) = player_slot(player, slot)
        {
            return Some(access);
        }

        // Vanilla `AbstractVillager.getSlot` (villagers; see the module docs
        // for the wandering trader).
        if let Some(villager) = entity.downcast_ref::<VillagerEntity>()
            && let Some(access) = villager_slot(villager, slot)
        {
            return Some(access);
        }

        // Vanilla `Piglin.getSlot`.
        if let Some(piglin) = entity.downcast_ref::<PiglinEntity>()
            && let Some(access) = mob_inventory_slot(piglin.get_inventory(), slot)
        {
            return Some(access);
        }

        // Vanilla `AbstractHorse.getSlot` and `AbstractChestedHorse.getSlot`.
        if let Some(access) = horse_slot(entity, slot) {
            return Some(access);
        }

        // Vanilla `ItemEntity.getSlot`.
        if let Some(item_entity) = entity.downcast_ref::<ItemEntity>()
            && slot == 0
        {
            return Some(Box::new(FnSlotAccess::new(
                move || item_entity.get_item(),
                move |stack: ItemStack| {
                    item_entity.set_item(stack);
                    true
                },
            )));
        }

        // Vanilla `Container.getSlot` for entities that are containers.
        if let Some(container) = container_of_entity(entity)
            && let Some(access) = container_slot(container, slot)
        {
            return Some(access);
        }

        // Vanilla `LivingEntity.getSlot`.
        let living = entity.as_living_entity()?;
        let equipment = equipment_slot(slot)?;
        Some(equipment_access(living, equipment))
    }
}

/// Vanilla `Player.getSlot`: cursor, crafting grid, main inventory, ender chest.
///
/// Vanilla checks 499, then 500-503, then 0-35, then 200+ before falling back
/// to `Avatar.getSlot` (the equipment chain in [`SlotProvider::get_slot`]).
fn player_slot(player: &Player, slot: i32) -> Option<Box<dyn SlotAccess + '_>> {
    // `player.cursor` (499) is not addressable; see the module docs.
    if slot == PLAYER_CURSOR_SLOT {
        return None;
    }

    // Vanilla `player.crafting.*` (500-503): the 2x2 grid of the always-open
    // inventory menu.
    if let Some(index) = offset_index(slot, PLAYER_CRAFTING_SLOT_OFFSET) {
        let container = player.crafting_container();
        if index < container.lock().get_container_size() {
            return Some(crafting_slot(container, index));
        }
    }

    // Vanilla `hotbar.*` and `inventory.*`: the 36 non-equipment slots.
    if let Ok(index) = usize::try_from(slot)
        && index < PLAYER_INVENTORY_SLOTS
    {
        return Some(player_inventory_slot(player, index));
    }

    // Vanilla `enderchest.*` (200-226).
    if let Some(index) = offset_index(slot, ENDER_CHEST_SLOT_OFFSET)
        && index < player.ender_chest_inventory.lock().get_container_size()
    {
        return Some(ender_chest_slot(player, index));
    }

    None
}

/// Vanilla `SlotAccess.forContainer(player.inventory, index)` for a main slot.
fn player_inventory_slot(player: &Player, index: usize) -> Box<dyn SlotAccess + '_> {
    Box::new(FnSlotAccess::new(
        move || player.inventory.lock().get_item(index).clone(),
        move |stack: ItemStack| {
            let mut inventory = player.inventory.lock();
            Container::set_item(&mut *inventory, index, stack);
            true
        },
    ))
}

/// Vanilla `SlotAccess.forContainer(getEnderChestInventory(), index)`.
fn ender_chest_slot(player: &Player, index: usize) -> Box<dyn SlotAccess + '_> {
    Box::new(FnSlotAccess::new(
        move || player.ender_chest_inventory.lock().get_item(index).clone(),
        move |stack: ItemStack| {
            let mut ender_chest = player.ender_chest_inventory.lock();
            ender_chest.set_item(index, stack);
            true
        },
    ))
}

/// Vanilla `SlotAccess.forContainer(inventoryMenu.getCraftSlots(), index)`.
fn crafting_slot(
    container: Shared<CraftingContainer>,
    index: usize,
) -> Box<dyn SlotAccess + 'static> {
    let get_container = Arc::clone(&container);
    let set_container = Arc::clone(&container);
    Box::new(FnSlotAccess::new(
        move || get_container.lock().get_item(index).clone(),
        move |stack: ItemStack| {
            let mut container = set_container.lock();
            container.set_item(index, stack);
            true
        },
    ))
}

/// Vanilla `AbstractVillager.getSlot`: 300+ indexes the villager's inventory.
fn villager_slot(villager: &VillagerEntity, slot: i32) -> Option<Box<dyn SlotAccess + '_>> {
    let index = offset_index(slot, MOB_INVENTORY_SLOT_OFFSET)?;
    if index >= villager.with_inventory_mut(|inventory| inventory.size()) {
        return None;
    }
    Some(Box::new(FnSlotAccess::new(
        move || villager.with_inventory_mut(|inventory| inventory.get_item(index).clone()),
        move |stack: ItemStack| {
            villager.with_inventory_mut(|inventory| inventory.set_item(index, stack));
            true
        },
    )))
}

/// Vanilla `Piglin.getSlot`: 300+ indexes the piglin's inventory.
fn mob_inventory_slot(
    inventory: &SyncMutex<InventoryCarrier>,
    slot: i32,
) -> Option<Box<dyn SlotAccess + '_>> {
    let index = offset_index(slot, MOB_INVENTORY_SLOT_OFFSET)?;
    if index >= inventory.lock().size() {
        return None;
    }
    Some(Box::new(FnSlotAccess::new(
        move || inventory.lock().get_item(index).clone(),
        move |stack: ItemStack| {
            let mut inventory = inventory.lock();
            inventory.set_item(index, stack);
            true
        },
    )))
}

/// Vanilla `AbstractHorse.getSlot` and `AbstractChestedHorse.getSlot`.
///
/// Steel models `AbstractHorse` with a shared [`InventoryCarrier`], so the
/// dispatch is per concrete equine type. Only the chested horses have the
/// `horse.chest` pseudo-slot; a non-chested horse has no slot at 499 either.
fn horse_slot(entity: &dyn Entity, slot: i32) -> Option<Box<dyn SlotAccess + '_>> {
    if let Some(horse) = entity.downcast_ref::<HorseEntity>() {
        return horse_inventory_slot(horse.carried_items(), slot);
    }
    if let Some(horse) = entity.downcast_ref::<SkeletonHorseEntity>() {
        return horse_inventory_slot(horse.carried_items(), slot);
    }
    if let Some(horse) = entity.downcast_ref::<ZombieHorseEntity>() {
        return horse_inventory_slot(horse.carried_items(), slot);
    }
    if let Some(llama) = entity.downcast_ref::<LlamaEntity>() {
        if slot == HORSE_CHEST_SLOT {
            return Some(chest_slot(llama));
        }
        return horse_inventory_slot(llama.carried_items(), slot);
    }
    if let Some(donkey) = entity.downcast_ref::<DonkeyEntity>() {
        if slot == HORSE_CHEST_SLOT {
            return Some(chest_slot(donkey));
        }
        return horse_inventory_slot(donkey.carried_items(), slot);
    }
    if let Some(mule) = entity.downcast_ref::<MuleEntity>() {
        if slot == HORSE_CHEST_SLOT {
            return Some(chest_slot(mule));
        }
        return horse_inventory_slot(mule.carried_items(), slot);
    }
    None
}

/// Vanilla `AbstractHorse.getSlot`'s `500 + index` mount-inventory slot.
fn horse_inventory_slot(
    inventory: &Shared<InventoryCarrier>,
    slot: i32,
) -> Option<Box<dyn SlotAccess + '_>> {
    let index = offset_index(slot, HORSE_INVENTORY_SLOT_OFFSET)?;
    if index >= inventory.lock().size() {
        return None;
    }
    let get_inventory = Arc::clone(inventory);
    let set_inventory = Arc::clone(inventory);
    Some(Box::new(FnSlotAccess::new(
        move || get_inventory.lock().get_item(index).clone(),
        move |stack: ItemStack| {
            let mut inventory = set_inventory.lock();
            inventory.set_item(index, stack);
            true
        },
    )))
}

/// Vanilla `AbstractChestedHorse.hasChest`/`setChest`, which the three chested
/// Steel horses expose as inherent methods.
trait ChestedHorse {
    fn has_chest(&self) -> bool;
    fn set_chest(&self, has_chest: bool);
}

impl ChestedHorse for DonkeyEntity {
    fn has_chest(&self) -> bool {
        DonkeyEntity::has_chest(self)
    }

    fn set_chest(&self, has_chest: bool) {
        DonkeyEntity::set_chest(self, has_chest);
    }
}

impl ChestedHorse for MuleEntity {
    fn has_chest(&self) -> bool {
        MuleEntity::has_chest(self)
    }

    fn set_chest(&self, has_chest: bool) {
        MuleEntity::set_chest(self, has_chest);
    }
}

impl ChestedHorse for LlamaEntity {
    fn has_chest(&self) -> bool {
        LlamaEntity::has_chest(self)
    }

    fn set_chest(&self, has_chest: bool) {
        LlamaEntity::set_chest(self, has_chest);
    }
}

/// Vanilla `AbstractChestedHorse.getSlot(499)` (`horse.chest`): a pseudo-slot
/// that reports the chest item while the horse carries a chest and toggles the
/// chest on write.
fn chest_slot(horse: &dyn ChestedHorse) -> Box<dyn SlotAccess + '_> {
    Box::new(FnSlotAccess::new(
        move || {
            if horse.has_chest() {
                ItemStack::new(&vanilla_items::CHEST)
            } else {
                ItemStack::empty()
            }
        },
        move |stack: ItemStack| {
            if stack.is_empty() {
                if !horse.has_chest() {
                    return false;
                }
                horse.set_chest(false);
                true
            } else if stack.is(&vanilla_items::CHEST) {
                if horse.has_chest() {
                    return false;
                }
                horse.set_chest(true);
                true
            } else {
                false
            }
        },
    ))
}

/// Vanilla `Container.getSlot` for an entity that is a container.
///
/// `ContainerRef::lock` is private, so every operation re-locks through
/// [`ContainerLockGuard::lock_all`]; no guard outlives a single `get`/`set`.
/// Writes go through [`ContainerLockGuard::set_item`], which mirrors the
/// container's own `setItem` notification boundary.
fn container_slot(container: ContainerRef, slot: i32) -> Option<Box<dyn SlotAccess + 'static>> {
    let index = usize::try_from(slot).ok()?;
    let size = {
        let guard = ContainerLockGuard::lock_all(slice::from_ref(&container));
        guard.get(container.container_id())?.get_container_size()
    };
    if index >= size {
        return None;
    }

    let get_container = container.clone();
    let set_container = container;
    Some(Box::new(FnSlotAccess::new(
        move || {
            let guard = ContainerLockGuard::lock_all(slice::from_ref(&get_container));
            guard
                .get(get_container.container_id())
                .map_or_else(ItemStack::empty, |container| {
                    container.get_item(index).clone()
                })
        },
        move |stack: ItemStack| {
            let mut guard = ContainerLockGuard::lock_all(slice::from_ref(&set_container));
            guard.set_item(set_container.container_id(), index, stack)
        },
    )))
}

/// Vanilla `LivingEntity.getEquipmentSlot(int)`.
const fn equipment_slot(slot: i32) -> Option<EquipmentSlot> {
    match slot {
        98 => Some(EquipmentSlot::MainHand),
        99 => Some(EquipmentSlot::OffHand),
        100 => Some(EquipmentSlot::Feet),
        101 => Some(EquipmentSlot::Legs),
        102 => Some(EquipmentSlot::Chest),
        103 => Some(EquipmentSlot::Head),
        105 => Some(EquipmentSlot::Body),
        106 => Some(EquipmentSlot::Saddle),
        _ => None,
    }
}

/// Vanilla `createEquipmentSlotAccess`: one live equipment-rack slot with its
/// write gate.
fn equipment_access(living: &dyn LivingEntity, slot: EquipmentSlot) -> Box<dyn SlotAccess + '_> {
    Box::new(FnSlotAccess::new(
        move || {
            let mut item = ItemStack::empty();
            living.with_equipment_slot(slot, &mut |stack| item = stack.clone());
            item
        },
        move |stack: ItemStack| {
            if !equipment_accepts(slot, &stack) {
                return false;
            }
            living.with_equipment_slot_mut(slot, &mut |current| *current = stack.clone());
            true
        },
    ))
}

/// Vanilla `createEquipmentSlotAccess`'s predicate: main hand, off hand and
/// head accept any stack; every other slot only accepts an empty stack or one
/// equippable in that slot.
fn equipment_accepts(slot: EquipmentSlot, item: &ItemStack) -> bool {
    match slot {
        EquipmentSlot::Head | EquipmentSlot::MainHand | EquipmentSlot::OffHand => true,
        _ => item.is_empty() || item.is_equippable_in_slot(slot),
    }
}

/// `slot - offset` when the result is a valid container index.
fn offset_index(slot: i32, offset: i32) -> Option<usize> {
    usize::try_from(slot.checked_sub(offset)?).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{
        init_vanilla_registry, item_stack::ItemStack, vanilla_entities, vanilla_items,
    };

    use crate::entity::Entity;
    use crate::entity::entities::{CowEntity, DonkeyEntity, HorseEntity, VillagerEntity};
    use crate::inventory::container::Container;
    use crate::inventory::equipment::EquipmentSlot;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    use super::super::{SlotProvider, range::name_to_range};
    use super::EntitySlots;

    fn cow() -> CowEntity {
        init_vanilla_registry();
        CowEntity::new(&vanilla_entities::COW, 1, DVec3::ZERO, Weak::new())
    }

    #[test]
    fn equipment_ranges_index_living_entity_equipment() {
        let cow = cow();
        let slots = EntitySlots::new(&cow);

        let mainhand = name_to_range("weapon.mainhand").expect("vanilla weapon.mainhand range");
        let chest = name_to_range("armor.chest").expect("vanilla armor.chest range");
        assert_eq!(mainhand.slots(), [98]);
        assert_eq!(chest.slots(), [102]);

        let sword = ItemStack::new(&vanilla_items::IRON_SWORD);
        let chestplate = ItemStack::new(&vanilla_items::DIAMOND_CHESTPLATE);
        let mainhand_access = slots.get_slot(98).expect("cow main hand slot");
        let chest_access = slots.get_slot(102).expect("cow chest slot");
        assert!(mainhand_access.set(sword));
        assert!(chest_access.set(chestplate));

        let living = cow.as_living_entity().expect("cow is a living entity");
        let mut main = ItemStack::empty();
        living.with_equipment_slot(EquipmentSlot::MainHand, &mut |item| main = item.clone());
        assert!(main.is(&vanilla_items::IRON_SWORD));
        let mut torso = ItemStack::empty();
        living.with_equipment_slot(EquipmentSlot::Chest, &mut |item| torso = item.clone());
        assert!(torso.is(&vanilla_items::DIAMOND_CHESTPLATE));

        let copies = slots.get_slots_from_range(mainhand.slots()).item_copies();
        assert_eq!(copies.len(), 1);
        assert!(copies[0].is(&vanilla_items::IRON_SWORD));
    }

    #[test]
    fn player_hotbar_slot_writes_the_inventory() {
        let fixture = fresh_test_world("entity_slot_provider_hotbar");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "SlotTester", 7).build();
        let slots = EntitySlots::new(player.as_ref());

        let hotbar = name_to_range("hotbar.0").expect("vanilla hotbar.0 range");
        assert_eq!(hotbar.slots(), [0]);

        let access = slots.get_slot(0).expect("player hotbar slot");
        assert!(access.set(ItemStack::new(&vanilla_items::STONE)));
        assert!(
            player
                .inventory
                .lock()
                .get_item(0)
                .is(&vanilla_items::STONE)
        );
        assert!(access.get().is(&vanilla_items::STONE));

        let copies = slots.get_slots_from_range(hotbar.slots()).item_copies();
        assert_eq!(copies.len(), 1);
        assert!(copies[0].is(&vanilla_items::STONE));
    }

    #[test]
    fn unsupported_entity_indices_have_no_slots() {
        let cow = cow();
        let slots = EntitySlots::new(&cow);
        assert!(slots.get_slot(1).is_none());
        assert_eq!(slots.get_slots_from_range(&[1]).size(), 0);
        assert_eq!(
            slots.get_slots_from_range(&[1]).item_copies(),
            [] as [ItemStack; 0]
        );

        let fixture = fresh_test_world("entity_slot_provider_unsupported");
        let player = TestPlayerBuilder::new(Arc::clone(&fixture.world), "SlotTester", 9).build();
        let slots = EntitySlots::new(player.as_ref());

        let mob_inventory = name_to_range("mob.inventory.0").expect("vanilla mob.inventory.0");
        assert_eq!(mob_inventory.slots(), [300]);
        assert!(slots.get_slot(300).is_none());
        assert_eq!(
            slots
                .get_slots_from_range(mob_inventory.slots())
                .item_copies(),
            [] as [ItemStack; 0]
        );

        // `player.cursor` has no Steel accessor today; vanilla's answer for a
        // slot the entity cannot address is a null access, i.e. `None` here.
        assert!(slots.get_slot(499).is_none());
    }

    #[test]
    fn equipment_slot_acceptance_matches_vanilla() {
        let cow = cow();
        let slots = EntitySlots::new(&cow);

        let head = slots.get_slot(103).expect("head slot");
        assert!(head.set(ItemStack::new(&vanilla_items::STONE)));
        assert!(head.get().is(&vanilla_items::STONE));

        let chest = slots.get_slot(102).expect("chest slot");
        assert!(!chest.set(ItemStack::new(&vanilla_items::STONE)));
        assert!(chest.get().is_empty());

        assert!(chest.set(ItemStack::new(&vanilla_items::DIAMOND_CHESTPLATE)));
        assert!(chest.get().is(&vanilla_items::DIAMOND_CHESTPLATE));

        assert!(chest.set(ItemStack::empty()));
        assert!(chest.get().is_empty());
    }

    #[test]
    fn villager_inventory_range_indexes_the_mob_inventory() {
        init_vanilla_registry();
        let villager =
            VillagerEntity::new(&vanilla_entities::VILLAGER, 1, DVec3::ZERO, Weak::new());
        let slots = EntitySlots::new(&villager);

        let mob_inventory = name_to_range("mob.inventory.0").expect("vanilla mob.inventory.0");
        assert_eq!(mob_inventory.slots(), [300]);

        let access = slots.get_slot(300).expect("villager inventory slot");
        assert!(access.set(ItemStack::new(&vanilla_items::STONE)));
        assert!(access.get().is(&vanilla_items::STONE));
        let stored = villager.with_inventory_mut(|inventory| inventory.get_item(0).clone());
        assert!(stored.is(&vanilla_items::STONE));

        assert_eq!(slots.get_slots_from_range(mob_inventory.slots()).size(), 1);
        assert!(slots.get_slot(308).is_none());
    }

    #[test]
    fn chested_horse_chest_slot_reports_and_toggles_the_chest() {
        init_vanilla_registry();
        let donkey = DonkeyEntity::new(&vanilla_entities::DONKEY, 1, DVec3::ZERO, Weak::new());
        let slots = EntitySlots::new(&donkey);

        let horse_chest = name_to_range("horse.chest").expect("vanilla horse.chest range");
        assert_eq!(horse_chest.slots(), [499]);

        let chest = slots.get_slot(499).expect("donkey chest slot");
        assert!(chest.get().is_empty());
        assert!(chest.set(ItemStack::new(&vanilla_items::CHEST)));
        assert!(donkey.has_chest());
        assert!(chest.get().is(&vanilla_items::CHEST));

        assert!(!chest.set(ItemStack::new(&vanilla_items::CHEST)));
        assert!(donkey.has_chest());

        assert!(!chest.set(ItemStack::new(&vanilla_items::STONE)));
        assert!(donkey.has_chest());

        assert!(chest.set(ItemStack::empty()));
        assert!(!donkey.has_chest());
        assert!(!chest.set(ItemStack::empty()));
    }

    #[test]
    fn horse_inventory_range_is_empty_without_storage() {
        init_vanilla_registry();
        let horse = HorseEntity::new(&vanilla_entities::HORSE, 1, DVec3::ZERO, Weak::new());
        let slots = EntitySlots::new(&horse);

        let horse_range = name_to_range("horse.0").expect("vanilla horse.0 range");
        assert_eq!(horse_range.slots(), [500]);

        // Vanilla `AbstractHorse.getSlot` bounds the offset by the inventory
        // size, and Steel's unchested horse inventory is empty.
        assert!(slots.get_slot(500).is_none());
        assert_eq!(
            slots
                .get_slots_from_range(horse_range.slots())
                .item_copies(),
            [] as [ItemStack; 0]
        );
        assert!(slots.get_slot(499).is_none());
    }
}
