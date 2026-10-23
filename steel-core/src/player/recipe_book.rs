//! Server-side recipe book (vanilla `net.minecraft.stats.ServerRecipeBook`).
//!
//! Tracks the recipes a player has unlocked plus the book's open/filtering flags,
//! resolves the recipe displays to send (vanilla
//! `RecipeManager.unpackRecipeInfo`, fed to the book through
//! `ServerRecipeBook.DisplayResolver`), sends the clientbound recipe-book packets
//! and fires the `recipe_unlocked` criterion.

use std::sync::OnceLock;

use rustc_hash::FxHashMap;
use steel_protocol::packets::game::{
    CRecipeBookAdd, CRecipeBookRemove, CRecipeBookSettings, ItemHolder, ItemHolderSet,
    RecipeBookAddEntry, RecipeBookSettings, RecipeBookType, RecipeBookTypeSettings, RecipeDisplay,
    RecipeDisplayEntry, RecipeDisplayId, SlotDisplay,
};
use steel_registry::advancement::triggers::TriggerEvent;
use steel_registry::item_stack::ItemStack;
use steel_registry::recipe::vanilla_recipe_book_categories as categories;
use steel_registry::recipe::{
    CookingBookCategory, CookingRecipe, CraftingBookCategory, CraftingRecipe, Ingredient,
    RecipeBookCategoryKind, RecipeBookCategoryRef, ShapedRecipe, ShapelessRecipe, SmithingRecipe,
    StonecuttingRecipe, UntypedRecipeRef, vanilla_recipe_types,
};
use steel_registry::{REGISTRY, RegistryEntry as _, RegistryExt as _, vanilla_items};
use steel_utils::Identifier;

use crate::entity::{Entity as _, living_loot_equipment};
use crate::player::Player;
use crate::player::advancement::triggers::{WorldLevelView, event_base, player_view, raise};

/// Vanilla `CriteriaTriggers.RECIPE_UNLOCKED`'s trigger id.
const RECIPE_UNLOCKED_TRIGGER: Identifier = Identifier::vanilla_static("recipe_unlocked");

/// Vanilla `CriteriaTriggers.RECIPE_CRAFTED`'s trigger id.
const RECIPE_CRAFTED_TRIGGER: Identifier = Identifier::vanilla_static("recipe_crafted");

