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

        // Vanilla logs the offending id and fails when any recipe is unknown;
        // nothing is awarded in that case because the loop returns first.
        for recipe_id in &recipe_ids {
            if REGISTRY.recipes.by_key(recipe_id).is_none() {
                log::error!("Invalid recipe: {recipe_id}");
                return InteractionResult::Fail;
            }
        }

        // Vanilla `Player.awardRecipes(Collection<RecipeHolder<?>>)` with the
        // list resolved from the component's ids.
        context.player.award_recipes_by_key(&recipe_ids);
        context.player.award_stat(
            &vanilla_stat_types::ITEM_USED,
            &vanilla_items::KNOWLEDGE_BOOK,
        );
        InteractionResult::Success
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::data_components::components::Recipes;
    use steel_registry::data_components::vanilla_components::RECIPES;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{REGISTRY, init_vanilla_registry, vanilla_items};
    use steel_utils::Identifier;
    use steel_utils::types::InteractionHand;

    use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext, init_behaviors};
    use crate::player::Player;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};
    use crate::world::World;

    use super::KnowledgeBookItem;

    fn book_with_recipes(recipe_keys: Vec<Identifier>) -> ItemStack {
        let mut stack = ItemStack::new(&vanilla_items::KNOWLEDGE_BOOK);
        stack.set(RECIPES, Recipes::new(recipe_keys));
        stack
    }

    fn use_book(player: &Arc<Player>, world: &Arc<World>, stack: ItemStack) -> InteractionResult {
        player
            .inventory
            .lock()
            .set_item_in_hand(InteractionHand::MainHand, stack);
        let mut context = UseItemContext::new(
            player,
            InteractionHand::MainHand,
            world,
            Arc::clone(&player.inventory),
        );
        KnowledgeBookItem.use_item(&mut context)
    }

    /// Vanilla `KnowledgeBookItem.use` awards every recipe id in the
    /// `minecraft:recipes` component to the reader's recipe book.
    #[test]
    fn knowledge_book_unlocks_the_component_recipes() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("itemfact_knowledge_book");
        let world = &world_fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "BookReader", 1).build();

        let key = REGISTRY
            .recipes
            .iter()
            .next()
            .expect("recipes exist")
            .key()
            .clone();
        assert!(
            !player.recipe_book().contains(&key),
            "the reader starts with no unlocked recipe"
        );

        let outcome = use_book(&player, world, book_with_recipes(vec![key.clone()]));

        assert_eq!(outcome, InteractionResult::Success);
        assert!(
            player.recipe_book().contains(&key),
            "taking a knowledge book unlocks {}",
            key
        );
    }

    /// Vanilla logs an unknown id and fails the use without unlocking anything.
    #[test]
    fn knowledge_book_with_an_unknown_recipe_fails_without_unlocking() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("itemfact_knowledge_book_invalid");
        let world = &world_fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "BookReader", 1).build();

        let outcome = use_book(
            &player,
            world,
            book_with_recipes(vec![Identifier::vanilla_static("not_a_recipe")]),
        );

        assert_eq!(outcome, InteractionResult::Fail);
        assert!(player.recipe_book().unlocked_identifiers().is_empty());
    }
}
