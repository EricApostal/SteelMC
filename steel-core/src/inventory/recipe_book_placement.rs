//! Vanilla `ServerPlaceRecipe`: fills a crafting grid from the player inventory.
//!
//! Mirrors `net.minecraft.recipebook.ServerPlaceRecipe` and
//! `net.minecraft.recipebook.PlaceRecipeHelper`: decide how many crafts the
//! inventory supports, arrange the ingredients into the grid (centering shaped
//! patterns), and report whether the client should be shown a ghost preview
//! because the ingredients are missing.

use steel_registry::data_components::vanilla_components::MAX_STACK_SIZE;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::recipe::{
    CraftingInput, CraftingRecipe, Ingredient, PlacementInfo, RecipeMatches as _, UntypedRecipeRef,
};
use steel_utils::locks::Shared;

use crate::inventory::container::{Container as _, CraftingContainer, ResultContainer};
use crate::inventory::lock::{ContainerId, ContainerLockGuard};
use crate::player::Player;
use crate::player::player_inventory::{PlayerInventory, Prediction};

/// Vanilla `RecipeBookMenu.PostPlaceAction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostPlaceAction {
    /// The grid was filled (or left untouched); nothing else to do.
    Nothing,
    /// The ingredients are missing; show the recipe as a ghost preview.
    PlaceGhostRecipe,
}

/// The grid a recipe is placed into.
pub struct PlacementGrid<'a> {
    /// The recipe's crafting grid.
    pub crafting: &'a Shared<CraftingContainer>,
    /// The grid's result container, cleared and recomputed by the caller.
    pub result: Option<&'a Shared<ResultContainer>>,
    /// Grid width in slots.
    pub width: usize,
    /// Grid height in slots.
    pub height: usize,
}

/// Vanilla `ServerPlaceRecipe.placeRecipe`.
///
/// `inventory` is the player's inventory container, which must be covered by
/// `guard`.
pub fn place_recipe(
    guard: &mut ContainerLockGuard,
    inventory: &Shared<PlayerInventory>,
    grid: &PlacementGrid<'_>,
    recipe: UntypedRecipeRef,
    use_max_items: bool,
    allow_dropping_items_to_clear: bool,
    player: &Player,
) -> PostPlaceAction {
    let Some(placement) = recipe_placement(recipe) else {
        return PostPlaceAction::Nothing;
    };
    if placement.slots_to_ingredient.is_empty() {
        return PostPlaceAction::Nothing;
    }

    if !allow_dropping_items_to_clear && !can_clear_grid(guard, inventory, grid) {
        return PostPlaceAction::Nothing;
    }

    let available = available_items(guard, inventory, grid);
    if can_craft(&placement, &available, None).is_none() {
        clear_grid(guard, grid, player);
        return PostPlaceAction::PlaceGhostRecipe;
    }

    place(
        guard,
        inventory,
        grid,
        recipe,
        &placement,
        use_max_items,
        player,
        &available,
    );
    PostPlaceAction::Nothing
}

/// The recipe's ingredient placement, or `None` for an unplaceable recipe.
fn recipe_placement(recipe: UntypedRecipeRef) -> Option<PlacementInfo> {
    recipe
        .data()
        .properties()
        .and_then(|properties| properties.placement.clone())
}

/// Vanilla `ServerPlaceRecipe.getAmountOfFreeSlotsInInventory`, over the
/// non-equipment inventory slots.
fn free_inventory_slots(inventory: &PlayerInventory) -> usize {
    (0..PlayerInventory::INVENTORY_SIZE)
        .filter(|&slot| inventory.get_item(slot).is_empty())
        .count()
}

/// Vanilla `ServerPlaceRecipe.testClearGrid`: every grid stack must fit back into
/// the inventory.
fn can_clear_grid(
    guard: &ContainerLockGuard,
    inventory: &Shared<PlayerInventory>,
    grid: &PlacementGrid<'_>,
) -> bool {
    let inv = inventory.lock();
    let mut free_slots = free_inventory_slots(&inv);
    let mut merged: Vec<(ItemRef, i32)> = Vec::new();

    let Some(crafting) = guard.get_typed::<CraftingContainer>(ContainerId::from_arc(grid.crafting))
    else {
        return false;
    };

    for stack in crafting.items() {
        if stack.is_empty() {
            continue;
        }
        let mut remaining = stack.count();
        for (item, capacity) in &mut merged {
            if remaining <= 0 {
                break;
            }
            if *item == stack.item() && *capacity > 0 {
                let moved = remaining.min(*capacity);
                remaining -= moved;
                *capacity -= moved;
            }
        }
        if remaining > 0 {
            if free_slots == 0 {
                return false;
            }
            free_slots -= 1;
            merged.push((stack.item(), stack.max_stack_size() - remaining));
        }
    }
    true
}

