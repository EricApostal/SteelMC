//! Brewing stand menu.
//!
//! Five container slots (bottles 0-2, ingredient 3, fuel 4) plus the player
//! inventory; four data slots track brew time, fuel, and their totals.

use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_menu_types;
use steel_utils::{DowncastType, DowncastTypeKey, locks::Shared};

use crate::block_entity::entities::BrewingStandContainer;
use crate::inventory::menu::builder::SectionKind;
use crate::inventory::prelude::*;
use crate::inventory::slots::{NormalSlot, RestrictedSlot, SlotStorage};
use crate::player::player_inventory::{PlayerInventory, Prediction};

/// Bottle slots (0-2): potion inputs only, into empty slots.
const BOTTLE_SLOTS: [usize; 3] = [0, 1, 2];
/// Ingredient slot (3).
const INGREDIENT_SLOT: usize = 3;
/// Fuel slot (4).
const FUEL_SLOT: usize = 4;

/// Builds the vanilla brewing stand menu for one block entity.
#[must_use]
pub fn brewing_stand(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    container: ContainerRef,
) -> Menu {
    let mut builder = MenuBuilder::new(&vanilla_menu_types::BREWING_STAND, container_id);

    let bottles = builder.section_at(
        container.clone(),
        BOTTLE_SLOTS,
        SectionKind::custom(|container, index| {
            Box::new(BrewingPotionSlot::new(container.clone(), index))
        }),
    );
    let ingredient = builder.section_at(
        container.clone(),
        [INGREDIENT_SLOT],
        SectionKind::custom(|container, index| {
            Box::new(RestrictedSlot::new(container.clone(), index, |_, stack| {
                BrewingStandContainer::is_brewing_reagent(stack)
            }))
        }),
    );
    let fuel = builder.section_at(
        container.clone(),
        [FUEL_SLOT],
        SectionKind::custom(|container, index| {
            Box::new(RestrictedSlot::new(container.clone(), index, |_, stack| {
                BrewingStandContainer::is_brewing_fuel(stack)
            }))
        }),
    );
    let player = builder.player_inventory(&inventory);
    let data = std::array::from_fn(|_| builder.data_slot(0));

    builder.route(bottles, player.all(), FillDirection::Backward);
    builder.route(ingredient, player.all(), FillDirection::Backward);
    builder.route(fuel, player.all(), FillDirection::Backward);
    builder.route(
        player.all(),
        Vec::from([bottles, ingredient, fuel]),
        FillDirection::Forward,
    );

    builder.build(BrewingStandKind {
        container,
        bottles,
        ingredient,
        fuel,
        player,
        data,
    })
}

/// Vanilla `BrewingStandMenu.PotionSlot`: accepts potion inputs into empty slots.
struct BrewingPotionSlot {
    base: NormalSlot,
}

// SAFETY: This Steel-owned key uniquely identifies brewing stand bottle slots.
unsafe impl DowncastType for BrewingPotionSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:slot/brewing_stand_potion");
}

impl BrewingPotionSlot {
    fn new(container: impl Into<ContainerRef>, index: usize) -> Self {
        Self {
            base: NormalSlot::new(container, index),
        }
    }
}

impl Slot for BrewingPotionSlot {
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

    fn may_place(&self, stack: &ItemStack) -> bool {
        BrewingStandContainer::is_potion_input(stack)
    }

    fn get_max_stack_size(&self, guard: &ContainerLockGuard) -> i32 {
        self.base.get_max_stack_size(guard)
    }

    /// Vanilla's potion slot only accepts a potion input into an empty slot, so
    /// an occupied bottle slot reports no capacity for any insertion.
    fn get_max_stack_size_for_item(&self, guard: &ContainerLockGuard, stack: &ItemStack) -> i32 {
        if self.base.get_item(guard).is_empty() {
            self.base.get_max_stack_size_for_item(guard, stack)
        } else {
            0
        }
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        self.base.set_changed(guard);
    }

    fn get_container_slot(&self) -> usize {
        self.base.get_container_slot()
    }
}

/// State and shift-click behavior for the brewing stand menu.
pub struct BrewingStandKind {
    container: ContainerRef,
    bottles: Section,
    ingredient: Section,
    fuel: Section,
    player: PlayerInventorySections,
    data: [DataSlot; 4],
}

// SAFETY: This Steel-owned key uniquely identifies brewing stand menu behavior.
unsafe impl DowncastType for BrewingStandKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/brewing_stand");
}

impl BrewingStandKind {
    fn update_data(&self, behavior: &mut MenuBehavior, guard: &ContainerLockGuard) {
        let Some(container) =
            guard.get_typed::<BrewingStandContainer>(self.container.container_id())
        else {
            return;
        };
        for (slot, value) in self.data.into_iter().zip(container.data()) {
            slot.set(behavior, value);
        }
    }
}

impl MenuKind for BrewingStandKind {
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.container.still_valid(player)
    }

    fn on_open(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_data(behavior, guard);
    }

    fn on_tick(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_data(behavior, guard);
    }

    fn quick_move(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        slot_index: usize,
        player: &Player,
    ) -> Option<ItemStack> {
        if slot_index >= behavior.slots().len() {
            return Some(ItemStack::empty());
        }
        let clicked = behavior.slots()[slot_index].get_item(guard).clone();
        if clicked.is_empty() {
            return Some(ItemStack::empty());
        }
        let mut remaining = clicked.clone();

        let container_slot = self.bottles.contains(slot_index)
            || self.ingredient.contains(slot_index)
            || self.fuel.contains(slot_index);
        let moved = if container_slot {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.player.all().start(),
                self.player.all().end(),
                FillDirection::Forward,
            )
        } else if BrewingStandContainer::is_brewing_reagent(&clicked) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.ingredient.start(),
                self.ingredient.end(),
                FillDirection::Forward,
            )
        } else if BrewingStandContainer::is_brewing_fuel(&clicked) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.fuel.start(),
                self.fuel.end(),
                FillDirection::Forward,
            )
        } else if BrewingStandContainer::is_potion_input(&clicked) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.bottles.start(),
                self.bottles.end(),
                FillDirection::Forward,
            )
        } else if self.player.main().contains(slot_index) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.player.hotbar().start(),
                self.player.hotbar().end(),
                FillDirection::Forward,
            )
        } else if self.player.hotbar().contains(slot_index) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.player.main().start(),
                self.player.main().end(),
                FillDirection::Forward,
            )
        } else {
            false
        };

        if !moved {
            return Some(ItemStack::empty());
        }
        behavior.update_quick_move_source(guard, slot_index, &remaining, &clicked);
        if remaining.count() == clicked.count() {
            return Some(ItemStack::empty());
        }
        if let Some(remainder) = behavior.slots()[slot_index].on_take(guard, &remaining, player) {
            player.add_item_or_drop_with_guard(guard, remainder, Prediction::Predicted);
        }
        Some(clicked)
    }
}