/// Resolves the display entries to send when a recipe is unlocked or removed.
///
/// Vanilla `ServerRecipeBook.DisplayResolver`.
pub trait DisplayResolver {
    /// Feeds every display entry of `recipe` to `consumer`, returning how many
    /// were produced.
    fn displays_for_recipe(
        &self,
        recipe: UntypedRecipeRef,
        consumer: &mut dyn FnMut(&'static RecipeDisplayEntry),
    ) -> usize;
}

/// The process-wide display resolver backed by the frozen recipe registry.
///
/// Vanilla builds one `ServerRecipeBook` per player, each holding the server-level
/// resolver; Steel keeps a single stateless resolver and resolves the entries
/// from a lazily built index over `REGISTRY.recipes`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RegistryDisplayResolver;

/// The shared [`RegistryDisplayResolver`].
pub static REGISTRY_DISPLAY_RESOLVER: RegistryDisplayResolver = RegistryDisplayResolver;

impl DisplayResolver for RegistryDisplayResolver {
    fn displays_for_recipe(
        &self,
        recipe: UntypedRecipeRef,
        consumer: &mut dyn FnMut(&'static RecipeDisplayEntry),
    ) -> usize {
        let Some(index) = display_index() else {
            return 0;
        };
        let Some(ids) = index.by_recipe.get(recipe.key()) else {
            return 0;
        };
        for &display_id in ids {
            consumer(&index.entries[display_id]);
        }
        ids.len()
    }
}

/// The recipe owning one display id, i.e. vanilla `RecipeManager.ServerDisplayInfo.parent`.
#[must_use]
pub fn recipe_for_display(display: RecipeDisplayId) -> Option<UntypedRecipeRef> {
    let index = display_index()?;
    let owner = index.owners.get(usize::try_from(display.index).ok()?)?;
    REGISTRY.recipes.by_key(owner)
}

/// The display entry registered for `id`, i.e. vanilla `RecipeManager.getRecipeFromDisplay`.
#[must_use]
pub fn display_for_id(id: RecipeDisplayId) -> Option<&'static RecipeDisplayEntry> {
    let index = display_index()?;
    index.entries.get(usize::try_from(id.index).ok()?)
}

/// Vanilla `Recipe.isSpecial`, sourced from the recipe's own properties.
#[must_use]
pub fn is_special(recipe: UntypedRecipeRef) -> bool {
    recipe
        .data()
        .properties()
        .is_some_and(|properties| properties.special)
}

/// Vanilla `Recipe.showNotification`, sourced from the recipe's own properties.
#[must_use]
pub fn show_notification(recipe: UntypedRecipeRef) -> bool {
    recipe
        .data()
        .properties()
        .and_then(|properties| properties.recipe_book.as_ref())
        .is_some_and(|book| book.show_notification)
}

/// The recipes a player knows, plus the book's UI state.
///
/// Vanilla `ServerRecipeBook` + `RecipeBook`. Vanilla keeps `known`/`highlight` in
/// identity hash sets; Steel keeps insertion-ordered vectors so the persisted file
/// and the initial book packet are deterministic.
#[derive(Debug, Default, Clone)]
pub struct ServerRecipeBook {
    known: Vec<Identifier>,
    highlight: Vec<Identifier>,
    settings: RecipeBookSettings,
    /// Recipe ids unlocked while the advancement lock was held (`grant_reward`),
    /// which must be applied after that lock is released.
    pending: Vec<Identifier>,
}

/// A persisted recipe book, matching vanilla `ServerRecipeBook.Packed`.
///
/// Vanilla stores this under the player's `recipeBook` tag with `recipes`
/// (known) and `toBeDisplayed` (highlight) lists plus the book settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackedRecipeBook {
    /// The book's open/filtering flags.
    pub settings: RecipeBookSettings,
    /// Every unlocked recipe (`recipes`).
    pub known: Vec<Identifier>,
    /// Recipes the book should still highlight (`toBeDisplayed`).
    pub highlight: Vec<Identifier>,
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
    pub fn set_book_setting(&mut self, book_type: RecipeBookType, open: bool, filtering: bool) {
        let target = match book_type {
            RecipeBookType::Crafting => &mut self.settings.crafting,
            RecipeBookType::Furnace => &mut self.settings.furnace,
            RecipeBookType::BlastFurnace => &mut self.settings.blast_furnace,
            RecipeBookType::Smoker => &mut self.settings.smoker,
        };
        *target = RecipeBookTypeSettings { open, filtering };
    }

    /// Vanilla `ServerRecipeBook.addRecipes`: unlocks every not-yet-known,
    /// non-special recipe, highlights it and resolves its display entries.
    ///
    /// Returns the entries to send and the recipes that were newly unlocked; the
    /// caller sends `CRecipeBookAdd` and raises `recipe_unlocked` per recipe.
    pub fn add_recipes(
        &mut self,
        recipes: &[UntypedRecipeRef],
        resolver: &dyn DisplayResolver,
    ) -> (Vec<RecipeBookAddEntry>, Vec<UntypedRecipeRef>) {
        let mut entries = Vec::new();
        let mut unlocked: Vec<UntypedRecipeRef> = Vec::new();
        for recipe in recipes {
            let id = recipe.key();
            if self.known.contains(id) || is_special(*recipe) {
                continue;
            }
            self.add(id.clone());
            self.highlight.push(id.clone());
            unlocked.push(*recipe);
            let notification = show_notification(*recipe);
            resolver.displays_for_recipe(*recipe, &mut |display| {
                entries.push(RecipeBookAddEntry::new(display.clone(), notification, true));
            });
        }
        (entries, unlocked)
    }

