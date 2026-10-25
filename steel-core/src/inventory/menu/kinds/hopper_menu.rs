//! Hopper menu: 5 container slots plus the player inventory.
//!
//! Vanilla parity: `HopperMenu` (`CONTAINER_SIZE = 5`).

use steel_registry::vanilla_menu_types;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::block_entity::entities::HOPPER_SLOTS;
use crate::inventory::prelude::*;
use crate::player::player_inventory::PlayerInventory;

/// Builds a hopper menu with 5 container slots plus the player inventory.
#[must_use]
pub fn hopper(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    container: ContainerRef,
) -> Menu {
    let mut builder = MenuBuilder::new(&vanilla_menu_types::HOPPER, container_id);
    let hopper = builder.section(&container, HOPPER_SLOTS);
    let player = builder.player_inventory(&inventory);

    builder.route(hopper, player.all(), FillDirection::Backward);
    builder.route(player.all(), hopper, FillDirection::Forward);

    builder.build(HopperKind { container })
}

/// Per-menu hopper state: just the backing container for the validity check.
pub struct HopperKind {
    container: ContainerRef,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl DowncastType for HopperKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/hopper");
}

impl MenuKind for HopperKind {
    /// Returns true if the backing container is still valid for the player.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.container.still_valid(player)
    }

    fn on_button_clicked(
        &mut self,
        _behavior: &mut MenuBehavior,
        _guard: &mut ContainerLockGuard,
        _button_id: i32,
        _player: &Player,
    ) -> bool {
        // Vanilla hoppers expose no container buttons.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hopper_menu_type_slot_count_matches_layout() {
        // 5 container slots + 27 main inventory + 9 hotbar.
        assert_eq!(vanilla_menu_types::HOPPER.slot_count, HOPPER_SLOTS + 36);
    }
}
