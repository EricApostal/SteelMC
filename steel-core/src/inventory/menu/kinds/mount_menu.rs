//! Mount inventory menu.
//!
//! Mirrors vanilla `HorseInventoryMenu` / `AbstractMountInventoryMenu`, the
//! screen opened by `ClientboundMountScreenOpenPacket`. Vanilla 26.3 has no
//! `horse` menu type: the client builds the same `HorseInventoryMenu` from the
//! packet's entity id and column count, so this menu declares no menu type and
//! the open path sends `CMountScreenOpen` instead of `COpenScreen`.
//!
//! # Slot layout
//!
//! | menu slot | vanilla source |
//! | --- | --- |
//! | 0 | `EquipmentSlot.Saddle` (`ArmorSlot`, saddle icon) |
//! | 1 | `EquipmentSlot.Body` (horse armor / llama armor icon) |
//! | `2..2 + columns * 3` | `AbstractHorse.inventory` (chest storage) |
//! | then | 27 player main slots, then 9 hotbar slots |
//!
//! The layout is fixed at `2 + getInventorySize(columns) + 36` slots, matching
//! the client's `HorseInventoryMenu` for every mount, including the camel
//! (whose armor slot stays inactive client-side).
//!
//! # Deliberate difference
//!
//! Vanilla backs the two equipment slots with
//! `AbstractHorse.createEquipmentSlotContainer`, a live `Container` view of the
//! entity's equipment. Steel's entity equipment is not a `Container` (items are
//! reached through a lock), so the menu edits two one-slot mirrors seeded from
//! the equipment, and publishes them back to the entity after every click and
//! on close.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::{Entity, LivingEntity as _, Mob as _, SharedEntity, WeakEntity};
use crate::inventory::prelude::*;
use crate::inventory::slots::SlotStorage;
use crate::player::player_inventory::{PlayerInventory, Prediction};

/// Vanilla `AbstractMountInventoryMenu.SLOT_SADDLE`.
pub const SLOT_SADDLE: usize = 0;
/// Vanilla `AbstractMountInventoryMenu.SLOT_BODY_ARMOR`.
pub const SLOT_BODY_ARMOR: usize = 1;
/// Vanilla `AbstractMountInventoryMenu.SLOT_INVENTORY_START`.
pub const SLOT_INVENTORY_START: usize = 2;
/// Vanilla `AbstractMountInventoryMenu.INVENTORY_ROWS`.
pub const INVENTORY_ROWS: usize = 3;

/// Vanilla `AbstractMountInventoryMenu.getInventorySize`: columns × 3 rows.
#[must_use]
pub fn get_inventory_size(inventory_columns: i32) -> usize {
    usize::try_from(inventory_columns.max(0)).unwrap_or(0) * INVENTORY_ROWS
}

/// Builds the mount inventory menu for one mount.
///
/// `inventory_generation` is the mount's inventory-rebuild token; a rebuild
/// (chest equipped or removed) closes the menu, mirroring vanilla
/// `AbstractHorse.hasInventoryChanged`.
#[must_use]
pub fn mount_inventory_menu(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    mount: SharedEntity,
    mount_inventory: ContainerRef,
    inventory_columns: i32,
    inventory_generation: Arc<AtomicU32>,
) -> Menu {
    let chest_slots = get_inventory_size(inventory_columns);
    let opened_generation = inventory_generation.load(Ordering::Relaxed);
    let saddle = equipment_mirror(&mount, EquipmentSlot::Saddle);
    let armor = equipment_mirror(&mount, EquipmentSlot::Body);

    let mut builder = MenuBuilder::new(None, container_id);

    let saddle_factory = {
        let mount = Arc::clone(&mount);
        move |container: &ContainerRef, index: usize| -> Box<dyn Slot> {
            Box::new(MountEquipmentSlot::new(
                container.clone(),
                index,
                Arc::clone(&mount),
                EquipmentSlot::Saddle,
            ))
        }
    };
    builder.section_with(&saddle, 1, SectionKind::custom(saddle_factory));

    let armor_factory = {
        let mount = Arc::clone(&mount);
        move |container: &ContainerRef, index: usize| -> Box<dyn Slot> {
            Box::new(MountEquipmentSlot::new(
                container.clone(),
                index,
                Arc::clone(&mount),
                EquipmentSlot::Body,
            ))
        }
    };
    builder.section_with(&armor, 1, SectionKind::custom(armor_factory));

    if chest_slots > 0 {
        builder.section(&mount_inventory, chest_slots);
    }

    builder.player_inventory(&inventory);

    builder.build(MountInventoryMenuKind {
        mount: Arc::downgrade(&mount),
        saddle,
        armor,
        chest_slots,
        inventory_generation,
        opened_generation,
    })
}