    /// Vanilla `ServerRecipeBook.removeRecipes`: forgets every known recipe and
    /// collects the display ids the client must drop.
    pub fn remove_recipes(
        &mut self,
        recipes: &[UntypedRecipeRef],
        resolver: &dyn DisplayResolver,
    ) -> Vec<RecipeDisplayId> {
        let mut removed = Vec::new();
        for recipe in recipes {
            let id = recipe.key();
            if !self.known.contains(id) {
                continue;
            }
            self.remove(id);
            resolver.displays_for_recipe(*recipe, &mut |display| {
                removed.push(display.id);
            });
        }
        removed
    }

    /// Vanilla `ServerRecipeBook.sendInitialRecipeBook`'s entry list: every known
    /// recipe's displays, notified only when still highlighted.
    #[must_use]
    pub fn initial_entries(&self, resolver: &dyn DisplayResolver) -> Vec<RecipeBookAddEntry> {
        let mut entries = Vec::with_capacity(self.known.len());
        for id in &self.known {
            let Some(recipe) = REGISTRY.recipes.by_key(id) else {
                continue;
            };
            let highlight = self.highlight.contains(id);
            resolver.displays_for_recipe(recipe, &mut |display| {
                entries.push(RecipeBookAddEntry::new(display.clone(), false, highlight));
            });
        }
        entries
    }

    /// Vanilla `ServerRecipeBook.toNetwork`: the packet that replaces the book.
    #[must_use]
    pub fn to_network(&self, entries: Vec<RecipeBookAddEntry>) -> CRecipeBookAdd {
        CRecipeBookAdd {
            entries,
            replace: true,
        }
    }

    /// Vanilla `ServerRecipeBook.pack`.
    #[must_use]
    pub fn pack(&self) -> PackedRecipeBook {
        PackedRecipeBook {
            settings: self.settings,
            known: self.known.clone(),
            highlight: self.highlight.clone(),
        }
    }

    /// Vanilla `ServerRecipeBook.apply`: replaces the book with `packed`.
    pub fn apply(&mut self, packed: PackedRecipeBook) {
        self.settings = packed.settings;
        self.known = packed.known;
        self.highlight = packed.highlight;
        self.pending.clear();
    }

    /// Vanilla `ServerRecipeBook.loadUntrusted`: keeps only the ids `validator`
    /// accepts, logging the rest the way vanilla logs unrecognized recipes.
    pub fn load_untrusted(
        &mut self,
        packed: PackedRecipeBook,
        mut validator: impl FnMut(&Identifier) -> bool,
    ) {
        self.settings = packed.settings;
        load_recipes(packed.known, &mut self.known, &mut validator);
        load_recipes(packed.highlight, &mut self.highlight, &mut validator);
    }

    /// Queues a recipe unlock that must wait for the advancement lock to be released.
    ///
    /// Vanilla awards advancement-reward recipes reentrantly; Steel defers them
    /// because trigger evaluation holds the player's advancement mutex.
    pub fn queue_unlock(&mut self, id: Identifier) {
        if !self.pending.contains(&id) {
            self.pending.push(id);
        }
    }

