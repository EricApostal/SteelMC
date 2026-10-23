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

    /// Vanilla `BrewingStandMenu.PotionSlot.getMaxStackSize`: a bottle slot
    /// holds exactly one item, so an occupied slot reports no capacity.
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
            // Vanilla `moveItemStackTo(stack, 5, 41, true)`: the player
            // inventory fills from the hotbar backwards.
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.player.all().start(),
                self.player.all().end(),
                FillDirection::Backward,
            )
        } else {
            let fuel_slot = self.fuel.start();
            let ingredient_slot = self.ingredient.start();
            let bottle_slot = self.bottles.start();
            let fuel_accepts = behavior.slots()[fuel_slot].may_place(&clicked);
            let ingredient_accepts = behavior.slots()[ingredient_slot].may_place(&clicked);
            let bottle_accepts = behavior.slots()[bottle_slot].may_place(&clicked);

            if fuel_accepts {
                // Fuel first, then the ingredient slot for items that are both
                // (blaze powder is also a brewing reagent).
                behavior.move_item_stack_to(
                    guard,
                    slot_index,
                    &mut remaining,
                    fuel_slot,
                    fuel_slot + 1,
                    FillDirection::Forward,
                ) || (ingredient_accepts
                    && behavior.move_item_stack_to(
                        guard,
                        slot_index,
                        &mut remaining,
                        ingredient_slot,
                        ingredient_slot + 1,
                        FillDirection::Forward,
                    ))
            } else if ingredient_accepts {
                behavior.move_item_stack_to(
                    guard,
                    slot_index,
                    &mut remaining,
                    ingredient_slot,
                    ingredient_slot + 1,
                    FillDirection::Forward,
                )
            } else if bottle_accepts {
                behavior.move_item_stack_to(
                    guard,
                    slot_index,
                    &mut remaining,
                    bottle_slot,
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
                behavior.move_item_stack_to(
                    guard,
                    slot_index,
                    &mut remaining,
                    self.player.all().start(),
                    self.player.all().end(),
                    FillDirection::Forward,
                )
            }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::{init_vanilla_registry, item_stack::ItemStack, vanilla_items};
    use steel_utils::locks::{IntoShared as _, Shared};

    use super::{brewing_stand, FUEL_SLOT, INGREDIENT_SLOT};
    use crate::block_entity::entities::BrewingStandContainer;
    use crate::inventory::click::{Click, MouseButton};
    use crate::inventory::container::{Container as _, SimpleContainer};
    use crate::inventory::lock::ContainerRef;
    use crate::player::Player;
    use crate::player::player_inventory::PlayerInventory;
    use crate::test_support::{TestPlayerBuilder, TestWorld, fresh_test_world};

    /// Menu slot of the first main-inventory slot: the five container slots come
    /// first, then the 27 main slots in inventory order 9..36.
    const FIRST_MAIN_SLOT: usize = 5;

    /// Builds a brewing stand menu over a five-slot stand container.
    fn brewing_menu(key: &'static str) -> (TestWorld, Arc<Player>, Shared<SimpleContainer>) {
        init_vanilla_registry();
        let world_fixture = fresh_test_world(key);
        let player =
            TestPlayerBuilder::new(Arc::clone(&world_fixture.world), "BrewingTester", 1).build();
        let container = SimpleContainer::new(5).into_shared();
        (world_fixture, player, container)
    }

    #[test]
    fn blaze_powder_fills_the_fuel_slot_before_the_ingredient_slot() {
        let (_world, player, container) = brewing_menu("brewing_stand_fuel_order");
        let mut menu = brewing_stand(
            Arc::clone(&player.inventory),
            1,
            ContainerRef::from(Arc::clone(&container)),
        );

        let powder = ItemStack::with_count(&vanilla_items::BLAZE_POWDER, 3);
        assert!(
            BrewingStandContainer::is_brewing_reagent(&powder),
            "blaze powder is a reagent too, which is why the fuel slot must win"
        );
        player
            .inventory
            .lock()
            .set_item(PlayerInventory::MAIN.start, powder);

        menu.clicked(
            Click::QuickMove {
                slot: FIRST_MAIN_SLOT,
            },
            &player,
        );

        let container = container.lock();
        assert_eq!(container.get_item(FUEL_SLOT).count(), 3);
        assert!(container.get_item(INGREDIENT_SLOT).is_empty());
    }

    #[test]
    fn unmatched_inventory_items_shift_into_the_hotbar() {
        let (_world, player, container) = brewing_menu("brewing_stand_hotbar_fallback");
        let mut menu = brewing_stand(
            Arc::clone(&player.inventory),
            1,
            ContainerRef::from(Arc::clone(&container)),
        );
        player.inventory.lock().set_item(
            PlayerInventory::MAIN.start,
            ItemStack::with_count(&vanilla_items::DIRT, 5),
        );

        menu.clicked(
            Click::QuickMove {
                slot: FIRST_MAIN_SLOT,
            },
            &player,
        );

        assert!((0..5).all(|slot| container.lock().get_item(slot).is_empty()));
        // Vanilla moves main-inventory items into the hotbar in slot order.
        assert_eq!(player.inventory.lock().get_item(0).count(), 5);
    }

    #[test]
    fn container_slots_shift_out_backwards_through_the_player_inventory() {
        let (_world, player, container) = brewing_menu("brewing_stand_backwards_exit");
        let mut menu = brewing_stand(
            Arc::clone(&player.inventory),
            1,
            ContainerRef::from(Arc::clone(&container)),
        );
        container
            .lock()
            .set_item(0, ItemStack::new(&vanilla_items::GLASS_BOTTLE));

        menu.clicked(Click::QuickMove { slot: 0 }, &player);

        assert!(container.lock().get_item(0).is_empty());
        // `moveItemStackTo(stack, 5, 41, true)` fills the last menu slot first,
        // which is hotbar index 8.
        assert!(
            player
                .inventory
                .lock()
                .get_item(8)
                .is(&vanilla_items::GLASS_BOTTLE)
        );
        assert!(player.inventory.lock().get_item(9).is_empty());
    }

    #[test]
    fn potion_slots_hold_a_single_bottle() {
        let (_world, player, container) = brewing_menu("brewing_stand_potion_capacity");
        let mut menu = brewing_stand(
            Arc::clone(&player.inventory),
            1,
            ContainerRef::from(Arc::clone(&container)),
        );
        *menu.behavior_mut().carried_mut() =
            ItemStack::with_count(&vanilla_items::GLASS_BOTTLE, 5);

        menu.clicked(
            Click::Pickup {
                slot: 0,
                button: MouseButton::Left,
            },
            &player,
        );

        assert_eq!(container.lock().get_item(0).count(), 1);
        assert_eq!(menu.behavior().carried().count(), 4);
    }
}