/// Seeds a one-slot mirror from the mount's equipment slot.
fn equipment_mirror(mount: &SharedEntity, slot: EquipmentSlot) -> Shared<SimpleContainer> {
    let mirror = SimpleContainer::new(1).into_shared();
    let mut equipped = ItemStack::empty();
    if let Some(living) = mount.as_living_entity() {
        living.with_equipment_slot(slot, &mut |stack| equipped = stack.clone());
    }
    *mirror.lock().get_item_mut(0) = equipped;
    mirror
}

/// A mount equipment slot: a `NormalSlot` restricted to the mount's own
/// equippable rules, mirroring vanilla `ArmorSlot`.
pub struct MountEquipmentSlot {
    base: NormalSlot,
    mount: SharedEntity,
    slot: EquipmentSlot,
}

// SAFETY: This Steel-owned key uniquely identifies `MountEquipmentSlot`.
unsafe impl DowncastType for MountEquipmentSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:slot/mount_equipment");
}

impl MountEquipmentSlot {
    /// Creates a mount equipment slot over one menu-local mirror slot.
    #[must_use]
    pub fn new(
        container: ContainerRef,
        index: usize,
        mount: SharedEntity,
        slot: EquipmentSlot,
    ) -> Self {
        Self {
            base: NormalSlot::new(container, index),
            mount,
            slot,
        }
    }

    /// The equipment slot this menu slot mirrors.
    #[must_use]
    pub const fn equipment_slot(&self) -> EquipmentSlot {
        self.slot
    }
}

impl Slot for MountEquipmentSlot {
    fn storage(&self) -> &SlotStorage {
        self.base.storage()
    }

    fn get_item<'a>(&self, guard: &'a ContainerLockGuard) -> &'a ItemStack {
        self.base.get_item(guard)
    }

    fn get_item_mut<'a>(&self, guard: &'a mut ContainerLockGuard) -> &'a mut ItemStack {
        self.base.get_item_mut(guard)
    }

    fn set_item(&self, guard: &mut ContainerLockGuard, stack: ItemStack) {
        self.base.set_item(guard, stack);
    }

    fn set_by_player(
        &self,
        guard: &mut ContainerLockGuard,
        stack: ItemStack,
        previous: &ItemStack,
    ) {
        // Vanilla `ArmorSlot.setByPlayer` calls `owner.onEquipItem`; Steel
        // publishes the mirror to the entity's equipment right after the click
        // instead (see `MenuKind::after_click`).
        let _ = previous;
        self.set_item(guard, stack);
    }

    fn may_place(&self, stack: &ItemStack) -> bool {
        // Vanilla `ArmorSlot.mayPlace`: `owner.isEquippableInSlot(stack, slot)`,
        // which also enforces the saddle / horse-armor entity tags.
        self.mount
            .as_living_entity()
            .is_some_and(|living| living.is_equippable_in_slot(stack, self.slot))
    }

    fn may_pickup(&self, guard: &ContainerLockGuard, player: &Player) -> bool {
        let item = self.get_item(guard);
        item.is_empty()
            || player.has_infinite_materials()
            || !item.has_enchantment_effect(EnchantmentEffectComponent::PreventArmorChange)
    }

    fn get_max_stack_size(&self, _guard: &ContainerLockGuard) -> i32 {
        1
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        self.base.set_changed(guard);
    }

    fn get_container_slot(&self) -> usize {
        self.base.get_container_slot()
    }
}

/// Per-menu mount state.
pub struct MountInventoryMenuKind {
    mount: WeakEntity,
    saddle: Shared<SimpleContainer>,
    armor: Shared<SimpleContainer>,
    chest_slots: usize,
    inventory_generation: Arc<AtomicU32>,
    opened_generation: u32,
}

// SAFETY: This Steel-owned key uniquely identifies the mount menu kind.
unsafe impl DowncastType for MountInventoryMenuKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/mount_inventory");
}

impl MountInventoryMenuKind {
    /// Publishes both equipment mirrors back to the mount's equipment slots.
    fn publish_equipment(&self, mount: &SharedEntity) {
        self.publish_slot(mount, EquipmentSlot::Saddle, &self.saddle);
        self.publish_slot(mount, EquipmentSlot::Body, &self.armor);
    }

    /// Copies one mirror into the entity's equipment slot, playing the equip
    /// sound vanilla's `onEquipItem` plays when the item actually changed.
    fn publish_slot(
        &self,
        mount: &SharedEntity,
        slot: EquipmentSlot,
        mirror: &Shared<SimpleContainer>,
    ) {
        let item = mirror.lock().get_item(0).clone();
        let Some(living) = mount.as_living_entity() else {
            return;
        };

        let mut previous = ItemStack::empty();
        living.with_equipment_slot(slot, &mut |current| previous = current.clone());
        if ItemStack::matches(&previous, &item) {
            return;
        }

        living.with_equipment_slot_mut(slot, &mut |current| *current = item.clone());
        if let Some(sound) = living.equip_sound(slot, &item) {
            mount.play_sound(sound, 1.0, 1.0);
        }
        if let Some(mob) = mount.as_mob() {
            mob.set_guaranteed_drop(slot);
        }
    }

