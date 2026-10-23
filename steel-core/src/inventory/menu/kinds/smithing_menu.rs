//! Smithing table menu.
//!
//! Slot layout (40 total), matching vanilla `SmithingMenu`:
//! - Slot 0: template
//! - Slot 1: base
//! - Slot 2: addition
//! - Slot 3: result
//! - Slots 4-39: the player's 36 inventory slots
//!
//! Vanilla gates the three input slots with the `smithing_template`,
//! `smithing_base`, and `smithing_addition` recipe property sets. Steel derives
//! the same accepted items from the registered smithing recipe ingredients.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{
    REGISTRY, RegistryHolder,
    data_components::vanilla_components::{ArmorTrim, PROVIDES_TRIM_MATERIAL, TRIM},
    item_stack::ItemStack,
    level_events,
    recipe::{
        Ingredient, SmithingRecipe, SmithingRecipeInput, TypedRecipeRef, vanilla_recipe_types,
    },
    vanilla_blocks, vanilla_menu_types,
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

/// Typed reference to one smithing recipe.
type SmithingRecipeRef = TypedRecipeRef<SmithingRecipe, SmithingRecipeInput>;

/// Builds the smithing table menu.
#[must_use]
pub fn smithing(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    pos: BlockPos,
    world: &Arc<World>,
) -> Menu {
    let input_container = SimpleContainer::new(3).into_shared();
    let result_container = ResultContainer::new().into_shared();
    let acceptance = Arc::new(SmithingAcceptance::collect());

    let mut builder = MenuBuilder::new(&vanilla_menu_types::SMITHING, container_id);

    let mut inputs = builder.split(Arc::clone(&input_container));
    let template = {
        let acceptance = Arc::clone(&acceptance);
        builder.section_with(
            &mut inputs,
            1,
            SectionKind::restricted(move |_, stack| acceptance.accepts(0, stack)),
        )
    };
    let base = {
        let acceptance = Arc::clone(&acceptance);
        builder.section_with(
            &mut inputs,
            1,
            SectionKind::restricted(move |_, stack| acceptance.accepts(1, stack)),
        )
    };
    let addition = {
        let acceptance = Arc::clone(&acceptance);
        builder.section_with(
            &mut inputs,
            1,
            SectionKind::restricted(move |_, stack| acceptance.accepts(2, stack)),
        )
    };
    let input_start = template.start();
    let input_end = addition.end();
    let result = builder.result_slot(SmithingResultHandler {
        input_container: Arc::clone(&input_container),
        result_container: Arc::clone(&result_container),
        block_pos: pos,
        world: Arc::clone(world),
    });
    let player = builder.player_inventory(&inventory);
    let has_recipe_error = builder.data_slot(0);

    builder.route_with_remainder_policy(
        result,
        player.all(),
        FillDirection::Backward,
        FakeResultRemainderPolicy::Discard,
    );
    builder.route(
        player.hotbar(),
        [template, base, addition],
        FillDirection::Forward,
    );
    builder.route(
        player.main(),
        [template, base, addition],
        FillDirection::Forward,
    );
    builder.drain([template, base, addition]);

    builder.build(SmithingKind {
        input_container,
        result_container,
        block_pos: pos,
        world: Arc::clone(world),
        input_start,
        input_end,
        result,
        player,
        has_recipe_error,
        acceptance,
    })
}

/// Ingredients accepted by each smithing input slot, derived from the recipes
/// that vanilla exposes through its `smithing_*` property sets.
struct SmithingAcceptance {
    templates: Vec<Ingredient>,
    bases: Vec<Ingredient>,
    additions: Vec<Ingredient>,
}

impl SmithingAcceptance {
    fn collect() -> Self {
        let mut templates = Vec::new();
        let mut bases = Vec::new();
        let mut additions = Vec::new();

        for recipe in REGISTRY
            .recipes
            .by_type(&vanilla_recipe_types::SMITHING)
            .iter()
        {
            match recipe.data() {
                SmithingRecipe::Transform(recipe) => {
                    if let Some(template) = &recipe.template {
                        templates.push(template.clone());
                    }
                    bases.push(recipe.base.clone());
                    if let Some(addition) = &recipe.addition {
                        additions.push(addition.clone());
                    }
                }
                SmithingRecipe::Trim(recipe) => {
                    templates.push(recipe.template.clone());
                    bases.push(recipe.base.clone());
                    additions.push(recipe.addition.clone());
                }
            }
        }

        Self {
            templates,
            bases,
            additions,
        }
    }

    fn accepts(&self, slot: usize, stack: &ItemStack) -> bool {
        let ingredients = match slot {
            0 => &self.templates,
            1 => &self.bases,
            _ => &self.additions,
        };
        !stack.is_empty() && ingredients.iter().any(|ingredient| ingredient.test(stack))
    }
}

