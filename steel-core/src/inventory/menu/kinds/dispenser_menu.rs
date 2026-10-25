//! Dispenser and dropper menu.
//!
//! Nine container slots plus the player inventory, using the shared
//! `GENERIC_3X3` menu type layout.

use steel_registry::menu_type::MenuTypeRef;
use steel_registry::vanilla_menu_types;
use steel_utils::{DowncastType, DowncastTypeKey, locks::Shared};

use crate::inventory::lock::ContainerRef;
use crate::inventory::menu::builder::{FillDirection, MenuBuilder};
use crate::inventory::menu::kind::MenuKind;
use crate::inventory::menu::{Menu, MenuBehavior};
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;

/// Builds the nine-slot dispenser menu shared by dispensers and droppers.
#[must_use]
pub fn dispenser(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    container: ContainerRef,
) -> Menu {
    let mut builder = MenuBuilder::new(menu_type(), container_id);
    let grid = builder.section(&container, 9);
    let player = builder.player_inventory(&inventory);

    builder.route(grid, player.all(), FillDirection::Backward);
    builder.route(player.all(), grid, FillDirection::Forward);

    builder.build(DispenserKind { container })
}

#[must_use]
const fn menu_type() -> MenuTypeRef {
    &vanilla_menu_types::GENERIC_3X3
}

/// Per-menu dispenser state: just the backing container for the validity check.
pub struct DispenserKind {
    /// The backing container.
    container: ContainerRef,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl DowncastType for DispenserKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/dispenser");
}

impl MenuKind for DispenserKind {
    /// Returns true if the backing container is still valid for the player.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.container.still_valid(player)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::BlockPos;
    use steel_utils::locks::IntoShared as _;

    use super::*;
    use crate::block_entity::BlockEntity;
    use crate::block_entity::entities::{DispenserBlockEntity, DropperBlockEntity};

    fn player_inventory() -> Shared<PlayerInventory> {
        PlayerInventory::new().into_shared()
    }

    #[test]
    fn dispenser_menu_has_nine_container_slots_then_the_player_inventory() {
        init_vanilla_registry();
        let entity = DispenserBlockEntity::new(
            Weak::new(),
            BlockPos::new(0, 0, 0),
            vanilla_blocks::DISPENSER.default_state(),
        );
        let container = entity.container_ref().expect("dispenser has a container");

        let menu = dispenser(player_inventory(), 0, container);

        assert_eq!(menu.behavior().slots().len(), 45);
    }

    #[test]
    fn dropper_and_dispenser_share_the_menu_layout() {
        init_vanilla_registry();
        let entity = DropperBlockEntity::new(
            Weak::new(),
            BlockPos::new(0, 0, 0),
            vanilla_blocks::DROPPER.default_state(),
        );
        let container = entity.container_ref().expect("dropper has a container");

        let menu = dispenser(player_inventory(), 0, container);

        assert_eq!(menu.behavior().slots().len(), 45);
    }
}