    /// Hands mirror contents back to the player when the mount is gone, so a
    /// menu open across the mount's death cannot swallow items.
    fn return_mirrors_to(&self, player: &Player) {
        for mirror in [&self.saddle, &self.armor] {
            let item = mirror.lock().get_item(0).clone();
            if !item.is_empty() {
                player.add_item_or_drop(item, Prediction::ServerOnly);
            }
        }
    }
}

impl MenuKind for MountInventoryMenuKind {
    fn after_click(&mut self, _player: &Player) {
        let Some(mount) = self.mount.upgrade() else {
            return;
        };
        if mount.is_alive() {
            self.publish_equipment(&mount);
        }
    }

    fn removed(&mut self, _behavior: &mut MenuBehavior, player: &Player) {
        let Some(mount) = self.mount.upgrade() else {
            self.return_mirrors_to(player);
            return;
        };
        if mount.is_alive() {
            self.publish_equipment(&mount);
        } else {
            self.return_mirrors_to(player);
        }
    }

    /// Vanilla `AbstractMountInventoryMenu.stillValid`.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        let Some(mount) = self.mount.upgrade() else {
            return false;
        };
        if self.inventory_generation.load(Ordering::Relaxed) != self.opened_generation {
            return false;
        }
        mount.is_alive()
            && player.is_within_entity_interaction_range_with_buffer(mount.bounding_box(), 4.0)
    }

    /// Vanilla `AbstractMountInventoryMenu.quickMoveStack`.
    fn quick_move(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        slot_index: usize,
        _player: &Player,
    ) -> Option<ItemStack> {
        let slot = &behavior.slots()[slot_index];
        if !slot.has_item(guard) {
            return Some(ItemStack::empty());
        }
        let clicked = slot.get_item(guard).clone();
        let mut remaining = clicked.clone();

        let player_start = SLOT_INVENTORY_START + self.chest_slots;
        let player_end = player_start + PlayerInventory::MAIN.len() + PlayerInventory::HOTBAR.len();

        if slot_index < player_start {
            if !behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                player_start,
                player_end,
                FillDirection::Backward,
            ) {
                return Some(ItemStack::empty());
            }
        } else if behavior.slots()[SLOT_BODY_ARMOR].may_place(&remaining)
            && !behavior.slots()[SLOT_BODY_ARMOR].has_item(guard)
        {
            if !behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                SLOT_BODY_ARMOR,
                SLOT_BODY_ARMOR + 1,
                FillDirection::Forward,
            ) {
                return Some(ItemStack::empty());
            }
        } else if behavior.slots()[SLOT_SADDLE].may_place(&remaining)
            && !behavior.slots()[SLOT_SADDLE].has_item(guard)
        {
            if !behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                SLOT_SADDLE,
                SLOT_SADDLE + 1,
                FillDirection::Forward,
            ) {
                return Some(ItemStack::empty());
            }
        } else if self.chest_slots == 0
            || !behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                SLOT_INVENTORY_START,
                player_start,
                FillDirection::Forward,
            )
        {
            let main_end = player_start + PlayerInventory::MAIN.len();
            let hotbar_start = main_end;
            let hotbar_end = hotbar_start + PlayerInventory::HOTBAR.len();
            let moved = if slot_index >= hotbar_start && slot_index < hotbar_end {
                behavior.move_item_stack_to(
                    guard,
                    slot_index,
                    &mut remaining,
                    player_start,
                    main_end,
                    FillDirection::Forward,
                )
            } else if slot_index >= player_start && slot_index < main_end {
                behavior.move_item_stack_to(
                    guard,
                    slot_index,
                    &mut remaining,
                    hotbar_start,
                    hotbar_end,
                    FillDirection::Forward,
                )
            } else {
                behavior.move_item_stack_to(
                    guard,
                    slot_index,
                    &mut remaining,
                    hotbar_start,
                    main_end,
                    FillDirection::Forward,
                )
            };
            if !moved {
                return Some(ItemStack::empty());
            }
            behavior.update_quick_move_source(guard, slot_index, &remaining, &clicked);
            // Vanilla returns `ItemStack.EMPTY` from this branch even after a
            // successful move.
            return Some(ItemStack::empty());
        }

        behavior.update_quick_move_source(guard, slot_index, &remaining, &clicked);
        Some(clicked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_size_matches_three_rows() {
        assert_eq!(get_inventory_size(0), 0);
        assert_eq!(get_inventory_size(5), 15);
        assert_eq!(get_inventory_size(-1), 0);
    }

    #[test]
    fn slot_layout_matches_the_client_menu() {
        // 2 equipment slots + chest storage + 27 main + 9 hotbar.
        assert_eq!(SLOT_INVENTORY_START + get_inventory_size(5) + 36, 53);
    }
}