/// Assembles a smithing result, mirroring vanilla `SmithingTransformRecipe` and
/// `SmithingTrimRecipe`.
fn assemble(recipe: SmithingRecipeRef, input: &SmithingRecipeInput) -> ItemStack {
    match recipe.data() {
        SmithingRecipe::Transform(recipe) => recipe
            .result
            .apply(recipe.result.count(), input.base.components_patch()),
        SmithingRecipe::Trim(recipe) => {
            let Some(material) = input.addition.get(PROVIDES_TRIM_MATERIAL) else {
                return ItemStack::empty();
            };
            let new_trim = ArmorTrim::new(
                material.material().clone(),
                RegistryHolder::reference(recipe.pattern),
            );
            if input.base.get(TRIM) == Some(&new_trim) {
                return ItemStack::empty();
            }
            let mut trimmed = input.base.copy_with_count(1);
            trimmed.set(TRIM, new_trim);
            trimmed
        }
    }
}

/// Writes the smithing result for the current inputs into the result container.
fn create_result(guard: &mut ContainerLockGuard, input_id: ContainerId, result_id: ContainerId) {
    let Some([input, result]) = guard.get_disjoint_mut([input_id, result_id]) else {
        log::warn!("failed to lock smithing input and result containers");
        return;
    };

    let recipe_input = SmithingRecipeInput::new(
        input.get_item(0).clone(),
        input.get_item(1).clone(),
        input.get_item(2).clone(),
    );
    let stack = REGISTRY
        .recipes
        .find_match(&vanilla_recipe_types::SMITHING, &recipe_input)
        .map_or_else(ItemStack::empty, |recipe| assemble(recipe, &recipe_input));
    result.set_item(0, stack);
}

/// Result slot handler for the smithing table.
struct SmithingResultHandler {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    block_pos: BlockPos,
    world: Arc<World>,
}

impl ResultHandler for SmithingResultHandler {
    fn result_container(&self) -> ContainerRef {
        ContainerRef::from(Arc::clone(&self.result_container))
    }

    fn dependencies(&self) -> Vec<ContainerRef> {
        vec![ContainerRef::from(Arc::clone(&self.input_container))]
    }

    fn update_result(&self, guard: &mut ContainerLockGuard) {
        create_result(
            guard,
            ContainerId::from_arc(&self.input_container),
            ContainerId::from_arc(&self.result_container),
        );
    }

    fn on_result_taken(
        &self,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) -> Option<ItemStack> {
        let input_id = ContainerId::from_arc(&self.input_container);
        if let Some(input) = guard.get_mut(input_id) {
            for slot in 0..3 {
                if !input.get_item(slot).is_empty() {
                    input.get_item_mut(slot).shrink_one();
                }
            }
            input.set_changed();
        }

        self.world.level_event(
            level_events::SOUND_SMITHING_TABLE_USED,
            self.block_pos,
            0,
            None,
        );
        None
    }

    fn is_result_valid(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        true
    }
}

/// Per-menu smithing table state.
pub struct SmithingKind {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    block_pos: BlockPos,
    world: Arc<World>,
    /// First menu slot of the three inputs, in vanilla's shift-click order.
    input_start: usize,
    /// One past the last menu slot of the three inputs.
    input_end: usize,
    result: Section,
    player: PlayerInventorySections,
    /// Client-facing `hasRecipeError` flag.
    has_recipe_error: DataSlot,
    acceptance: Arc<SmithingAcceptance>,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl steel_utils::DowncastType for SmithingKind {
    const TYPE_KEY: steel_utils::DowncastTypeKey =
        steel_utils::DowncastTypeKey::new("steel:menu/smithing");
}

impl MenuKind for SmithingKind {
    /// Returns true while the original smithing table remains in range.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        let state = self.world.get_block_state(self.block_pos);
        state.get_block() == &vanilla_blocks::SMITHING_TABLE
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
        create_result(guard, input_id, result_id);

        let has_recipe_error = {
            let Some(input) = guard.get(input_id) else {
                return;
            };
            let all_inputs = (0..3).all(|slot| !input.get_item(slot).is_empty());
            let result_empty = guard
                .get(result_id)
                .is_none_or(|result| result.get_item(0).is_empty());
            all_inputs && result_empty
        };
        self.has_recipe_error
            .set(behavior, i16::from(has_recipe_error));
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

        let can_move_into_inputs = self.can_move_into_input_slots(guard, &clicked);
        let mut remaining = clicked.clone();
        let (destination_start, destination_end) = if can_move_into_inputs {
            (self.input_start, self.input_end)
        } else if self.player.main().contains(slot_index) {
            (self.player.hotbar().start(), self.player.hotbar().end())
        } else {
            (self.player.main().start(), self.player.main().end())
        };

        let moved = behavior.move_item_stack_to(
            guard,
            slot_index,
            &mut remaining,
            destination_start,
            destination_end,
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

impl SmithingKind {
    /// Mirrors vanilla `SmithingMenu.canMoveIntoInputSlots`: a matching item is
    /// accepted only while its target input slot is empty.
    fn can_move_into_input_slots(&self, guard: &ContainerLockGuard, stack: &ItemStack) -> bool {
        let Some(input) = guard.get(ContainerId::from_arc(&self.input_container)) else {
            return false;
        };
        (0..3).any(|slot| input.get_item(slot).is_empty() && self.acceptance.accepts(slot, stack))
    }
}