/// Vanilla `StackedItemContents` input: the inventory's non-equipment items plus
/// the crafting grid's contents.
fn available_items(
    guard: &ContainerLockGuard,
    inventory: &Shared<PlayerInventory>,
    grid: &PlacementGrid<'_>,
) -> Vec<ItemStack> {
    let inv = inventory.lock();
    let mut items: Vec<ItemStack> = (0..PlayerInventory::INVENTORY_SIZE)
        .map(|slot| inv.get_item(slot).clone())
        .filter(|stack| !stack.is_empty())
        .collect();
    drop(inv);

    if let Some(crafting) =
        guard.get_typed::<CraftingContainer>(ContainerId::from_arc(grid.crafting))
    {
        items.extend(crafting.items().iter().filter(|s| !s.is_empty()).cloned());
    }
    items
}

/// The number of grid slots each ingredient occupies, matching vanilla
/// `PlacementInfo.slotsToIngredientIndex`.
fn ingredient_counts(placement: &PlacementInfo) -> Vec<i32> {
    let mut counts = vec![0; placement.ingredients.len()];
    for ingredient in placement.slots_to_ingredient.iter().flatten() {
        if let Some(count) = counts.get_mut(*ingredient) {
            *count += 1;
        }
    }
    counts
}

/// Vanilla `StackedItemContents.canCraft(recipe, amount)`.
///
/// Returns the item chosen for each ingredient, or `None` when the available
/// items cannot satisfy the recipe `amount` times (`None` = once). Mirrors
/// `StackedContents`' greedy per-ingredient allocation.
fn can_craft(
    placement: &PlacementInfo,
    available: &[ItemStack],
    amount: Option<i32>,
) -> Option<Vec<ItemRef>> {
    let counts = ingredient_counts(placement);
    let mut pool: Vec<i32> = available.iter().map(ItemStack::count).collect();
    let mut used = Vec::with_capacity(placement.ingredients.len());

    for (ingredient_index, ingredient) in placement.ingredients.iter().enumerate() {
        let per_craft = counts.get(ingredient_index).copied().unwrap_or(0);
        if per_craft == 0 {
            continue;
        }
        let mut need = per_craft * amount.unwrap_or(1);
        let mut chosen: Option<ItemRef> = None;

        for (index, stack) in available.iter().enumerate() {
            if need == 0 {
                break;
            }
            if pool[index] <= 0 || !ingredient.test(stack) {
                continue;
            }
            let taken = need.min(pool[index]);
            pool[index] -= taken;
            need -= taken;
            chosen.get_or_insert(stack.item());
        }

        if need > 0 {
            return None;
        }
        used.push(chosen?);
    }
    Some(used)
}

/// Vanilla `StackedItemContents.getBiggestCraftableStack`.
fn biggest_craftable_stack(placement: &PlacementInfo, available: &[ItemStack]) -> i32 {
    let counts = ingredient_counts(placement);
    let mut upper = i32::MAX;
    for (ingredient_index, ingredient) in placement.ingredients.iter().enumerate() {
        let per_craft = counts.get(ingredient_index).copied().unwrap_or(0);
        if per_craft == 0 {
            continue;
        }
        let total: i32 = available
            .iter()
            .filter(|stack| ingredient.test(stack))
            .map(ItemStack::count)
            .sum();
        upper = upper.min(total / per_craft);
    }
    if upper == i32::MAX {
        return 0;
    }

    // Vanilla counts the craftable amount directly; Steel verifies the bound
    // against the greedy allocation so an unsatisfiable amount is never reported.
    let mut amount = upper;
    while amount > 0 {
        if can_craft(placement, available, Some(amount)).is_some() {
            return amount;
        }
        amount -= 1;
    }
    0
}

