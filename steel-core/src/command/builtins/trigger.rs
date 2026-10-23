//! Vanilla `trigger` command.
//!
//! Mirrors `TriggerCommand`, backed by [`crate::scoreboard::Scoreboard`].

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        SteelSuggestionContext, argument, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use super::scoreboard::{is_trigger, scoreboard_error, source_scoreboard, source_scoreboard_for};
use crate::scoreboard::{ScoreHolder, ScoreboardObjective};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("trigger"), |_| command()).default_access()
}

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

fn command() -> Builder {
    literal("trigger").then(
        argument("objective", SteelArgumentType::objective())
            .suggests(suggest_objectives)
            .executes(simple_trigger)
            .then(literal("add").then(
                argument("value", ArgumentType::integer(i32::MIN, i32::MAX)).executes(add_value),
            ))
            .then(literal("set").then(
                argument("value", ArgumentType::integer(i32::MIN, i32::MAX)).executes(set_value),
            )),
    )
}

fn add_value(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let value = context.integer("value")?;
    let (holder, objective) = primed_score(context)?;
    let scoreboard = source_scoreboard(context)?;
    let current = scoreboard
        .get_or_create_score(&holder, &objective, false)
        .map_err(scoreboard_error)?;
    let total = current.saturating_add(value);
    scoreboard
        .set_score(&holder, &objective, total)
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_TRIGGER_ADD_SUCCESS
        .message([
            objective.formatted_display_name(),
            TextComponent::from(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(total)
}

fn set_value(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let value = context.integer("value")?;
    let (holder, objective) = primed_score(context)?;
    let scoreboard = source_scoreboard(context)?;
    scoreboard
        .set_score(&holder, &objective, value)
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_TRIGGER_SET_SUCCESS
        .message([
            objective.formatted_display_name(),
            TextComponent::from(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(value)
}

fn simple_trigger(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let (holder, objective) = primed_score(context)?;
    let scoreboard = source_scoreboard(context)?;
    let current = scoreboard
        .get_or_create_score(&holder, &objective, false)
        .map_err(scoreboard_error)?;
    let total = current.saturating_add(1);
    scoreboard
        .set_score(&holder, &objective, total)
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_TRIGGER_SIMPLE_SUCCESS
        .message([objective.formatted_display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(total)
}

/// Vanilla `TriggerCommand.suggestObjectives`: primed trigger objectives only.
fn suggest_objectives(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let source = context.source();
    let Some(player) = source.player() else {
        return;
    };
    let Ok(scoreboard) = source_scoreboard_for(source) else {
        return;
    };

    let holder = ScoreHolder::new(player.gameprofile.name().to_owned());
    let available = scoreboard
        .objective_names()
        .into_iter()
        .filter(|name| {
            let Some(objective) = scoreboard.objective(name) else {
                return false;
            };
            is_trigger(&objective)
                && scoreboard
                    .score_entry(&holder, &objective)
                    .is_some_and(|score| !score.is_locked())
        })
        .collect::<Vec<_>>();
    suggest_list(builder, available);
}

/// Vanilla `TriggerCommand.getScore`: validates the objective and consumes the primer.
fn primed_score(
    context: &SteelCommandContext<CommandSource>,
) -> Result<(ScoreHolder, ScoreboardObjective), CommandSyntaxError> {
    let source = context.source();
    let player = source.player().cloned().ok_or_else(|| {
        CommandSyntaxError::dynamic(translations::PERMISSIONS_REQUIRES_PLAYER.msg().component())
    })?;
    let scoreboard = source_scoreboard(context)?;
    let objective_name = context.objective_name("objective")?;
    let objective = scoreboard.objective(objective_name).ok_or_else(|| {
        CommandSyntaxError::dynamic(
            translations::ARGUMENTS_OBJECTIVE_NOT_FOUND
                .message([TextComponent::from(objective_name.to_owned())])
                .component(),
        )
    })?;
    if !is_trigger(&objective) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TRIGGER_FAILED_INVALID
                .msg()
                .component(),
        ));
    }

    let holder = ScoreHolder::new(player.gameprofile.name().to_owned());
    let unlocked = scoreboard
        .score_entry(&holder, &objective)
        .is_some_and(|score| !score.is_locked());
    if !unlocked {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TRIGGER_FAILED_UNPRIMED
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_score_locked(&holder, &objective, true)
        .map_err(scoreboard_error)?;
    Ok((holder, objective))
}
