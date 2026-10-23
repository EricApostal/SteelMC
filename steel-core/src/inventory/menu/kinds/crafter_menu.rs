//! Crafter menu.
//!
//! Slot layout (46 total, mirroring Vanilla `CrafterMenu`):
//! - Slots 0-8: crafter grid, placement gated by the disabled-slot bitset
//! - Slots 9-35: main inventory (27)
//! - Slots 36-44: hotbar (9)
//! - Slot 45: preview result (display-only, always mirrors the current grid)

use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_menu_types;
use steel_utils::locks::{IntoShared, Shared};
use steel_utils::{Downcast as _, DowncastType, DowncastTypeKey};

use crate::block_entity::SharedBlockEntity;
use crate::block_entity::entities::{CRAFTER_SLOTS, CrafterBlockEntity};
use crate::inventory::container::{CraftingContainer, ResultContainer};
use crate::inventory::lock::{ContainerId, ContainerRef};
use crate::inventory::menu::builder::SectionKind;
use crate::inventory::menu::kind::MenuKind;
use crate::inventory::menu::{Menu, MenuBehavior};
use crate::inventory::prelude::*;
use crate::inventory::recipe_manager;
use crate::player::player_inventory::PlayerInventory;

/// Builds the crafter menu.
///
/// The preview result is a display-only slot over a menu-owned container that
/// is recomputed from the shared recipe registry whenever the grid changes,
/// mirroring Vanilla's `CrafterMenu.refreshRecipeResult`. The grid's placement
/// gate reads the block entity's disabled-slot bitset, which lives outside the
/// container lock so it can be consulted while that lock is held.
#[must_use]
pub fn crafter(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    container: ContainerRef,
) -> Menu {
    let owner = container.owner_block_entity();
    let result_container: Shared<ResultContainer> = ResultContainer::new().into_shared();
    let result_ref: ContainerRef = Shared::clone(&result_container).into();

    let gate_owner = owner.clone();
    let mut builder = MenuBuilder::new(&vanilla_menu_types::CRAFTER_3X3, container_id);
    let grid = builder.section_at(
        container.clone(),
        0..CRAFTER_SLOTS,
        SectionKind::restricted(move |slot, _| {
            gate_owner
                .as_ref()
                .and_then(|entity| entity.downcast_ref::<CrafterBlockEntity>())
                .is_none_or(|crafter| !crafter.is_slot_disabled(slot))
        }),
    );
    let player = builder.player_inventory(&inventory);
    builder.section_with(result_ref, 1, SectionKind::Display);
    let data = std::array::from_fn(|_| builder.data_slot(0));

    builder.route(grid, player.all(), FillDirection::Backward);
    builder.route(player.all(), grid, FillDirection::Forward);

    builder.build(CrafterKind {
        container,
        owner,
        result_container,
        data,
    })
}

/// Per-menu crafter state.
///
/// The ten data slots mirror Vanilla's `ContainerData` layout: indices 0-8 are
/// the disabled flags and index 9 is the powered flag.
pub struct CrafterKind {
    container: ContainerRef,
    owner: Option<SharedBlockEntity>,
    result_container: Shared<ResultContainer>,
    data: [DataSlot; 10],
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl DowncastType for CrafterKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/crafter");
}

impl CrafterKind {
    fn crafter(&self) -> Option<&CrafterBlockEntity> {
        self.owner
            .as_ref()
            .and_then(|entity| entity.downcast_ref::<CrafterBlockEntity>())
    }

    /// Vanilla `CrafterMenu.refreshRecipeResult`.
    fn update_result(&self, guard: &mut ContainerLockGuard) {
        let Some(container) = guard.get(self.container.container_id()) else {
            return;
        };
        let mut grid = CraftingContainer::new(3, 3);
        for slot in 0..CRAFTER_SLOTS {
            grid.set_item(slot, container.get_item(slot).clone());
        }
        let result = recipe_manager::assemble_for_container(&grid).unwrap_or_else(ItemStack::empty);

        let result_id = ContainerId::from_arc(&self.result_container);
        if let Some(result_container) = guard.get_typed_mut::<ResultContainer>(result_id) {
            result_container.set_item(0, result);
        }
    }

    /// Publishes the disabled bits and the powered flag to the client.
    fn update_data(&self, behavior: &mut MenuBehavior) {
        let crafter = self.crafter();
        let mask = crafter.map_or(0, |crafter| crafter.slot_states().disabled_mask());
        let triggered = i16::from(crafter.is_some_and(CrafterBlockEntity::is_triggered));
        for (slot, data) in self.data.iter().enumerate() {
            let value = if slot < CRAFTER_SLOTS {
                i16::from((mask & (1 << slot)) != 0)
            } else {
                triggered
            };
            data.set(behavior, value);
        }
    }
}

impl MenuKind for CrafterKind {
    /// Returns true if the backing container is still valid for the player.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.container.still_valid(player)
    }

    /// Seeds the preview result and the data slots before the first render.
    fn on_open(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_result(guard);
        self.update_data(behavior);
    }

    /// Keeps the powered indicator and disabled bits current while open.
    fn on_tick(
        &mut self,
        behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_data(behavior);
    }

    /// Vanilla `CrafterMenu.slotChanged`: recompute the preview result.
    fn slots_changed(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_result(guard);
    }

    /// Vanilla `ServerGamePacketListenerImpl.handleContainerSlotStateChanged`.
    ///
    /// `new_state` is the *enabled* flag; the container only toggles empty
    /// slots, so a rejected toggle leaves the menu unchanged. The container is
    /// already locked by `guard`, so the toggle goes through the locked storage
    /// instead of the block entity's locking wrapper.
    fn on_slot_state_changed(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        slot_id: i32,
        new_state: bool,
        _player: &Player,
    ) -> bool {
        let Ok(slot) = usize::try_from(slot_id) else {
            return false;
        };
        if slot >= CRAFTER_SLOTS {
            return false;
        }
        let Some(crafter) = self.crafter() else {
            return false;
        };
        let container_id = self.container.container_id();
        let Some(container) = guard.get(container_id) else {
            return false;
        };
        if !container.get_item(slot).is_empty() {
            return false;
        }
        crafter.apply_slot_state(slot, new_state);
        guard.set_changed(container_id);
        self.update_data(behavior);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::BlockPos;

    use super::*;
    use crate::block_entity::BlockEntity as _;

    fn crafter_entity() -> CrafterBlockEntity {
        init_vanilla_registry();
        CrafterBlockEntity::new(
            Weak::new(),
            BlockPos::new(0, 0, 0),
            vanilla_blocks::CRAFTER.default_state(),
        )
    }

    #[test]
    fn menu_has_nine_grid_slots_player_inventory_and_display_result() {
        let entity = crafter_entity();
        let container = entity.container_ref().expect("crafter has a container");

        let menu = crafter(PlayerInventory::new().into_shared(), 0, container);

        assert_eq!(
            menu.behavior().slots().len(),
            vanilla_menu_types::CRAFTER_3X3.slot_count
        );
    }
}
