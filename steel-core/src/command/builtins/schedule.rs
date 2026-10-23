//! Vanilla `/schedule` command.
//!
//! Mirrors `net.minecraft.server.commands.ScheduleCommand`: `function <name>
//! <time> [append|replace]` queues a function or `#tag` on the overworld's game
//! time, and `clear <function>` drops every queued entry for one function.
//!
//! The queue is drained once per game tick by [`crate::datapack::tick`], exactly
//! like vanilla's `TimerQueue` in `ServerLevel.tickTime`.

use steel_utils::{
    Identifier,
    translations::{
        COMMANDS_SCHEDULE_CLEARED_FAILURE, COMMANDS_SCHEDULE_CLEARED_SUCCESS,
        COMMANDS_SCHEDULE_CREATED_FUNCTION, COMMANDS_SCHEDULE_CREATED_TAG, COMMANDS_SCHEDULE_MACRO,
        COMMANDS_SCHEDULE_SAME_TICK,
    },
};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        SteelSuggestionContext, argument, literal,
    },
    registration::CommandRegistration,
};
use super::function::{function_argument, function_value};
use crate::{
    datapack::{self, ScheduleMode, ScheduledCallback},
    permission::PermissionLevel,
};

/// Vanilla `ScheduleCommand.register`: `LEVEL_GAMEMASTERS`.
pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("schedule"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("schedule")
        .then(
            literal("function").then(
                argument("function", function_argument()).then(
                    argument("time", SteelArgumentType::time(0))
                        .executes(|context| schedule(context, ScheduleMode::Replace))
                        .then(
                            literal("append")
                                .executes(|context| schedule(context, ScheduleMode::Append)),
                        )
                        .then(
                            literal("replace")
                                .executes(|context| schedule(context, ScheduleMode::Replace)),
                        ),
                ),
            ),
        )
        .then(
            literal("clear").then(
                argument("function", SteelArgumentType::identifier_argument())
                    .suggests(suggest_scheduled)
                    .executes(clear),
            ),
        )
}

/// Vanilla `ScheduleCommand.schedule`.
fn schedule(
    context: &SteelCommandContext<CommandSource>,
    mode: ScheduleMode,
) -> Result<i32, CommandSyntaxError> {
    let time = context.time("time")?;
    if time == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &COMMANDS_SCHEDULE_SAME_TICK,
        )));
    }

    let target = function_value(context)?;
    let source = context.source();
    let tick = source.world().game_time() + i64::from(time);
    let callback = if target.tag {
        ScheduledCallback::Tag(target.id.clone())
    } else {
        // Vanilla `ScheduleCommand.schedule` rejects a macro function.
        let is_macro = datapack::try_datapacks().is_some_and(|manager| {
            manager
                .functions()
                .read()
                .get(&target.id)
                .is_some_and(|function| !function.macro_names.is_empty())
        });
        if is_macro {
            return Err(CommandSyntaxError::dynamic(TextComponent::from(
                &COMMANDS_SCHEDULE_MACRO,
            )));
        }
        ScheduledCallback::Function(target.id.clone())
    };

    let Some(manager) = datapack::try_datapacks() else {
        return Err(no_datapacks());
    };
    manager.schedules().write().schedule(callback, tick, mode);

    let id = TextComponent::plain(target.id.to_string());
    let ticks = TextComponent::plain(time.to_string());
    let at = TextComponent::plain(tick.to_string());
    let message = if target.tag {
        COMMANDS_SCHEDULE_CREATED_TAG
            .message([id, ticks, at])
            .component()
    } else {
        COMMANDS_SCHEDULE_CREATED_FUNCTION
            .message([id, ticks, at])
            .component()
    };
    source.send_success(&message, true);

    // Vanilla `Math.floorMod(tickTime, Integer.MAX_VALUE)`.
    Ok(tick.rem_euclid(i64::from(i32::MAX)) as i32)
}

/// Vanilla `ScheduleCommand.remove`.
fn clear(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("function")?.clone();
    let Some(manager) = datapack::try_datapacks() else {
        return Err(no_datapacks());
    };
    let count = manager.schedules().write().clear(&id);
    if count == 0 {
        return Err(CommandSyntaxError::dynamic(
            COMMANDS_SCHEDULE_CLEARED_FAILURE
                .message([TextComponent::plain(id.to_string())])
                .component(),
        ));
    }

    context.source().send_success(
        &COMMANDS_SCHEDULE_CLEARED_SUCCESS
            .message([
                TextComponent::plain(count.to_string()),
                TextComponent::plain(id.to_string()),
            ])
            .component(),
        true,
    );
    Ok(i32::try_from(count).unwrap_or(i32::MAX))
}

/// Vanilla `ScheduleCommand.SUGGEST_SCHEDULE`: ids with a pending entry.
fn suggest_scheduled(
    _context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let Some(manager) = datapack::try_datapacks() else {
        return;
    };
    let prefix = builder.remaining().to_owned();
    for id in manager.schedules().read().pending_ids() {
        // Vanilla's `IdentifierArgument` never matches a `#tag` key, so only
        // function ids are suggested.
        if !id.starts_with('#') && id.starts_with(&prefix) {
            builder.suggest(id);
        }
    }
}

fn no_datapacks() -> CommandSyntaxError {
    CommandSyntaxError::dynamic("Data packs are not loaded yet")
}
