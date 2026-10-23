//! Passive brewing recipe data and input snapshots.

use steel_utils::{DowncastType, DowncastTypeKey};

use crate::data_component_predicate::PotionsPredicate;
use crate::data_components::vanilla_components;
use crate::item_stack::ItemStack;
use crate::item_stack_template::ItemStackTemplate;

use super::{Ingredient, RecipeData, RecipeInput, RecipeMatches, RecipeProperties};

/// Input snapshot matched by a brewing recipe: the bottle plus its reagent.
///
/// Mirrors vanilla `BrewingInput`.
#[derive(Debug, Clone)]
pub struct BrewingInput {
    pub input: ItemStack,
    pub reagent: ItemStack,
}

impl BrewingInput {
    #[must_use]
    pub const fn new(input: ItemStack, reagent: ItemStack) -> Self {
        Self { input, reagent }
    }
}

// SAFETY: This Steel-owned key uniquely identifies a brewing matching snapshot.
unsafe impl DowncastType for BrewingInput {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:recipe_input/brewing");
}

impl RecipeInput for BrewingInput {
    fn is_empty(&self) -> bool {
        self.input.is_empty() && self.reagent.is_empty()
    }
}

/// An item plus an optional potion-registry constraint.
///
/// Mirrors vanilla `PotionIngredient`: the item-side [`Ingredient`] must accept
/// the stack and, when present, the stack's `potion_contents` potion must belong
/// to the accepted potion set.
#[derive(Debug, Clone)]
pub struct PotionIngredient {
    pub ingredient: Ingredient,
    pub potions: Option<PotionsPredicate>,
}

impl PotionIngredient {
    #[must_use]
    pub const fn new(ingredient: Ingredient, potions: Option<PotionsPredicate>) -> Self {
        Self {
            ingredient,
            potions,
        }
    }

    /// Tests a stack against the item and potion constraints.
    #[must_use]
    pub fn test(&self, stack: &ItemStack) -> bool {
        if !self.ingredient.test(stack) {
            return false;
        }
        let Some(potions) = &self.potions else {
            return true;
        };
        let Some(contents) = stack.get(vanilla_components::POTION_CONTENTS) else {
            return false;
        };
        let Some(potion) = contents.potion() else {
            return false;
        };
        potions.potions().contains(potion.value())
    }
}

/// Brewing recipe data.
#[derive(Debug)]
pub struct BrewingRecipe {
    pub properties: RecipeProperties,
    pub input: PotionIngredient,
    pub reagent: PotionIngredient,
    pub output: ItemStackTemplate,
}

// SAFETY: This Steel-owned key uniquely identifies vanilla brewing recipe data.
unsafe impl DowncastType for BrewingRecipe {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:recipe_data/brewing");
}

impl RecipeData for BrewingRecipe {
    fn properties(&self) -> Option<&RecipeProperties> {
        Some(&self.properties)
    }
}

impl BrewingRecipe {
    /// Creates the recipe output for a matched input.
    #[must_use]
    pub fn assemble(&self) -> ItemStack {
        self.output.create()
    }
}

impl RecipeMatches<BrewingInput> for BrewingRecipe {
    fn matches(&self, input: &BrewingInput) -> bool {
        self.input.test(&input.input) && self.reagent.test(&input.reagent)
    }
}
