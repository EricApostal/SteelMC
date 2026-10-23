//! Cartography table menu.
//!
//! Slot layout (39 total), matching vanilla `CartographyTableMenu`:
//! - Slot 0: map
//! - Slot 1: additional item (paper, map, or glass pane)
//! - Slot 2: result
//! - Slots 3-38: the player's 36 inventory slots
//!
//! Vanilla consults `MapItemSavedData` before extending or locking a map. Steel
//! does not store map saved data yet, so the scale/locked checks are omitted and
//! the result is derived from the item's components alone; see the map sync gap
//! reported alongside this menu.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{
    REGISTRY, TaggedRegistryExt,
    data_components::vanilla_components::{MAP_ID, MAP_POST_PROCESSING, MapPostProcessing},
    item_stack::ItemStack,
    sound_events, vanilla_blocks,
    vanilla_item_tags::ItemTag,
    vanilla_items, vanilla_menu_types,
};
use steel_utils::{
    BlockPos,
    locks::{IntoShared, Shared},
};

use crate::{
    inventory::{
        container::{ResultContainer, SimpleContainer},
        lock::ContainerId,
        prelude::*,
        slots::ResultHandler,
    },
    player::player_inventory::{PlayerInventory, Prediction},
    world::World,
};

/// Builds the cartography table menu.
#[must_use]
pub fn cartography(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    pos: BlockPos,
    world: &Arc<World>,
) -> Menu {
    let input_container = SimpleContainer::new(2).into_shared();
    let result_container = ResultContainer::new().into_shared();

    let mut builder = MenuBuilder::new(&vanilla_menu_types::CARTOGRAPHY_TABLE, container_id);

    let mut inputs = builder.split(Arc::clone(&input_container));
    let map = builder.section_with(
        &mut inputs,
        1,
        SectionKind::restricted(|_, stack| !stack.is_empty() && stack.has(MAP_ID)),
    );
    let addition = builder.section_with(
        &mut inputs,
        1,
        SectionKind::restricted(|_, stack| is_additional_item(stack)),
    );
    let result = builder.result_slot(CartographyResultHandler {
        input_container: Arc::clone(&input_container),
        result_container: Arc::clone(&result_container),
        block_pos: pos,
        world: Arc::clone(world),
        last_sound_time: Arc::new(AtomicI64::new(-1)),
    });
    let player = builder.player_inventory(&inventory);

    builder.route_with_remainder_policy(
        result,
        player.all(),
        FillDirection::Backward,
        FakeResultRemainderPolicy::Discard,
    );
    builder.route(map, player.all(), FillDirection::Forward);
    builder.route(addition, player.all(), FillDirection::Forward);
    builder.drain([map, addition]);

    builder.build(CartographyKind {
        input_container,
        result_container,
        block_pos: pos,
        world: Arc::clone(world),
        map,
        addition,
        result,
        player,
    })
}

/// Items the second slot accepts, mirroring vanilla's slot predicate.
fn is_additional_item(stack: &ItemStack) -> bool {
    stack.is(&vanilla_items::PAPER)
        || stack.is(&vanilla_items::MAP)
        || stack.is(&vanilla_items::GLASS_PANE)
}

/// Result slot handler for the cartography table.
struct CartographyResultHandler {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    block_pos: BlockPos,
    world: Arc<World>,
    last_sound_time: Arc<AtomicI64>,
}

impl ResultHandler for CartographyResultHandler {
    fn result_container(&self) -> ContainerRef {
        ContainerRef::from(Arc::clone(&self.result_container))
    }

    fn dependencies(&self) -> Vec<ContainerRef> {
        vec![ContainerRef::from(Arc::clone(&self.input_container))]
    }

    fn update_result(&self, _guard: &mut ContainerLockGuard) {}

    fn on_result_taken(
        &self,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) -> Option<ItemStack> {
        let input_id = ContainerId::from_arc(&self.input_container);
        if let Some(input) = guard.get_mut(input_id) {
            input.remove_item(0, 1);
            input.remove_item(1, 1);
            input.set_changed();
        }

        let game_time = self.world.game_time();
        if self.last_sound_time.load(Ordering::Relaxed) != game_time {
            self.last_sound_time.store(game_time, Ordering::Relaxed);
            self.world.play_block_sound(
                &sound_events::UI_CARTOGRAPHY_TABLE_TAKE_RESULT,
                self.block_pos,
                1.0,
                1.0,
                None,
            );
        }
        None
    }

