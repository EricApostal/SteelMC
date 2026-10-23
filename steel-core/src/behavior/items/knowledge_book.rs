//! Knowledge book item behavior.

use steel_macros::item_behavior;
use steel_registry::REGISTRY;
use steel_registry::data_components::vanilla_components::RECIPES;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_items;

use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};

/// Vanilla knowledge book behavior.
///
/// Reads the recipe ids from the `minecraft:recipes` component, consumes one
/// book (creative players keep it) and then validates every id against the
/// recipe registry. An empty or invalid book is still consumed, matching
/// vanilla `KnowledgeBookItem.use`.
#[item_behavior]
pub struct KnowledgeBookItem;

impl ItemBehavior for KnowledgeBookItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        // Vanilla snapshots the ids before `ItemStack.consume(1, player)`.
        let recipe_ids = context.inv.with_item(|stack| {
            stack
                .get(RECIPES)
                .map_or_else(Vec::new, |recipes| recipes.keys().to_vec())
        });

        let has_infinite_materials = context.player.has_infinite_materials();
        context
            .inv
            .with_item(|stack| stack.consume(1, has_infinite_materials));

        if recipe_ids.is_empty() {
            return InteractionResult::Fail;
        }

        // Vanilla logs the offending id and fails when any recipe is unknown.
        for recipe_id in &recipe_ids {
            if REGISTRY.recipes.by_key(recipe_id).is_none() {
                return InteractionResult::Fail;
            }
        }

        // Vanilla then calls `Player.awardRecipes(...)`; the player recipe book
        // is not implemented yet, so only the usage stat is recorded here.
        context.player.award_stat(
            &vanilla_stat_types::ITEM_USED,
            &vanilla_items::KNOWLEDGE_BOOK,
        );
        InteractionResult::Success
    }
}
