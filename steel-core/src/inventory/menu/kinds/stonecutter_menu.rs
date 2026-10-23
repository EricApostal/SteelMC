//! Stonecutter menu.
//!
//! Slot layout (38 total), matching vanilla `StonecutterMenu`:
//! - Slot 0: input
//! - Slot 1: result
//! - Slots 2-37: the player's 36 inventory slots

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use steel_registry::{
    REGISTRY,
    blocks::block_state_ext::BlockStateExt,
    item_stack::ItemStack,
    recipe::{SingleItemRecipeInput, StonecuttingRecipe, TypedRecipeRef, vanilla_recipe_types},
    sound_events, vanilla_blocks, vanilla_menu_types,
};
use steel_utils::{
    BlockPos,
    locks::{IntoShared, Shared, SyncMutex},
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

/// Typed reference to one stonecutting recipe.
type StonecutterRecipeRef = TypedRecipeRef<StonecuttingRecipe, SingleItemRecipeInput>;

/// Builds the stonecutter menu.
#[must_use]
pub fn stonecutter(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    pos: BlockPos,
    world: &Arc<World>,
) -> Menu {
    let input_container = SimpleContainer::new(1).into_shared();
    let result_container = ResultContainer::new().into_shared();
    let state = Arc::new(SyncMutex::new(StonecutterState::new()));
    let last_sound_time = Arc::new(AtomicI64::new(-1));

    let mut builder = MenuBuilder::new(&vanilla_menu_types::STONECUTTER, container_id);

    let input = builder.section_all(&input_container);
    let result = builder.result_slot(StonecutterResultHandler {
        input_container: Arc::clone(&input_container),
        result_container: Arc::clone(&result_container),
        state: Arc::clone(&state),
        block_pos: pos,
        world: Arc::clone(world),
        last_sound_time: Arc::clone(&last_sound_time),
    });
    let player = builder.player_inventory(&inventory);
    let selected_recipe = builder.data_slot(-1);

    builder.route_with_remainder_policy(
        result,
        player.all(),
        FillDirection::Backward,
        FakeResultRemainderPolicy::Discard,
    );
    builder.route(input, player.all(), FillDirection::Forward);
    builder.drain(input);

    builder.build(StonecutterKind {
        input_container,
        result_container,
        block_pos: pos,
        world: Arc::clone(world),
        input,
        result,
        player,
        selected_recipe,
        state,
        last_sound_time,
    })
}

/// Recipes matching the current input plus the selected recipe index.
struct StonecutterState {
    recipes: Vec<StonecutterRecipeRef>,
    /// Index into [`recipes`](Self::recipes); `-1` when nothing is selected.
    selected: i32,
    /// Input item the cached recipe list was computed for.
    last_input: ItemStack,
}

impl StonecutterState {
    fn new() -> Self {
        Self {
            recipes: Vec::new(),
            selected: -1,
            last_input: ItemStack::empty(),
        }
    }

    fn is_valid_recipe_index(&self, index: i32) -> bool {
        index >= 0 && (index as usize) < self.recipes.len()
    }
}

/// Recomputes the cached stonecutting recipe list when the input item changes.
///
/// Mirrors vanilla `StonecutterMenu.slotsChanged`/`setupRecipeList`: the
/// selection and result are cleared whenever a different input item is placed.
fn refresh_recipes(
    guard: &ContainerLockGuard,
    input_id: ContainerId,
    state: &SyncMutex<StonecutterState>,
) -> bool {
    let Some(input) = guard.get(input_id) else {
        return false;
    };
    let input_item = input.get_item(0).clone();
    let mut state = state.lock();
    if ItemStack::is_same_item(&input_item, &state.last_input) {
        return false;
    }

    state.last_input = input_item.clone();
    state.selected = -1;
    state.recipes = if input_item.is_empty() {
        Vec::new()
    } else {
        REGISTRY
            .recipes
            .matching(
                &vanilla_recipe_types::STONECUTTING,
                &SingleItemRecipeInput::new(input_item),
            )
            .collect()
    };
    true
}

/// Writes the result for the selected recipe into the result container.
fn update_result(
    guard: &mut ContainerLockGuard,
    input_id: ContainerId,
    result_id: ContainerId,
    state: &SyncMutex<StonecutterState>,
) {
    let Some([input, result]) = guard.get_disjoint_mut([input_id, result_id]) else {
        log::warn!("failed to lock stonecutter input and result containers");
        return;
    };

    let input_item = input.get_item(0);
    let state = state.lock();
    let stack = if input_item.is_empty() {
        ItemStack::empty()
    } else if state.is_valid_recipe_index(state.selected) {
        state.recipes[state.selected as usize]
            .data()
            .result
            .create()
    } else {
        ItemStack::empty()
    };
    drop(state);

    result.set_item(0, stack);
}

/// Result slot handler for the stonecutter.
struct StonecutterResultHandler {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    state: Arc<SyncMutex<StonecutterState>>,
    block_pos: BlockPos,
    world: Arc<World>,
    last_sound_time: Arc<AtomicI64>,
}

impl ResultHandler for StonecutterResultHandler {
    fn result_container(&self) -> ContainerRef {
        ContainerRef::from(Arc::clone(&self.result_container))
    }

    fn dependencies(&self) -> Vec<ContainerRef> {
        vec![ContainerRef::from(Arc::clone(&self.input_container))]
    }

    fn update_result(&self, guard: &mut ContainerLockGuard) {
        update_result(
            guard,
            ContainerId::from_arc(&self.input_container),
            ContainerId::from_arc(&self.result_container),
            &self.state,
        );
    }

    fn on_result_taken(
        &self,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) -> Option<ItemStack> {
        let input_id = ContainerId::from_arc(&self.input_container);
        if let Some(input) = guard.get_mut(input_id) {
            input.remove_item(0, 1);
            input.set_changed();
        }

        let game_time = self.world.game_time();
        if self.last_sound_time.load(Ordering::Relaxed) != game_time {
            self.last_sound_time.store(game_time, Ordering::Relaxed);
            self.world.play_block_sound(
                &sound_events::UI_STONECUTTER_TAKE_RESULT,
                self.block_pos,
                1.0,
                1.0,
                None,
            );
        }

        update_result(
            guard,
            input_id,
            ContainerId::from_arc(&self.result_container),
            &self.state,
        );
        None
    }

    fn is_result_valid(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        true
    }
}

/// Per-menu stonecutter state: input, result, cached recipes, and selection.
pub struct StonecutterKind {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    block_pos: BlockPos,
    world: Arc<World>,
    input: Section,
    result: Section,
    player: PlayerInventorySections,
    /// Client-facing selected recipe index.
    selected_recipe: DataSlot,
    state: Arc<SyncMutex<StonecutterState>>,
    last_sound_time: Arc<AtomicI64>,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl steel_utils::DowncastType for StonecutterKind {
    const TYPE_KEY: steel_utils::DowncastTypeKey =
        steel_utils::DowncastTypeKey::new("steel:menu/stonecutter");
}

/// Returns whether any stonecutting recipe accepts `stack`.
///
/// Vanilla answers this with `StonecutterRecipeInput.acceptsInput`; the recipe
/// registry is the same source of truth here.
fn accepts_input(stack: &ItemStack) -> bool {
    REGISTRY
        .recipes
        .find_match(
            &vanilla_recipe_types::STONECUTTING,
            &SingleItemRecipeInput::new(stack.clone()),
        )
        .is_some()
}

impl MenuKind for StonecutterKind {
    /// Returns true while the original stonecutter remains in range.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        let state = self.world.get_block_state(self.block_pos);
        state.get_block() == &vanilla_blocks::STONECUTTER
            && player.is_within_block_interaction_range_with_buffer(self.block_pos, 4.0)
    }

    fn slots_changed(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        let input_id = ContainerId::from_arc(&self.input_container);
        let result_id = ContainerId::from_arc(&self.result_container);
        if refresh_recipes(guard, input_id, &self.state) {
            if let Some(result) = guard.get_mut(result_id) {
                result.set_item(0, ItemStack::empty());
            }
            self.selected_recipe.set(behavior, -1);
        }
    }

    /// Clears the virtual result on close. The input is drained by [`Menu::removed`].
    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        self.result_container.lock().set_item(0, ItemStack::empty());
    }

    fn on_button_clicked(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        button_id: i32,
        _player: &Player,
    ) -> bool {
        {
            let state = self.state.lock();
            if state.selected == button_id {
                return false;
            }
            if !state.is_valid_recipe_index(button_id) {
                // Vanilla accepts any other index without changing the selection.
                return true;
            }
        }

        self.state.lock().selected = button_id;
        self.selected_recipe.set(behavior, button_id as i16);
        update_result(
            guard,
            ContainerId::from_arc(&self.input_container),
            ContainerId::from_arc(&self.result_container),
            &self.state,
        );
        true
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
        let destination = if accepts_input(&clicked) {
            self.input
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