    /// Takes the queued recipe unlocks.
    pub fn take_pending(&mut self) -> Vec<Identifier> {
        std::mem::take(&mut self.pending)
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

/// Vanilla `ServerRecipeBook.loadRecipes`.
fn load_recipes(
    recipes: Vec<Identifier>,
    target: &mut Vec<Identifier>,
    validator: &mut impl FnMut(&Identifier) -> bool,
) {
    for recipe in recipes {
        if !validator(&recipe) {
            log::error!("Tried to load unrecognized recipe: {recipe}, removed now.");
            continue;
        }
        if !target.contains(&recipe) {
            target.push(recipe);
        }
    }
}

impl Player {
    /// Vanilla `ServerPlayer.awardRecipes`: returns the number of display entries
    /// sent, which is also what `/recipe give` reports.
    pub fn award_recipes(&self, recipes: &[UntypedRecipeRef]) -> i32 {
        let (entries, unlocked) = {
            let mut book = self.recipe_book.lock();
            book.add_recipes(recipes, &REGISTRY_DISPLAY_RESOLVER)
        };
        let count = entries.len();
        if !entries.is_empty() {
            self.send_packet(CRecipeBookAdd {
                entries,
                replace: false,
            });
        }
        self.trigger_recipe_unlocked(&unlocked);
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// Vanilla `ServerPlayer.awardRecipesByKey`.
    pub fn award_recipes_by_key(&self, recipe_ids: &[Identifier]) {
        let recipes = ServerRecipeBook::resolve_recipes(recipe_ids);
        self.award_recipes(&recipes);
    }

    /// Vanilla `ServerPlayer.resetRecipes`: returns the number of removed displays.
    pub fn reset_recipes(&self, recipes: &[UntypedRecipeRef]) -> i32 {
        let removed = {
            let mut book = self.recipe_book.lock();
            book.remove_recipes(recipes, &REGISTRY_DISPLAY_RESOLVER)
        };
        let count = removed.len();
        if !removed.is_empty() {
            self.send_packet(CRecipeBookRemove { recipes: removed });
        }
        i32::try_from(count).unwrap_or(i32::MAX)
    }

    /// Queues recipe ids unlocked while the advancement lock was held.
    ///
    /// Vanilla `AdvancementRewards.grant` calls `awardRecipesByKey` directly; Steel
    /// defers it to [`Self::flush_pending_recipe_awards`] because the rewards run
    /// inside `PlayerAdvancement::award`'s lock.
    pub fn queue_recipe_awards(&self, recipe_ids: impl IntoIterator<Item = Identifier>) {
        let mut book = self.recipe_book.lock();
        for id in recipe_ids {
            book.queue_unlock(id);
        }
    }

    /// Applies every queued recipe award, including those queued by the resulting
    /// `recipe_unlocked` criteria. Must be called without the advancement lock.
    pub fn flush_pending_recipe_awards(&self) {
        loop {
            let ids = {
                let mut book = self.recipe_book.lock();
                book.take_pending()
            };
            if ids.is_empty() {
                return;
            }
            self.award_recipes_by_key(&ids);
        }
    }

    /// Vanilla `ServerRecipeBook.sendInitialRecipeBook`: settings, then the whole book.
    pub fn send_initial_recipe_book(&self) {
        let (settings, entries) = {
            let book = self.recipe_book.lock();
            (
                book.settings(),
                book.initial_entries(&REGISTRY_DISPLAY_RESOLVER),
            )
        };
        self.send_packet(CRecipeBookSettings {
            book_settings: settings,
        });
        self.send_packet(CRecipeBookAdd {
            entries,
            replace: true,
        });
    }

    /// Vanilla `ServerGamePacketListenerImpl.handleRecipeBookChangeSettingsPacket`.
    pub fn handle_recipe_book_change_settings(
        &self,
        packet: &steel_protocol::packets::game::SRecipeBookChangeSettings,
    ) {
        self.recipe_book.lock().set_book_setting(
            packet.book_type,
            packet.is_open,
            packet.is_filtering,
        );
    }

    /// Vanilla `ServerGamePacketListenerImpl.handleRecipeBookSeenRecipePacket`.
    pub fn handle_recipe_book_seen_recipe(
        &self,
        packet: &steel_protocol::packets::game::SRecipeBookSeenRecipe,
    ) {
        let Some(recipe) = recipe_for_display(packet.recipe) else {
            return;
        };
        self.recipe_book.lock().remove_highlight(recipe.key());
    }

    /// Vanilla `ServerPlayer.awardRecipes`'s `RECIPE_UNLOCKED` trigger per recipe.
    fn trigger_recipe_unlocked(&self, recipes: &[UntypedRecipeRef]) {
        if recipes.is_empty() {
            return;
        }
        // Snapshot the book first: the event must not hold the book lock while
        // `raise` takes the advancement lock (a rewards grant re-enters the book).
        let known = {
            let book = self.recipe_book.lock();
            book.unlocked_identifiers().to_vec()
        };
        let world = self.get_world();
        let origin = self.block_position();
        let level = WorldLevelView::new(&world, origin);
        let equipment = living_loot_equipment(self);
        let view = player_view(self, &level, Some(&equipment), &known);
        for recipe in recipes {
            let event = TriggerEvent::RecipeUnlocked {
                base: event_base(&view, &level, origin),
                recipe: recipe.key(),
            };
            raise(self, &RECIPE_UNLOCKED_TRIGGER, &event);
        }
    }
}

/// Vanilla `ServerPlayer.triggerRecipeCrafted`.
///
/// Raised by the crafting take path (`RecipeCraftingHolder.awardUsedRecipes`) with
/// the crafting grid's items, so it lives beside the other recipe trigger.
pub fn raise_recipe_crafted(player: &Player, recipe: &Identifier, ingredients: &[ItemStack]) {
    // See `Player::trigger_recipe_unlocked`: the book lock must not be held while
    // the advancement lock is taken.
    let known = {
        let book = player.recipe_book.lock();
        book.unlocked_identifiers().to_vec()
    };
    let world = player.get_world();
    let origin = player.block_position();
    let level = WorldLevelView::new(&world, origin);
    let equipment = living_loot_equipment(player);
    let view = player_view(player, &level, Some(&equipment), &known);
    let event = TriggerEvent::RecipeCrafted {
        base: event_base(&view, &level, origin),
        recipe,
        ingredients,
    };
    raise(player, &RECIPE_CRAFTED_TRIGGER, &event);
}

/// Every display entry of the whole recipe registry, indexed the way vanilla's
/// `RecipeManager` indexes `allDisplays`/`recipeToDisplay`.
struct DisplayIndex {
    entries: Vec<RecipeDisplayEntry>,
    /// Recipes owning each entry, parallel to `entries`.
    owners: Vec<Identifier>,
    by_recipe: FxHashMap<Identifier, Vec<usize>>,
}

static DISPLAY_INDEX: OnceLock<DisplayIndex> = OnceLock::new();

/// The display index, or `None` while the recipe registry is still empty.
///
/// The index is built once and cached. A pre-registry call is not cached so a
/// caller cannot freeze an empty index before `init_vanilla_registry`; that
/// runtime guard is why this uses `OnceLock` instead of `LazyLock`.
#[must_use]
fn display_index() -> Option<&'static DisplayIndex> {
    if let Some(index) = DISPLAY_INDEX.get() {
        return Some(index);
    }
    if REGISTRY.recipes.is_empty() {
        return None;
    }
    Some(DISPLAY_INDEX.get_or_init(build_display_index))
}

/// Vanilla `RecipeManager.unpackRecipeInfo`.
fn build_display_index() -> DisplayIndex {
    let mut entries: Vec<RecipeDisplayEntry> = Vec::new();
    let mut owners: Vec<Identifier> = Vec::new();
    let mut by_recipe: FxHashMap<Identifier, Vec<usize>> = FxHashMap::default();
    let mut groups: FxHashMap<String, i32> = FxHashMap::default();

    for recipe in REGISTRY.recipes.iter() {
        let Some(properties) = recipe.data().properties() else {
            continue;
        };
        let Some(book) = properties.recipe_book.as_ref() else {
            // Only special recipes lack book metadata; vanilla gives them no display.
            continue;
        };
        let Some(display) = recipe_display(recipe) else {
            continue;
        };

        let group = book.group.as_ref().map(|group| {
            let next = i32::try_from(groups.len()).unwrap_or(i32::MAX);
            *groups.entry(group.clone()).or_insert(next)
        });
        let category = recipe_book_category(book.category, recipe);
        let crafting_requirements = properties.placement.as_ref().map(|placement| {
            placement
                .ingredients
                .iter()
                .filter_map(ingredient_holder_set)
                .collect::<Vec<_>>()
        });

        let display_id = i32::try_from(entries.len()).unwrap_or(i32::MAX);
        let index = entries.len();
        entries.push(RecipeDisplayEntry {
            id: RecipeDisplayId { index: display_id },
            display,
            group,
            category,
            crafting_requirements,
        });
        owners.push(recipe.key().clone());
        by_recipe
            .entry(recipe.key().clone())
            .or_default()
            .push(index);
    }

    DisplayIndex {
        entries,
        owners,
        by_recipe,
    }
}

/// Vanilla `Recipe.recipeBookCategory()` for the steel recipe types.
fn recipe_book_category(
    kind: RecipeBookCategoryKind,
    recipe: UntypedRecipeRef,
) -> RecipeBookCategoryRef {
    let type_key = &recipe.recipe_type().key;
    let is_smelting = type_key == vanilla_recipe_types::SMELTING.key();
    let is_blasting = type_key == vanilla_recipe_types::BLASTING.key();
    let is_smoking = type_key == vanilla_recipe_types::SMOKING.key();
    let is_campfire = type_key == vanilla_recipe_types::CAMPFIRE_COOKING.key();

    match kind {
        RecipeBookCategoryKind::Crafting(category) => match category {
            CraftingBookCategory::Building => &categories::CRAFTING_BUILDING_BLOCKS,
            CraftingBookCategory::Redstone => &categories::CRAFTING_REDSTONE,
            CraftingBookCategory::Equipment => &categories::CRAFTING_EQUIPMENT,
            CraftingBookCategory::Misc => &categories::CRAFTING_MISC,
        },
        RecipeBookCategoryKind::Cooking(category) => {
            // Vanilla maps each cooking category per cooking type: smoking and
            // campfire always report their own book, blasting folds food into misc.
            if is_smoking {
                return &categories::SMOKER_FOOD;
            }
            if is_campfire {
                return &categories::CAMPFIRE;
            }
            match (category, is_blasting, is_smelting) {
                (CookingBookCategory::Blocks, true, _) => &categories::BLAST_FURNACE_BLOCKS,
                (CookingBookCategory::Food, false, _) => &categories::FURNACE_FOOD,
                (CookingBookCategory::Food | CookingBookCategory::Misc, true, _) => {
                    &categories::BLAST_FURNACE_MISC
                }
                (CookingBookCategory::Misc, false, _) => &categories::FURNACE_MISC,
                (CookingBookCategory::Blocks, false, _) => &categories::FURNACE_BLOCKS,
            }
        }
        RecipeBookCategoryKind::Stonecutter => &categories::STONECUTTER,
        RecipeBookCategoryKind::Smithing => &categories::SMITHING,
    }
}

/// Vanilla `Recipe.display()`. Every vanilla serializer yields at most one display.
fn recipe_display(recipe: UntypedRecipeRef) -> Option<RecipeDisplay> {
    let type_key = &recipe.recipe_type().key;

    if type_key == vanilla_recipe_types::CRAFTING.key() {
        return match recipe.downcast_data::<CraftingRecipe>()? {
            CraftingRecipe::Shaped(shaped) => Some(shaped_display(shaped)),
            CraftingRecipe::Shapeless(shapeless) => Some(shapeless_display(shapeless)),
            // Vanilla's remaining crafting serializers extend `CustomRecipe`, whose
            // default `display()` is empty; they are all special recipes.
            _ => None,
        };
    }
    if type_key == vanilla_recipe_types::SMELTING.key() {
        return cooking_display(recipe, &vanilla_items::FURNACE);
    }
    if type_key == vanilla_recipe_types::BLASTING.key() {
        return cooking_display(recipe, &vanilla_items::BLAST_FURNACE);
    }
    if type_key == vanilla_recipe_types::SMOKING.key() {
        return cooking_display(recipe, &vanilla_items::SMOKER);
    }
    if type_key == vanilla_recipe_types::CAMPFIRE_COOKING.key() {
        return cooking_display(recipe, &vanilla_items::CAMPFIRE);
    }
    if type_key == vanilla_recipe_types::STONECUTTING.key() {
        let stonecutting = recipe.downcast_data::<StonecuttingRecipe>()?;
        return Some(RecipeDisplay::Stonecutter {
            input: ingredient_display(&stonecutting.ingredient),
            result: stack_display(&stonecutting.result),
            crafting_station: station_display(&vanilla_items::STONECUTTER),
        });
    }
    if type_key == vanilla_recipe_types::SMITHING.key() {
        return smithing_display(recipe.downcast_data::<SmithingRecipe>()?);
    }
    // Brewing recipes are special and carry no display, matching vanilla.
    None
}

/// Vanilla `ShapedCraftingRecipeDisplay`.
fn shaped_display(recipe: &ShapedRecipe) -> RecipeDisplay {
    RecipeDisplay::ShapedCrafting {
        width: i32::try_from(recipe.width).unwrap_or(i32::MAX),
        height: i32::try_from(recipe.height).unwrap_or(i32::MAX),
        ingredients: recipe
            .pattern
            .iter()
            .map(|ingredient| {
                if ingredient.is_empty() {
                    SlotDisplay::Empty
                } else {
                    ingredient_display(ingredient)
                }
            })
            .collect(),
        result: stack_display(&recipe.result),
        crafting_station: station_display(&vanilla_items::CRAFTING_TABLE),
    }
}

/// Vanilla `ShapelessCraftingRecipeDisplay`.
fn shapeless_display(recipe: &ShapelessRecipe) -> RecipeDisplay {
    RecipeDisplay::ShapelessCrafting {
        ingredients: recipe.ingredients.iter().map(ingredient_display).collect(),
        result: stack_display(&recipe.result),
        crafting_station: station_display(&vanilla_items::CRAFTING_TABLE),
    }
}

/// Vanilla `AbstractCookingRecipe.display()` with the type's furnace icon.
fn cooking_display(
    recipe: UntypedRecipeRef,
    furnace_icon: &'static steel_registry::items::Item,
) -> Option<RecipeDisplay> {
    let cooking = recipe.downcast_data::<CookingRecipe>()?;
    Some(RecipeDisplay::Furnace {
        ingredient: ingredient_display(&cooking.ingredient),
        fuel: SlotDisplay::AnyFuel,
        result: stack_display(&cooking.result),
        crafting_station: station_display(furnace_icon),
        duration: cooking.cooking_time,
        experience: cooking.experience,
    })
}

/// Vanilla `SmithingTransformRecipe.display()`/`SmithingTrimRecipe.display()`.
fn smithing_display(recipe: &SmithingRecipe) -> Option<RecipeDisplay> {
    match recipe {
        SmithingRecipe::Transform(transform) => Some(RecipeDisplay::Smithing {
            template: optional_ingredient_display(transform.template.as_ref()),
            base: ingredient_display(&transform.base),
            addition: optional_ingredient_display(transform.addition.as_ref()),
            result: stack_display(&transform.result),
            crafting_station: station_display(&vanilla_items::SMITHING_TABLE),
        }),
        SmithingRecipe::Trim(trim) => {
            let base = ingredient_display(&trim.base);
            let material = ingredient_display(&trim.addition);
            let result = SlotDisplay::SmithingTrim {
                base: Box::new(base.clone()),
                material: Box::new(material.clone()),
                pattern: steel_protocol::packets::game::TrimPatternHolder(
                    steel_registry::registry::holder::RegistryHolder::Reference(trim.pattern),
                ),
            };
            Some(RecipeDisplay::Smithing {
                template: ingredient_display(&trim.template),
                base,
                addition: material,
                result,
                crafting_station: station_display(&vanilla_items::SMITHING_TABLE),
            })
        }
    }
}

/// Vanilla `Ingredient.display()`: a tag display over the ingredient's items.
fn ingredient_display(ingredient: &Ingredient) -> SlotDisplay {
    match ingredient_holder_set(ingredient) {
        Some(items) => SlotDisplay::Tag(items),
        None => SlotDisplay::Empty,
    }
}

/// Vanilla `Ingredient.optionalIngredientToDisplay`.
fn optional_ingredient_display(ingredient: Option<&Ingredient>) -> SlotDisplay {
    ingredient.map_or(SlotDisplay::Empty, ingredient_display)
}

/// Vanilla `Ingredient.CONTENTS_STREAM_CODEC`.
fn ingredient_holder_set(ingredient: &Ingredient) -> Option<ItemHolderSet> {
    match ingredient {
        Ingredient::Empty => None,
        Ingredient::Item(item) => Some(ItemHolderSet::Direct(vec![*item])),
        Ingredient::Tag(tag) => Some(ItemHolderSet::Tag(tag.clone())),
        Ingredient::Choice(items) => Some(ItemHolderSet::Direct(items.to_vec())),
    }
}

/// Vanilla `SlotDisplay.ItemStackSlotDisplay`.
fn stack_display(stack: &steel_registry::item_stack_template::ItemStackTemplate) -> SlotDisplay {
    SlotDisplay::ItemStack(stack.clone())
}

/// Vanilla `SlotDisplay.ItemSlotDisplay` for a crafting station.
fn station_display(item: &'static steel_registry::items::Item) -> SlotDisplay {
    SlotDisplay::Item(ItemHolder(item))
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::init_vanilla_registry;

    #[test]
    fn display_index_covers_every_learnable_recipe() {
        init_vanilla_registry();
        let index = display_index().expect("registry is initialized");
        for recipe in REGISTRY.recipes.iter() {
            if is_special(recipe) {
                continue;
            }
            let ids = index
                .by_recipe
                .get(recipe.key())
                .unwrap_or_else(|| panic!("recipe {} has no display", recipe.key()));
            assert!(!ids.is_empty(), "recipe {} has no display", recipe.key());
            for &id in ids {
                assert_eq!(
                    index.owners[id],
                    *recipe.key(),
                    "display {id} is owned by another recipe"
                );
            }
        }
    }

    #[test]
    fn shaped_display_matches_the_recipe_pattern() {
        init_vanilla_registry();
        let recipe = REGISTRY
            .recipes
            .by_key(&Identifier::vanilla_static("iron_pickaxe"))
            .expect("iron pickaxe is registered");
        let Some(RecipeDisplay::ShapedCrafting {
            width,
            height,
            ingredients,
            crafting_station,
            ..
        }) = recipe_display(recipe)
        else {
            panic!("iron pickaxe must resolve to a shaped crafting display");
        };
        assert_eq!((width, height), (3, 3));
        assert_eq!(ingredients.len(), 9);
        assert!(matches!(crafting_station, SlotDisplay::Item(_)));
    }

    #[test]
    fn book_add_recipes_reports_entries_and_unlocks() {
        init_vanilla_registry();
        let recipe = REGISTRY
            .recipes
            .by_key(&Identifier::vanilla_static("iron_pickaxe"))
            .expect("iron pickaxe is registered");

        let mut book = ServerRecipeBook::new();
        let (entries, unlocked) = book.add_recipes(&[recipe], &REGISTRY_DISPLAY_RESOLVER);
        assert_eq!(entries.len(), 1);
        assert_eq!(unlocked.len(), 1);
        assert!(entries[0].notification());
        assert!(entries[0].highlight());
        assert!(book.contains(recipe.key()));

        let (again, unlocked) = book.add_recipes(&[recipe], &REGISTRY_DISPLAY_RESOLVER);
        assert!(again.is_empty());
        assert!(unlocked.is_empty());

        let removed = book.remove_recipes(&[recipe], &REGISTRY_DISPLAY_RESOLVER);
        assert_eq!(removed, vec![entries[0].contents.id]);
        assert!(!book.contains(recipe.key()));
    }

    #[test]
    fn packed_book_round_trips() {
        init_vanilla_registry();
        let recipe = REGISTRY
            .recipes
            .by_key(&Identifier::vanilla_static("iron_pickaxe"))
            .expect("iron pickaxe is registered");

        let mut book = ServerRecipeBook::new();
        book.add_recipes(&[recipe], &REGISTRY_DISPLAY_RESOLVER);
        book.set_book_setting(RecipeBookType::Crafting, true, true);

        let packed = book.pack();
        let mut restored = ServerRecipeBook::new();
        restored.apply(packed);
        assert_eq!(restored.known(), book.known());
        assert_eq!(restored.settings(), book.settings());
        assert!(restored.contains(recipe.key()));
    }
}
