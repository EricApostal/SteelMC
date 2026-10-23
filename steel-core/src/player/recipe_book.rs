//! Server-side recipe book (vanilla `net.minecraft.stats.ServerRecipeBook`).
//!
//! Tracks the recipes a player has unlocked plus the book's open/filtering flags,
//! sends the clientbound recipe-book packets, and fires the `recipe_unlocked`
//! criterion. Vanilla resolves the display entries through a
//! `ServerRecipeBook.DisplayResolver`; Steel keeps the same seam so this type
//! does not depend on the packet encoding of display entries.
//!
//! STAGED: this file is not yet wired into `Player`. See the run report for the
//! exact `player/mod.rs` / `player_data.rs` lines that must add the field, the
//! `recipe_book()` accessor and the persistence entry.

use steel_protocol::packets::game::{CRecipeBookAdd, RecipeBookAddEntry, RecipeBookSettings};
use steel_registry::recipe::{RecipeBookCategoryRef, UntypedRecipeRef};
use steel_registry::{REGISTRY, RegistryExt};
use steel_utils::Identifier;

/// Resolves the display entries to send when a recipe is unlocked.
///
/// Vanilla `ServerRecipeBook.DisplayResolver`.
pub trait DisplayResolver {
    /// Feeds every display entry of `recipe` to `consumer`, returning how many
    /// were produced.
    fn displays_for_recipe(
        &self,
        recipe: UntypedRecipeRef,
        consumer: &mut dyn FnMut(RecipeBookEntry),
    ) -> usize;
}

/// One resolved display entry, ready to be wrapped in a
/// [`RecipeBookAddEntry`].
#[derive(Debug, Clone)]
pub struct RecipeBookEntry {
    /// The recipe display contents (`net.minecraft.world.item.crafting.display.RecipeDisplayEntry`).
    pub contents: steel_protocol::packets::game::RecipeDisplayEntry,
    /// The category the display is filed under.
    pub category: RecipeBookCategoryRef,
}

/// The recipes a player knows, plus the book's UI state.
///
/// Vanilla `ServerRecipeBook` + `RecipeBook`.
#[derive(Debug, Default, Clone)]
pub struct ServerRecipeBook {
    known: Vec<Identifier>,
    highlight: Vec<Identifier>,
    settings: RecipeBookSettings,
}

impl ServerRecipeBook {
    /// Creates an empty recipe book with default settings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every unlocked recipe, in unlock order.
    #[must_use]
    pub fn known(&self) -> &[Identifier] {
        &self.known
    }

    /// The unlocked recipes as identifiers, for `PlayerView.unlocked_recipes`.
    #[must_use]
    pub fn unlocked_identifiers(&self) -> &[Identifier] {
        &self.known
    }

    /// Vanilla `contains`.
    #[must_use]
    pub fn contains(&self, id: &Identifier) -> bool {
        self.known.contains(id)
    }

    /// Vanilla `add`.
    pub fn add(&mut self, id: Identifier) {
        if !self.known.contains(&id) {
            self.known.push(id);
        }
    }

    /// Vanilla `remove`: also drops the highlight.
    pub fn remove(&mut self, id: &Identifier) {
        self.known.retain(|known| known != id);
        self.highlight.retain(|highlight| highlight != id);
    }

    /// Vanilla `removeHighlight`.
    pub fn remove_highlight(&mut self, id: &Identifier) {
        self.highlight.retain(|highlight| highlight != id);
    }

    /// The book's open/filtering flags, for `CRecipeBookSettings`.
    #[must_use]
    pub const fn settings(&self) -> RecipeBookSettings {
        self.settings
    }

    /// Vanilla `RecipeBook.setBookSetting`.
    pub fn set_book_setting(
        &mut self,
        book_type: steel_protocol::packets::game::RecipeBookType,
        open: bool,
        filtering: bool,
    ) {
        use steel_protocol::packets::game::{RecipeBookType, RecipeBookTypeSettings};
        let target = match book_type {
            RecipeBookType::Crafting => &mut self.settings.crafting,
            RecipeBookType::Furnace => &mut self.settings.furnace,
            RecipeBookType::BlastFurnace => &mut self.settings.blast_furnace,
            RecipeBookType::Smoker => &mut self.settings.smoker,
        };
        *target = RecipeBookTypeSettings { open, filtering };
    }

    /// Vanilla `ServerRecipeBook.addRecipes`: unlocks every not-yet-known,
    /// non-special recipe, highlights it, and returns the entries to send.
    ///
    /// The caller is responsible for raising `CriteriaTriggers.RECIPE_UNLOCKED`
    /// per newly unlocked recipe (see [`Self::grant_recipes`]).
    pub fn add_recipes(
        &mut self,
        recipes: &[UntypedRecipeRef],
        resolver: Option<&dyn DisplayResolver>,
    ) -> (Vec<RecipeBookAddEntry>, Vec<UntypedRecipeRef>) {
        let mut entries = Vec::new();
        let mut unlocked: Vec<UntypedRecipeRef> = Vec::new();
        for recipe in recipes {
            if self.known.contains(recipe.key()) {
                continue;
            }
            self.add(recipe.key().clone());
            self.highlight.push(recipe.key().clone());
            unlocked.push(*recipe);
            if let Some(resolver) = resolver {
                resolver.displays_for_recipe(*recipe, &mut |entry| {
                    entries.push(RecipeBookAddEntry::new(entry.contents, true, true));
                });
            }
        }
        (entries, unlocked)
    }

    /// Vanilla `ServerRecipeBook.toNetwork`: the packet that replaces the book.
    #[must_use]
    pub fn to_network(&self, entries: Vec<RecipeBookAddEntry>) -> CRecipeBookAdd {
        CRecipeBookAdd {
            entries,
            replace: true,
        }
    }

    /// Resolves every recipe id in `ids` to its registered recipe, dropping
    /// unknown ids the way vanilla's `RecipeManager.byKey` does.
    #[must_use]
    pub fn resolve_recipes(ids: &[Identifier]) -> Vec<UntypedRecipeRef> {
        ids.iter()
            .filter_map(|id| REGISTRY.recipes.by_key(id))
            .collect()
    }
}