    fn is_result_valid(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        true
    }
}

/// Per-menu cartography table state.
pub struct CartographyKind {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    block_pos: BlockPos,
    world: Arc<World>,
    map: Section,
    addition: Section,
    result: Section,
    player: PlayerInventorySections,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl steel_utils::DowncastType for CartographyKind {
    const TYPE_KEY: steel_utils::DowncastTypeKey =
        steel_utils::DowncastTypeKey::new("steel:menu/cartography");
}

impl CartographyKind {
    /// Mirrors vanilla `CartographyTableMenu.setupResultSlot` without the map
    /// saved-data scale/locked checks.
    fn setup_result_slot(
        &self,
        guard: &mut ContainerLockGuard,
        map_stack: &ItemStack,
        additional_stack: &ItemStack,
    ) {
        let result_id = ContainerId::from_arc(&self.result_container);
        let Some(result_container) = guard.get_mut(result_id) else {
            return;
        };

        let mut result = ItemStack::empty();
        if additional_stack.is(&vanilla_items::PAPER)
            && REGISTRY
                .items
                .is_in_tag(map_stack.item(), &ItemTag::EXTENDABLE_MAPS)
        {
            result = map_stack.copy_with_count(1);
            result.set(MAP_POST_PROCESSING, MapPostProcessing::Scale);
        } else if additional_stack.is(&vanilla_items::GLASS_PANE) {
            result = map_stack.copy_with_count(1);
            result.set(MAP_POST_PROCESSING, MapPostProcessing::Lock);
        } else if additional_stack.is(&vanilla_items::MAP) {
            result = map_stack.copy_with_count(2);
        } else {
            result_container.set_item(0, ItemStack::empty());
            return;
        }

        if !ItemStack::matches(&result, result_container.get_item(0)) {
            result_container.set_item(0, result);
        }
    }
}

impl MenuKind for CartographyKind {
    /// Returns true while the original cartography table remains in range.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        let state = self.world.get_block_state(self.block_pos);
        state.get_block() == &vanilla_blocks::CARTOGRAPHY_TABLE
            && player.is_within_block_interaction_range_with_buffer(self.block_pos, 4.0)
    }

    fn slots_changed(
        &mut self,
        _behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        let input_id = ContainerId::from_arc(&self.input_container);
        let result_id = ContainerId::from_arc(&self.result_container);

        let (map_stack, additional_stack, result_empty) = {
            let Some(input) = guard.get(input_id) else {
                return;
            };
            let result_empty = guard
                .get(result_id)
                .is_none_or(|result| result.get_item(0).is_empty());
            (
                input.get_item(0).clone(),
                input.get_item(1).clone(),
                result_empty,
            )
        };

        let inputs_filled = !map_stack.is_empty() && !additional_stack.is_empty();
        if result_empty || inputs_filled {
            if inputs_filled {
                self.setup_result_slot(guard, &map_stack, &additional_stack);
            }
        } else if let Some(result) = guard.get_mut(result_id) {
            result.set_item(0, ItemStack::empty());
        }
    }

    /// Clears the virtual result on close. Inputs are drained by [`Menu::removed`].
    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        self.result_container.lock().set_item(0, ItemStack::empty());
    }

    fn can_take_item_for_pick_all(&self, _carried: &ItemStack, slot_index: usize) -> bool {
        !self.result.contains(slot_index)
    }

    fn quick_move(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        slot_index: usize,
        player: &Player,
    ) -> Option<ItemStack> {
        if !self.player.all().contains(slot_index) {
            return None;
        }

        let clicked = behavior.slots()[slot_index].get_item(guard).clone();
        if clicked.is_empty() {
            return Some(ItemStack::empty());
        }

        let mut remaining = clicked.clone();
        let destination = if clicked.has(MAP_ID) {
            self.map
        } else if is_additional_item(&clicked) {
            self.addition
        } else if self.player.main().contains(slot_index) {
            self.player.hotbar()
        } else {
            self.player.main()
        };

        let moved = behavior.move_item_stack_to(
            guard,
            slot_index,
            &mut remaining,
            destination.start(),
            destination.end(),
            FillDirection::Forward,
        );
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
