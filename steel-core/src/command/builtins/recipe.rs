//! Vanilla `/recipe` command.
//!
//! Mirrors `net.minecraft.server.commands.RecipeCommand`: `give` unlocks
//! recipes for players and `take` forgets them, both accepting either one
//! recipe id or `*` for every learnable recipe (every non-special recipe, as
//! in vanilla `RecipeManager.learnableRecipes`).

use std::fmt;

use steel_protocol::packets::game::{
    ArgumentType as ProtocolArgumentType, SuggestionType as ProtocolSuggestionType,
};
use steel_registry::{REGISTRY, recipe::UntypedRecipeRef};
use steel_utils::{DowncastType, DowncastTypeKey, Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{
        CommandNodeBuilder, CommandSyntaxError, CommandSyntaxErrorKind, StringReader,
        SuggestionsBuilder,
    },
    execution::{
        CommandArgumentSource, CommandSource, SteelArgumentParser, SteelArgumentSuggestionContext,
        SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument, literal,
    },
    registration::CommandRegistration,
};
use crate::{
    command::incorrectly_typed_argument, entity::Entity as _, player::Player,
    player::recipe_book::is_special,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("recipe"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("recipe")
        .then(
            literal("give").then(
                argument("targets", SteelArgumentType::players())
                    .then(
                        argument("recipe", recipe_argument())
                            .executes(|context| give_recipes(context, false)),
                    )
                    .then(literal("*").executes(|context| give_recipes(context, true))),
            ),
        )
        .then(
            literal("take").then(
                argument("targets", SteelArgumentType::players())
                    .then(
                        argument("recipe", recipe_argument())
                            .executes(|context| take_recipes(context, false)),
                    )
                    .then(literal("*").executes(|context| take_recipes(context, true))),
            ),
        )
}

/// Vanilla `giveRecipes`.
fn give_recipes(
    context: &SteelCommandContext<CommandSource>,
    all: bool,
) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    let recipes = selected_recipes(context, all)?;

    let mut tracker = PlayerTracker::new();
    for player in &players {
        tracker.track(player.as_ref(), player.award_recipes(&recipes));
    }
    finish(context, &tracker, RecipeAction::Give)
}

/// Vanilla `takeRecipes`.
fn take_recipes(
    context: &SteelCommandContext<CommandSource>,
    all: bool,
) -> Result<i32, CommandSyntaxError> {
    let players = context.players("targets")?;
    let recipes = selected_recipes(context, all)?;

    let mut tracker = PlayerTracker::new();
    for player in &players {
        tracker.track(player.as_ref(), player.reset_recipes(&recipes));
    }
    finish(context, &tracker, RecipeAction::Take)
}

/// Which recipe book operation the response belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecipeAction {
    /// `recipe give`, reported with the `commands.recipe.give.*` messages.
    Give,
    /// `recipe take`, reported with the `commands.recipe.take.*` messages.
    Take,
}

/// Vanilla `CommandResponseTracker<ServerPlayer>` restricted to the `NON_ZERO`
/// element set, which is the only set `/recipe` dispatches.
struct PlayerTracker<'a> {
    total_value: i32,
    only_non_zero_element: Option<&'a Player>,
    non_zero_element_count: i32,
}

impl<'a> PlayerTracker<'a> {
    const fn new() -> Self {
        Self {
            total_value: 0,
            only_non_zero_element: None,
            non_zero_element_count: 0,
        }
    }

    fn track(&mut self, player: &'a Player, value: i32) {
        self.total_value += value;
        if value != 0 {
            self.non_zero_element_count += 1;
            self.only_non_zero_element = if self.non_zero_element_count == 1 {
                Some(player)
            } else {
                None
            };
        }
    }
}