/// Vanilla `PlaceRecipeHelper.placeRecipe`.
///
/// `visit` receives `(ingredient index, grid slot index)` for every filled cell.
fn arrange(
    grid_width: usize,
    grid_height: usize,
    recipe: UntypedRecipeRef,
    placement: &PlacementInfo,
    mut visit: impl FnMut(usize, usize),
) {
    let (recipe_width, recipe_height) = match recipe.downcast_data::<CraftingRecipe>() {
        Some(CraftingRecipe::Shaped(shaped)) => (shaped.width, shaped.height),
        _ => (grid_width, grid_height),
    };
    let mut iterator = placement.slots_to_ingredient.iter();

    let mut grid_index = 0usize;
    let mut grid_y = 0usize;
    while grid_y < grid_height {
        let should_center_recipe = (recipe_height as f32) < grid_height as f32 / 2.0;
        let start_pos_center_recipe =
            (grid_height as f32 / 2.0 - recipe_height as f32 / 2.0).floor() as usize;
        if should_center_recipe && start_pos_center_recipe > grid_y {
            grid_index += grid_width;
            grid_y += 1;
        }

        let mut grid_x = 0usize;
        while grid_x < grid_width {
            let Some(entry) = iterator.next() else {
                return;
            };

            let should_center_recipe = (recipe_width as f32) < grid_width as f32 / 2.0;
            let start_pos_center_recipe =
                (grid_width as f32 / 2.0 - recipe_width as f32 / 2.0).floor() as usize;
            let mut total_recipe_width_in_grid = recipe_width;
            let mut add_ingredient_to_slot = grid_x < recipe_width;
            if should_center_recipe {
                total_recipe_width_in_grid = start_pos_center_recipe + recipe_width;
                add_ingredient_to_slot = start_pos_center_recipe <= grid_x
                    && grid_x < start_pos_center_recipe + recipe_width;
            }

            if add_ingredient_to_slot {
                if let Some(ingredient_index) = entry {
                    visit(*ingredient_index, grid_index);
                }
            } else if total_recipe_width_in_grid == grid_x {
                grid_index += grid_width - grid_x;
                break;
            }

            grid_index += 1;
            grid_x += 1;
        }
        grid_y += 1;
    }
}

/// Whether the current grid already matches `recipe` (`CraftingMenuAccess.recipeMatches`).
fn recipe_matches(
    guard: &ContainerLockGuard,
    grid: &PlacementGrid<'_>,
    recipe: UntypedRecipeRef,
) -> bool {
    let Some(crafting) = guard.get_typed::<CraftingContainer>(ContainerId::from_arc(grid.crafting))
    else {
        return false;
    };
    let Some(crafting_recipe) = recipe.downcast_data::<CraftingRecipe>() else {
        return false;
    };
    let input: CraftingInput = crafting.as_positioned_input().input;
    crafting_recipe.matches(&input)
}

/// Vanilla `ServerPlaceRecipe.placeRecipe`.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors the vanilla placement entry point"
)]
fn place(
    guard: &mut ContainerLockGuard,
    inventory: &Shared<PlayerInventory>,
    grid: &PlacementGrid<'_>,
    recipe: UntypedRecipeRef,
    placement: &PlacementInfo,
    use_max_items: bool,
    player: &Player,
    available: &[ItemStack],
) {
    let recipe_matches_placed = recipe_matches(guard, grid, recipe);
    let biggest = biggest_craftable_stack(placement, available);

    if recipe_matches_placed
        && let Some(crafting) =
            guard.get_typed::<CraftingContainer>(ContainerId::from_arc(grid.crafting))
    {
        for stack in crafting.items() {
            if !stack.is_empty() && biggest.min(stack.max_stack_size()) < stack.count() + 1 {
                return;
            }
        }
    }

    let amount_to_craft =
        calculate_amount_to_craft(guard, grid, biggest, recipe_matches_placed, use_max_items);
    let Some(items_used) = can_craft(placement, available, Some(amount_to_craft)) else {
        return;
    };
    let adjusted = clamp_to_max_stack_size(amount_to_craft, &items_used);
    let items_used = if adjusted == amount_to_craft {
        items_used
    } else {
        match can_craft(placement, available, Some(adjusted)) {
            Some(items) => items,
            None => return,
        }
    };

    clear_grid(guard, grid, player);

    let mut targets: Vec<(usize, usize)> = Vec::new();
    arrange(
        grid.width,
        grid.height,
        recipe,
        placement,
        |ingredient_index, grid_index| targets.push((ingredient_index, grid_index)),
    );

    for (ingredient_index, grid_index) in targets {
        let Some(item) = items_used.get(ingredient_index) else {
            continue;
        };
        let mut remaining = adjusted;
        while remaining > 0 {
            match move_item_to_grid(guard, inventory, grid, grid_index, *item, remaining) {
                Some(left) => remaining = left,
                None => return,
            }
        }
    }
}

