//! Vanilla per-server stopwatch command.
//!
//! Vanilla `MinecraftServer.getStopwatches` stores a process-local map of named
//! stopwatches. Steel mirrors that with a crate-local registry keyed on
//! [`Identifier`]; every create/query/restart/remove branch mutates real state
//! and matches vanilla's parse tree, messages and result values.

use std::borrow::Cow;
use std::sync::LazyLock;

use steel_utils::locks::SyncMutex;
use steel_utils::{Identifier, translations};
use text_components::{TextComponent, translation::TranslatedMessage};

use super::super::{
    brigadier::{
        ArgumentSuggestionContext, ArgumentType, CommandNodeBuilder, CommandSyntaxError,
        SuggestionsBuilder,
    },
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};

/// A named Steel stopwatch, vanilla `world.Stopwatch` equivalent.
#[derive(Debug, Clone, Copy)]
struct Stopwatch {
    /// Server tick when the stopwatch was created or last restarted.
    start_tick: u64,
}

/// Vanilla `MinecraftServer.getStopwatches` equivalent.
static STOPWATCHES: LazyLock<SyncMutex<Vec<(Identifier, Stopwatch)>>> =
    LazyLock::new(|| SyncMutex::new(Vec::new()));

/// Vanilla `Component.translationArg(id)`: the identifier itself is the key.
fn translation_arg(id: &Identifier) -> TextComponent {
    TextComponent::translated(TranslatedMessage {
        key: Cow::Owned(id.to_string()),
        args: None,
        fallback: None,
    })
}

/// Vanilla `StopwatchCommand.SUGGEST_STOPWATCHES`.
pub(super) fn suggest_stopwatches(builder: &mut SuggestionsBuilder<'_>) {
    let prefix = builder.remaining().to_owned();
    let ids: Vec<Identifier> = STOPWATCHES
        .lock()
        .iter()
        .map(|(id, _)| id.clone())
        .collect();
    for id in ids {
        let id = id.to_string();
        if id.starts_with(&prefix) {
            builder.suggest(id);
        }
    }
}

/// Vanilla `Stopwatch.elapsedSeconds`: ticks since start divided by 20.
///
/// `None` is vanilla's `StopwatchCommand.ERROR_DOES_NOT_EXIST` branch.
pub(super) fn elapsed_seconds(tick: u64, id: &Identifier) -> Option<f64> {
    let stopwatches = STOPWATCHES.lock();
    stopwatches
        .iter()
        .find_map(|(existing, stopwatch)| (existing == id).then_some(*stopwatch))
        .map(|stopwatch| (tick.saturating_sub(stopwatch.start_tick) as f64) / 20.0)
}

/// Vanilla `StopwatchCommand.ERROR_DOES_NOT_EXIST`.
pub(super) fn does_not_exist(id: &Identifier) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_STOPWATCH_DOES_NOT_EXIST
            .message([translation_arg(id)])
            .component(),
    )
}

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("stopwatch"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    let suggestions = |_: &ArgumentSuggestionContext<'_, _, _>,
                       builder: &mut SuggestionsBuilder<'_>| {
        suggest_stopwatches(builder);
    };

    literal("stopwatch")
        .then(literal("create").then(
            argument("id", SteelArgumentType::identifier_argument()).executes(create_stopwatch),
        ))
        .then(
            literal("query").then(
                argument("id", SteelArgumentType::identifier_argument())
                    .suggests(suggestions)
                    .then(
                        argument("scale", ArgumentType::double(f64::MIN, f64::MAX))
                            .executes(query_stopwatch),
                    )
                    .executes(query_stopwatch),
            ),
        )
        .then(
            literal("restart").then(
                argument("id", SteelArgumentType::identifier_argument())
                    .suggests(suggestions)
                    .executes(restart_stopwatch),
            ),
        )
        .then(
            literal("remove").then(
                argument("id", SteelArgumentType::identifier_argument())
                    .suggests(suggestions)
                    .executes(remove_stopwatch),
            ),
        )
}

/// Vanilla `StopwatchCommand.createStopwatch`.
fn create_stopwatch(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?.clone();
    let tick = context.source().server().current_tick();
    let mut stopwatches = STOPWATCHES.lock();
    if stopwatches.iter().any(|(existing, _)| existing == &id) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_STOPWATCH_ALREADY_EXISTS
                .message([translation_arg(&id)])
                .component(),
        ));
    }
    stopwatches.push((id.clone(), Stopwatch { start_tick: tick }));
    drop(stopwatches);

    let message = translations::COMMANDS_STOPWATCH_CREATE_SUCCESS
        .message([translation_arg(&id)])
        .component();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Vanilla `StopwatchCommand.queryStopwatch`.
///
/// Vanilla's no-`scale` branch passes `1.0`, and the returned result is the
/// `(int)` cast of `elapsedSeconds * scale`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the command result is vanilla's (int) cast of the scaled elapsed seconds"
)]
fn query_stopwatch(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?.clone();
    let scale: f64 = context.double("scale").unwrap_or(1.0);
    let tick = context.source().server().current_tick();
    let stopwatch = {
        let stopwatches = STOPWATCHES.lock();
        stopwatches
            .iter()
            .find_map(|(existing, stopwatch)| (existing == &id).then_some(*stopwatch))
    };
    let Some(stopwatch) = stopwatch else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_STOPWATCH_DOES_NOT_EXIST
                .message([translation_arg(&id)])
                .component(),
        ));
    };

    let elapsed = (tick.saturating_sub(stopwatch.start_tick) as f64) / 20.0;
    let message = translations::COMMANDS_STOPWATCH_QUERY
        .message([
            translation_arg(&id),
            TextComponent::plain(format!("{elapsed:.2}")),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok((elapsed * scale) as i32)
}

/// Vanilla `StopwatchCommand.restartStopwatch`.
fn restart_stopwatch(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?.clone();
    let tick = context.source().server().current_tick();
    let restarted = {
        let mut stopwatches = STOPWATCHES.lock();
        stopwatches
            .iter_mut()
            .find_map(|(existing, stopwatch)| (existing == &id).then_some(stopwatch))
            .map(|stopwatch| stopwatch.start_tick = tick)
            .is_some()
    };
    if !restarted {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_STOPWATCH_DOES_NOT_EXIST
                .message([translation_arg(&id)])
                .component(),
        ));
    }

    let message = translations::COMMANDS_STOPWATCH_RESTART_SUCCESS
        .message([translation_arg(&id)])
        .component();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Vanilla `StopwatchCommand.removeStopwatch`.
fn remove_stopwatch(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?.clone();
    let removed = {
        let mut stopwatches = STOPWATCHES.lock();
        let index = stopwatches.iter().position(|(existing, _)| existing == &id);
        index.map(|index| stopwatches.remove(index)).is_some()
    };
    if !removed {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_STOPWATCH_DOES_NOT_EXIST
                .message([translation_arg(&id)])
                .component(),
        ));
    }

    let message = translations::COMMANDS_STOPWATCH_REMOVE_SUCCESS
        .message([translation_arg(&id)])
        .component();
    context.source().send_success(&message, true);
    Ok(1)
}