/// Vanilla `CommandResponseTracker.sendFeedback` for `/recipe`.
fn finish(
    context: &SteelCommandContext<CommandSource>,
    tracker: &PlayerTracker<'_>,
    action: RecipeAction,
) -> Result<i32, CommandSyntaxError> {
    if tracker.non_zero_element_count == 0 {
        let failed: &text_components::translation::Translation<0> = match action {
            RecipeAction::Give => &translations::COMMANDS_RECIPE_GIVE_FAILED,
            RecipeAction::Take => &translations::COMMANDS_RECIPE_TAKE_FAILED,
        };
        return Err(CommandSyntaxError::dynamic(failed.msg().component()));
    }

    let total = TextComponent::from(tracker.total_value.to_string());
    let message = match (action, tracker.only_non_zero_element) {
        (RecipeAction::Give, Some(player)) => translations::COMMANDS_RECIPE_GIVE_SUCCESS_SINGLE
            .message([total, player.display_name()])
            .component(),
        (RecipeAction::Give, None) => translations::COMMANDS_RECIPE_GIVE_SUCCESS_MULTIPLE
            .message([
                total,
                TextComponent::from(tracker.non_zero_element_count.to_string()),
            ])
            .component(),
        (RecipeAction::Take, Some(player)) => translations::COMMANDS_RECIPE_TAKE_SUCCESS_SINGLE
            .message([total, player.display_name()])
            .component(),
        (RecipeAction::Take, None) => translations::COMMANDS_RECIPE_TAKE_SUCCESS_MULTIPLE
            .message([
                total,
                TextComponent::from(tracker.non_zero_element_count.to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value)
}

/// Vanilla `ResourceKeyArgument.getRecipe` for one id, or
/// `RecipeManager.getLearnableRecipes` for `*`.
fn selected_recipes(
    context: &SteelCommandContext<CommandSource>,
    all: bool,
) -> Result<Vec<UntypedRecipeRef>, CommandSyntaxError> {
    if !all {
        let recipe = recipe_value(context)?;
        return Ok(vec![recipe]);
    }
    Ok(learnable_recipes())
}

/// Every non-special recipe, matching vanilla `RecipeManager.learnableRecipes`.
fn learnable_recipes() -> Vec<UntypedRecipeRef> {
    REGISTRY
        .recipes
        .iter()
        .filter(|recipe| !is_special(*recipe))
        .collect()
}

fn recipe_value(
    context: &SteelCommandContext<CommandSource>,
) -> Result<UntypedRecipeRef, CommandSyntaxError> {
    context
        .argument("recipe")?
        .downcast_ref::<RecipeValue>()
        .map(|value| value.0)
        .ok_or_else(|| incorrectly_typed_argument("recipe"))
}

fn recipe_argument() -> SteelArgumentType {
    SteelArgumentType::new(RecipeParser)
}

/// A resolved `minecraft:resource_key` recipe argument.
#[derive(Clone, Copy)]
struct RecipeValue(UntypedRecipeRef);

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for RecipeValue {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/value/recipe");
}

impl fmt::Debug for RecipeValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RecipeValue")
            .field(self.0.key())
            .finish()
    }
}

/// Vanilla `ResourceKeyArgument.key(Registries.RECIPE, LEARNABLE_RECIPE)`.
///
/// Unknown ids and special recipes both fail with `recipe.notFound`, matching
/// `ResourceKeyArgument.getRecipe`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RecipeParser;

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for RecipeParser {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/parser/recipe");
}

impl SteelArgumentParser for RecipeParser {
    type Value = RecipeValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        let raw = read_identifier(reader).to_owned();
        let key: Identifier = raw.parse().map_err(|_| not_found(reader, &raw))?;
        REGISTRY
            .recipes
            .by_key(&key)
            .filter(|recipe| !is_special(*recipe))
            .map_or_else(
                || Err(not_found(reader, &raw)),
                |recipe| Ok(RecipeValue(recipe)),
            )
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        let prefix = builder.remaining().to_owned();
        for recipe in REGISTRY.recipes.iter() {
            let key = recipe.key().to_string();
            if key.starts_with(&prefix) && !is_special(recipe) {
                builder.suggest(key);
            }
        }
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::ResourceKey {
                identifier: RECIPE_REGISTRY,
            },
            Some(ProtocolSuggestionType::AskServer),
        )
    }
}

/// The registry feeding the `minecraft:resource_key` recipe argument.
const RECIPE_REGISTRY: &str = "minecraft:recipe";

fn not_found(reader: &StringReader<'_>, key: &str) -> CommandSyntaxError {
    reader.error(CommandSyntaxErrorKind::Dynamic(Box::new(
        translations::RECIPE_NOT_FOUND
            .message([TextComponent::plain(key.to_owned())])
            .component(),
    )))
}

/// Reads a namespaced identifier, matching vanilla `Identifier.read`.
fn read_identifier<'input>(reader: &mut StringReader<'input>) -> &'input str {
    let start = reader.read_so_far().len();
    while reader.peek().is_some_and(is_allowed_in_identifier) {
        reader.skip();
    }
    &reader.read_so_far()[start..]
}

const fn is_allowed_in_identifier(character: char) -> bool {
    character.is_ascii_digit()
        || character.is_ascii_lowercase()
        || matches!(character, '_' | ':' | '/' | '.' | '-')
}