/// Vanilla `ServerPlaceRecipe.calculateAmountToCraft`.
fn calculate_amount_to_craft(
    guard: &ContainerLockGuard,
    grid: &PlacementGrid<'_>,
    biggest_craftable_stack: i32,
    recipe_matches_placed: bool,
    use_max_items: bool,
) -> i32 {
    if use_max_items {
        return biggest_craftable_stack;
    }
    if recipe_matches_placed
        && let Some(crafting) =
            guard.get_typed::<CraftingContainer>(ContainerId::from_arc(grid.crafting))
    {
        let mut smallest = i32::MAX;
        for stack in crafting.items() {
            if !stack.is_empty() && smallest > stack.count() {
                smallest = stack.count();
            }
        }
        if smallest != i32::MAX {
            return smallest + 1;
        }
    }
    1
}

/// Vanilla `ServerPlaceRecipe.clampToMaxStackSize`.
fn clamp_to_max_stack_size(value: i32, items: &[ItemRef]) -> i32 {
    items.iter().fold(value, |value, item| {
        value.min(item.components.get(MAX_STACK_SIZE).unwrap_or(1))
    })
}

/// Vanilla `ServerPlaceRecipe.moveItemToGrid`.
fn move_item_to_grid(
    guard: &mut ContainerLockGuard,
    inventory: &Shared<PlayerInventory>,
    grid: &PlacementGrid<'_>,
    grid_index: usize,
    item: ItemRef,
    count: i32,
) -> Option<i32> {
    let inv_id = ContainerId::from_arc(inventory);
    let (taken, taken_count) = {
        let inv = guard.get_mut(inv_id)?;
        let inventory_slot = (0..PlayerInventory::INVENTORY_SIZE).find(|&slot| {
            let stack = inv.get_item(slot);
            !stack.is_empty() && stack.item() == item
        })?;
        if count < inv.get_item(inventory_slot).count() {
            let taken = inv.remove_item(inventory_slot, count);
            let taken_count = taken.count();
            (taken, taken_count)
        } else {
            let taken = inv.remove_item_no_update(inventory_slot);
            let taken_count = taken.count();
            (taken, taken_count)
        }
    };

    let crafting =
        guard.get_typed_mut::<CraftingContainer>(ContainerId::from_arc(grid.crafting))?;
    if grid_index >= crafting.get_container_size() {
        return None;
    }
    let current = crafting.get_item(grid_index).clone();
    if current.is_empty() {
        crafting.set_item(grid_index, taken);
    } else {
        crafting.get_item_mut(grid_index).grow(taken_count);
    }
    Some(count - taken_count)
}

/// Vanilla `ServerPlaceRecipe.clearGrid`: the grid's items go back to the
/// inventory, then the grid and the virtual result are emptied.
fn clear_grid(guard: &mut ContainerLockGuard, grid: &PlacementGrid<'_>, player: &Player) {
    let grid_id = ContainerId::from_arc(grid.crafting);
    let contents: Vec<ItemStack> = match guard.get_typed::<CraftingContainer>(grid_id) {
        Some(crafting) => crafting.items().to_vec(),
        None => return,
    };

    if let Some(crafting) = guard.get_typed_mut::<CraftingContainer>(grid_id) {
        for slot in 0..contents.len() {
            crafting.set_item(slot, ItemStack::empty());
        }
    }
    if let Some(result) = grid.result
        && let Some(container) = guard.get_mut(ContainerId::from_arc(result))
    {
        container.set_item(0, ItemStack::empty());
    }

    for stack in contents {
        player.add_item_or_drop_with_guard(guard, stack, Prediction::Predicted);
    }
}
