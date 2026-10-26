//! Vanilla `scoreboard` command.
//!
//! Mirrors `ScoreboardCommand`, backed by [`crate::scoreboard::Scoreboard`].

use steel_utils::{Identifier, translations};
use text_components::{Modifier, TextComponent, format::Color};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, ScoreHolderArgument, ScoreHolderWildcard, SteelArgumentType,
        SteelCommandContext, SteelCommandRuntime, SteelSuggestionContext, argument, literal,
        suggest_list,
    },
    registration::CommandRegistration,
};
use crate::scoreboard::{
    NumberFormat, ObjectiveCriteria, RenderType, ScoreHolder, Scoreboard, ScoreboardError,
    ScoreboardObjective,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("scoreboard"), |_| command())
}

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Vanilla `ScoreboardCommand.NumberFormatCommandExecutor`.
type NumberFormatExecutor = fn(
    &SteelCommandContext<CommandSource>,
    Option<NumberFormat>,
) -> Result<i32, CommandSyntaxError>;

fn command() -> Builder {
    literal("scoreboard")
        .then(objectives())
        .then(set_display())
        .then(players())
        .then(operation())
}

fn objectives() -> Builder {
    literal("objectives")
        .then(literal("list").executes(list_objectives))
        .then(
            literal("add").then(
                argument("objective", ArgumentType::word()).then(
                    argument("criteria", SteelArgumentType::objective_criteria())
                        .executes(add_objective_default_name)
                        .then(
                            argument("displayName", SteelArgumentType::component())
                                .executes(add_objective_display_name),
                        ),
                ),
            ),
        )
        .then(
            literal("modify").then(
                argument("objective", SteelArgumentType::objective())
                    .then(
                        literal("displayname").then(
                            argument("displayName", SteelArgumentType::component())
                                .executes(set_objective_display_name),
                        ),
                    )
                    .then(render_type())
                    .then(
                        literal("displayautoupdate").then(
                            argument("value", ArgumentType::bool())
                                .executes(set_objective_display_auto_update),
                        ),
                    )
                    .then(number_formats(
                        literal("numberformat"),
                        set_objective_number_format,
                    )),
            ),
        )
        .then(
            literal("remove").then(
                argument("objective", SteelArgumentType::objective()).executes(remove_objective),
            ),
        )
}

fn render_type() -> Builder {
    literal("rendertype")
        .then(literal("integer").executes(|context| set_render_type(context, RenderType::Integer)))
        .then(literal("hearts").executes(|context| set_render_type(context, RenderType::Hearts)))
}

fn set_display() -> Builder {
    literal("setdisplay").then(
        argument("slot", SteelArgumentType::scoreboard_slot())
            .executes(clear_display_slot)
            .then(argument("objective", SteelArgumentType::objective()).executes(set_display_slot)),
    )
}

fn players() -> Builder {
    literal("players")
        .then(
            literal("list").executes(list_tracked_players).then(
                argument("target", SteelArgumentType::score_holder())
                    .executes(list_tracked_player_scores),
            ),
        )
        .then(
            literal("set").then(
                argument("targets", SteelArgumentType::score_holders()).then(
                    argument("objective", SteelArgumentType::objective()).then(
                        argument("score", ArgumentType::integer(i32::MIN, i32::MAX))
                            .executes(set_score),
                    ),
                ),
            ),
        )
        .then(
            literal("get").then(
                argument("target", SteelArgumentType::score_holder()).then(
                    argument("objective", SteelArgumentType::objective()).executes(get_score),
                ),
            ),
        )
        .then(literal("add").then(
            argument("targets", SteelArgumentType::score_holders()).then(
                argument("objective", SteelArgumentType::objective()).then(
                    argument("score", ArgumentType::integer(0, i32::MAX)).executes(add_score),
                ),
            ),
        ))
        .then(literal("remove").then(
            argument("targets", SteelArgumentType::score_holders()).then(
                argument("objective", SteelArgumentType::objective()).then(
                    argument("score", ArgumentType::integer(0, i32::MAX)).executes(remove_score),
                ),
            ),
        ))
        .then(
            literal("reset").then(
                argument("targets", SteelArgumentType::score_holders())
                    .executes(reset_scores)
                    .then(
                        argument("objective", SteelArgumentType::objective()).executes(reset_score),
                    ),
            ),
        )
        .then(
            literal("enable").then(
                argument("targets", SteelArgumentType::score_holders()).then(
                    argument("objective", SteelArgumentType::objective())
                        .suggests(suggest_triggers)
                        .executes(enable_trigger),
                ),
            ),
        )
        .then(
            literal("display")
                .then(
                    literal("name").then(
                        argument("targets", SteelArgumentType::score_holders()).then(
                            argument("objective", SteelArgumentType::objective())
                                .then(
                                    argument("name", SteelArgumentType::component())
                                        .executes(set_score_display_name),
                                )
                                .executes(clear_score_display_name),
                        ),
                    ),
                )
                .then(literal("numberformat").then(
                    argument("targets", SteelArgumentType::score_holders()).then(number_formats(
                        argument("objective", SteelArgumentType::objective()),
                        set_score_number_format,
                    )),
                )),
        )
}

fn operation() -> Builder {
    literal("operation").then(
        argument("targets", SteelArgumentType::score_holders()).then(
            argument("targetObjective", SteelArgumentType::objective()).then(
                argument("operation", SteelArgumentType::operation()).then(
                    argument("source", SteelArgumentType::score_holders()).then(
                        argument("sourceObjective", SteelArgumentType::objective())
                            .executes(perform_operation),
                    ),
                ),
            ),
        ),
    )
}

/// Vanilla `ScoreboardCommand.addNumberFormats`.
fn number_formats(top: Builder, apply: NumberFormatExecutor) -> Builder {
    top.then(literal("blank").executes(move |context| apply(context, Some(NumberFormat::Blank))))
        .then(literal("fixed").then(
            argument("contents", SteelArgumentType::component()).executes(move |context| {
                let contents = context.text_component("contents")?.clone();
                apply(context, Some(NumberFormat::Fixed(contents)))
            }),
        ))
        .then(
            literal("styled").then(argument("style", SteelArgumentType::style()).executes(
                move |context| {
                    let style = context.style("style")?.clone();
                    apply(context, Some(NumberFormat::Styled(style)))
                },
            )),
        )
        .executes(move |context| apply(context, None))
}

/// Vanilla `ScoreboardCommand.suggestTriggers`.
fn suggest_triggers(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let Ok(argument) = context.argument("targets") else {
        return;
    };
    let Some(targets) = argument.downcast_ref::<ScoreHolderArgument>() else {
        return;
    };
    let Ok(targets) = targets.resolve(context.source(), ScoreHolderWildcard::Tracked) else {
        return;
    };
    let Ok(scoreboard) = source_scoreboard_for(context.source()) else {
        return;
    };

    let available = scoreboard
        .objective_names()
        .into_iter()
        .filter(|name| {
            let Some(objective) = scoreboard.objective(name) else {
                return false;
            };
            if !is_trigger(&objective) {
                return false;
            }
            targets.iter().any(|target| {
                scoreboard
                    .score_entry(target, &objective)
                    .is_none_or(|score| score.is_locked())
            })
        })
        .collect::<Vec<_>>();
    suggest_list(builder, available);
}

fn list_objectives(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let objectives = objective_snapshot(scoreboard);
    let source = context.source();
    if objectives.is_empty() {
        source.send_success(
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_LIST_EMPTY
                .msg()
                .component(),
            false,
        );
    } else {
        let listed = format_list(
            objectives
                .iter()
                .map(ScoreboardObjective::formatted_display_name),
        );
        let message = translations::COMMANDS_SCOREBOARD_OBJECTIVES_LIST_SUCCESS
            .message([TextComponent::from(objectives.len().to_string()), listed])
            .component();
        source.send_success(&message, false);
    }
    Ok(objectives.len() as i32)
}

fn add_objective_default_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let name = context.string("objective")?.to_owned();
    let display_name = TextComponent::plain(name.clone());
    add_objective(context, name, display_name)
}

fn add_objective_display_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let name = context.string("objective")?.to_owned();
    let display_name = context.text_component("displayName")?.clone();
    add_objective(context, name, display_name)
}

fn add_objective(
    context: &SteelCommandContext<CommandSource>,
    name: String,
    display_name: TextComponent,
) -> Result<i32, CommandSyntaxError> {
    let criteria = objective_criteria(context, "criteria")?;
    let scoreboard = source_scoreboard(context)?;
    if scoreboard.objective(&name).is_some() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCOREBOARD_OBJECTIVES_ADD_DUPLICATE
                .msg()
                .component(),
        ));
    }

    let render_type = criteria.render_type();
    scoreboard
        .add_objective_full(
            name.clone(),
            criteria,
            display_name,
            render_type,
            false,
            None,
        )
        .map_err(scoreboard_error)?;
    let objective = require_objective(scoreboard, &name)?;
    let message = translations::COMMANDS_SCOREBOARD_OBJECTIVES_ADD_SUCCESS
        .message([objective.formatted_display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(scoreboard.objective_names().len() as i32)
}

fn remove_objective(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    scoreboard
        .remove_objective(objective.name())
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_SCOREBOARD_OBJECTIVES_REMOVE_SUCCESS
        .message([objective.formatted_display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(scoreboard.objective_names().len() as i32)
}

fn set_objective_display_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    let display_name = context.text_component("displayName")?.clone();
    if objective.display_name() != &display_name {
        scoreboard
            .set_objective_display_name(objective.name(), display_name)
            .map_err(scoreboard_error)?;
        let updated = require_objective(scoreboard, objective.name())?;
        let message = translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAYNAME
            .message([
                TextComponent::from(updated.name().to_owned()),
                updated.formatted_display_name(),
            ])
            .component();
        context.source().send_success(&message, true);
    }
    Ok(0)
}

fn set_render_type(
    context: &SteelCommandContext<CommandSource>,
    render_type: RenderType,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    if objective.render_type() != render_type {
        scoreboard
            .set_objective_render_type(objective.name(), render_type)
            .map_err(scoreboard_error)?;
        let message = translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_RENDERTYPE
            .message([objective.formatted_display_name()])
            .component();
        context.source().send_success(&message, true);
    }
    Ok(0)
}

fn set_objective_display_auto_update(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    let value = context.boolean("value")?;
    if objective.display_auto_update() != value {
        scoreboard
            .set_objective_display_auto_update(objective.name(), value)
            .map_err(scoreboard_error)?;
        // Vanilla passes an unused objective name first and the formatted name
        // second; the fallback only renders the formatted name.
        let key = if value {
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAY_AUTO_UPDATE_ENABLE
        } else {
            &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_DISPLAY_AUTO_UPDATE_DISABLE
        };
        let message = key
            .message([objective.formatted_display_name()])
            .component();
        context.source().send_success(&message, true);
    }
    Ok(0)
}

fn set_objective_number_format(
    context: &SteelCommandContext<CommandSource>,
    number_format: Option<NumberFormat>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    let is_set = number_format.is_some();
    scoreboard
        .set_objective_number_format(objective.name(), number_format)
        .map_err(scoreboard_error)?;
    let key = if is_set {
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_OBJECTIVE_FORMAT_SET
    } else {
        &translations::COMMANDS_SCOREBOARD_OBJECTIVES_MODIFY_OBJECTIVE_FORMAT_CLEAR
    };
    let message = key
        .message([TextComponent::from(objective.name().to_owned())])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn clear_display_slot(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let slot = context.scoreboard_slot("slot")?;
    if scoreboard.display_objective(slot).is_none() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADY_EMPTY
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_display_objective(slot, None)
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_CLEARED
        .message([TextComponent::from(slot.as_str())])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_display_slot(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let slot = context.scoreboard_slot("slot")?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    if scoreboard.display_objective(slot).as_deref() == Some(objective.name()) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_ALREADY_SET
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_display_objective(slot, Some(&objective))
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_SCOREBOARD_OBJECTIVES_DISPLAY_SET
        .message([
            TextComponent::from(slot.as_str()),
            objective.display_name().clone(),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn list_tracked_players(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let holders = scoreboard.tracked_holders();
    let source = context.source();
    if holders.is_empty() {
        source.send_success(
            &translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_EMPTY
                .msg()
                .component(),
            false,
        );
    } else {
        let listed = format_list(holders.iter().map(ScoreHolder::feedback_display_name));
        let message = translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_SUCCESS
            .message([TextComponent::from(holders.len().to_string()), listed])
            .component();
        source.send_success(&message, false);
    }
    Ok(holders.len() as i32)
}

fn list_tracked_player_scores(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let holder = context.score_holder("target")?;
    let scores = scoreboard.list_player_scores(&holder);
    let source = context.source();
    if scores.is_empty() {
        let message = translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_EMPTY
            .message([holder.feedback_display_name()])
            .component();
        source.send_success(&message, false);
    } else {
        let message = translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_SUCCESS
            .message([
                holder.feedback_display_name(),
                TextComponent::from(scores.len().to_string()),
            ])
            .component();
        source.send_success(&message, false);
        for (objective_name, score) in &scores {
            let display_name = scoreboard.objective(objective_name).map_or_else(
                || TextComponent::from(objective_name.clone()),
                |objective| objective.formatted_display_name(),
            );
            let message = translations::COMMANDS_SCOREBOARD_PLAYERS_LIST_ENTITY_ENTRY
                .message([display_name, TextComponent::from(score.value().to_string())])
                .component();
            source.send_success(&message, false);
        }
    }
    Ok(scores.len() as i32)
}

fn set_score(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let objective = writable_objective_argument(context, scoreboard, "objective")?;
    let value = context.integer("score")?;
    let mut tracker = ScoreTracker::default();
    for target in &targets {
        scoreboard
            .set_score(target, &objective, value)
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), value);
    }

    let message = match tracker.first(ElementType::NonZero) {
        Some(holder) => translations::COMMANDS_SCOREBOARD_PLAYERS_SET_SUCCESS_SINGLE
            .message([
                objective.formatted_display_name(),
                holder.feedback_display_name(),
                TextComponent::from(value.to_string()),
            ])
            .component(),
        None => translations::COMMANDS_SCOREBOARD_PLAYERS_SET_SUCCESS_MULTIPLE
            .message([
                objective.formatted_display_name(),
                TextComponent::from(tracker.count(ElementType::NonZero).to_string()),
                TextComponent::from(value.to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn get_score(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let target = context.score_holder("target")?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    let Some(value) = scoreboard.score(&target, &objective) else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCOREBOARD_PLAYERS_GET_NULL
                .message([
                    TextComponent::from(objective.name().to_owned()),
                    target.feedback_display_name(),
                ])
                .component(),
        ));
    };

    let message = translations::COMMANDS_SCOREBOARD_PLAYERS_GET_SUCCESS
        .message([
            target.feedback_display_name(),
            TextComponent::from(value.to_string()),
            objective.formatted_display_name(),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(value)
}

fn add_score(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let objective = writable_objective_argument(context, scoreboard, "objective")?;
    let value = context.integer("score")?;
    let mut tracker = ScoreTracker::default();
    for target in &targets {
        let current = scoreboard
            .get_or_create_score(target, &objective, false)
            .map_err(scoreboard_error)?;
        let total = current.saturating_add(value);
        scoreboard
            .set_score(target, &objective, total)
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), total);
    }

    let message = match tracker.first(ElementType::Any) {
        Some(holder) => translations::COMMANDS_SCOREBOARD_PLAYERS_ADD_SUCCESS_SINGLE
            .message([
                TextComponent::from(value.to_string()),
                objective.formatted_display_name(),
                holder.feedback_display_name(),
                TextComponent::from(tracker.total_value().to_string()),
            ])
            .component(),
        None => translations::COMMANDS_SCOREBOARD_PLAYERS_ADD_SUCCESS_MULTIPLE
            .message([
                TextComponent::from(value.to_string()),
                objective.formatted_display_name(),
                TextComponent::from(tracker.count(ElementType::Any).to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn remove_score(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let objective = writable_objective_argument(context, scoreboard, "objective")?;
    let value = context.integer("score")?;
    let mut tracker = ScoreTracker::default();
    for target in &targets {
        let current = scoreboard
            .get_or_create_score(target, &objective, false)
            .map_err(scoreboard_error)?;
        let total = current.saturating_sub(value);
        scoreboard
            .set_score(target, &objective, total)
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), total);
    }

    let message = match tracker.first(ElementType::Any) {
        Some(holder) => translations::COMMANDS_SCOREBOARD_PLAYERS_REMOVE_SUCCESS_SINGLE
            .message([
                TextComponent::from(value.to_string()),
                objective.formatted_display_name(),
                holder.feedback_display_name(),
                TextComponent::from(tracker.total_value().to_string()),
            ])
            .component(),
        None => translations::COMMANDS_SCOREBOARD_PLAYERS_REMOVE_SUCCESS_MULTIPLE
            .message([
                TextComponent::from(value.to_string()),
                objective.formatted_display_name(),
                TextComponent::from(tracker.count(ElementType::Any).to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn reset_scores(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let mut tracker = ScoreTracker::default();
    for target in &targets {
        scoreboard
            .reset_scores(target, None)
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), 1);
    }

    let message = match tracker.first(ElementType::NonZero) {
        Some(holder) => translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_ALL_SINGLE
            .message([holder.feedback_display_name()])
            .component(),
        None => translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_ALL_MULTIPLE
            .message([targets.len().to_string()])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn reset_score(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    let mut tracker = ScoreTracker::default();
    for target in &targets {
        scoreboard
            .reset_scores(target, Some(&objective))
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), 1);
    }

    let message = match tracker.first(ElementType::NonZero) {
        Some(holder) => translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_SPECIFIC_SINGLE
            .message([
                objective.formatted_display_name(),
                holder.feedback_display_name(),
            ])
            .component(),
        None => translations::COMMANDS_SCOREBOARD_PLAYERS_RESET_SPECIFIC_MULTIPLE
            .message([
                objective.formatted_display_name(),
                TextComponent::from(tracker.count(ElementType::NonZero).to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn enable_trigger(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    if !is_trigger(&objective) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_INVALID
                .msg()
                .component(),
        ));
    }

    let mut tracker = ScoreTracker::default();
    for target in &targets {
        // Vanilla reads the accessor first, so an absent score is created locked.
        scoreboard
            .get_or_create_score(target, &objective, false)
            .map_err(scoreboard_error)?;
        let is_locked = scoreboard
            .score_entry(target, &objective)
            .is_none_or(|score| score.is_locked());
        if is_locked {
            scoreboard
                .set_score_locked(target, &objective, false)
                .map_err(scoreboard_error)?;
            tracker.track(target.clone(), 1);
        }
    }

    if tracker.count(ElementType::NonZero) == 0 {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_FAILED
                .msg()
                .component(),
        ));
    }

    let message = match tracker.first(ElementType::NonZero) {
        Some(holder) => translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_SINGLE
            .message([
                objective.formatted_display_name(),
                holder.feedback_display_name(),
            ])
            .component(),
        None => translations::COMMANDS_SCOREBOARD_PLAYERS_ENABLE_SUCCESS_MULTIPLE
            .message([
                objective.formatted_display_name(),
                TextComponent::from(tracker.count(ElementType::NonZero).to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn set_score_display_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let display = context.text_component("name")?.clone();
    set_score_display(context, Some(display))
}

fn clear_score_display_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    set_score_display(context, None)
}

fn set_score_display(
    context: &SteelCommandContext<CommandSource>,
    display: Option<TextComponent>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    let mut tracker = ScoreTracker::default();
    for target in &targets {
        scoreboard
            .set_score_display(target, &objective, display.clone())
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), 1);
    }

    let holder_message = |holder: &ScoreHolder| {
        if let Some(display) = &display {
            translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_SET_SUCCESS_SINGLE
                .message([
                    display.clone(),
                    holder.feedback_display_name(),
                    objective.formatted_display_name(),
                ])
                .component()
        } else {
            translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_CLEAR_SUCCESS_SINGLE
                .message([
                    holder.feedback_display_name(),
                    objective.formatted_display_name(),
                ])
                .component()
        }
    };
    let message = if let Some(holder) = tracker.first(ElementType::NonZero) {
        holder_message(holder)
    } else {
        let count = TextComponent::from(tracker.count(ElementType::NonZero).to_string());
        if let Some(display) = &display {
            translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_SET_SUCCESS_MULTIPLE
                .message([display.clone(), count, objective.formatted_display_name()])
                .component()
        } else {
            translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NAME_CLEAR_SUCCESS_MULTIPLE
                .message([count, objective.formatted_display_name()])
                .component()
        }
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn set_score_number_format(
    context: &SteelCommandContext<CommandSource>,
    number_format: Option<NumberFormat>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let objective = objective_argument(context, scoreboard, "objective")?;
    let mut tracker = ScoreTracker::default();
    for target in &targets {
        scoreboard
            .set_score_number_format(target, &objective, number_format.clone())
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), 1);
    }

    let message = if let Some(holder) = tracker.first(ElementType::NonZero) {
        let key = if number_format.is_some() {
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_SET_SUCCESS_SINGLE
        } else {
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_CLEAR_SUCCESS_SINGLE
        };
        key.message([
            holder.feedback_display_name(),
            objective.formatted_display_name(),
        ])
        .component()
    } else {
        let count = TextComponent::from(tracker.count(ElementType::NonZero).to_string());
        let key = if number_format.is_some() {
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_SET_SUCCESS_MULTIPLE
        } else {
            &translations::COMMANDS_SCOREBOARD_PLAYERS_DISPLAY_NUMBER_FORMAT_CLEAR_SUCCESS_MULTIPLE
        };
        key.message([count, objective.formatted_display_name()])
            .component()
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn perform_operation(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let targets = context.score_holders("targets", ScoreHolderWildcard::Tracked)?;
    let target_objective = writable_objective_argument(context, scoreboard, "targetObjective")?;
    let operation = context.operation("operation")?;
    let sources = context.score_holders("source", ScoreHolderWildcard::Tracked)?;
    let source_objective = objective_argument(context, scoreboard, "sourceObjective")?;

    let mut tracker = ScoreTracker::default();
    for target in &targets {
        let mut target_value = scoreboard
            .get_or_create_score(target, &target_objective, false)
            .map_err(scoreboard_error)?;
        for from in &sources {
            let source_value = scoreboard
                .get_or_create_score(from, &source_objective, false)
                .map_err(scoreboard_error)?;
            if operation.is_swap() {
                scoreboard
                    .set_score(from, &source_objective, target_value)
                    .map_err(scoreboard_error)?;
                target_value = source_value;
            } else {
                target_value = operation.apply_values(target_value, source_value)?;
            }
        }
        scoreboard
            .set_score(target, &target_objective, target_value)
            .map_err(scoreboard_error)?;
        tracker.track(target.clone(), target_value);
    }

    let message = match tracker.first(ElementType::Any) {
        Some(holder) => translations::COMMANDS_SCOREBOARD_PLAYERS_OPERATION_SUCCESS_SINGLE
            .message([
                target_objective.formatted_display_name(),
                holder.feedback_display_name(),
                TextComponent::from(tracker.total_value().to_string()),
            ])
            .component(),
        None => translations::COMMANDS_SCOREBOARD_PLAYERS_OPERATION_SUCCESS_MULTIPLE
            .message([
                target_objective.formatted_display_name(),
                TextComponent::from(tracker.count(ElementType::Any).to_string()),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

/// Vanilla `CommandResponseTracker.ElementType`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ElementType {
    /// Counts every tracked element.
    Any,
    /// Counts only elements tracked with a non-zero value.
    NonZero,
}

/// Vanilla `CommandResponseTracker` for score holders.
#[derive(Default)]
pub(super) struct ScoreTracker {
    total_value: i32,
    only_element: Option<ScoreHolder>,
    element_count: usize,
    only_non_zero_element: Option<ScoreHolder>,
    non_zero_element_count: usize,
}

impl ScoreTracker {
    /// Tracks one element with its vanilla feedback value.
    pub(super) fn track(&mut self, holder: ScoreHolder, value: i32) {
        self.total_value += value;
        self.element_count += 1;
        self.only_element = (self.element_count == 1).then_some(holder.clone());
        if value != 0 {
            self.non_zero_element_count += 1;
            self.only_non_zero_element = (self.non_zero_element_count == 1).then_some(holder);
        }
    }

    /// Vanilla `CommandResponseTracker.firstElement`.
    pub(super) const fn first(&self, element_type: ElementType) -> Option<&ScoreHolder> {
        match element_type {
            ElementType::Any => self.only_element.as_ref(),
            ElementType::NonZero => self.only_non_zero_element.as_ref(),
        }
    }

    /// Vanilla `CommandResponseTracker.elementCount`.
    pub(super) const fn count(&self, element_type: ElementType) -> usize {
        match element_type {
            ElementType::Any => self.element_count,
            ElementType::NonZero => self.non_zero_element_count,
        }
    }

    /// Vanilla `CommandResponseTracker.totalValue`.
    pub(super) const fn total_value(&self) -> i32 {
        self.total_value
    }
}

/// Resolves the scoreboard owned by the source's domain.
pub(super) fn source_scoreboard(
    context: &SteelCommandContext<CommandSource>,
) -> Result<&Scoreboard, CommandSyntaxError> {
    source_scoreboard_for(context.source())
}

pub(super) fn source_scoreboard_for(
    source: &CommandSource,
) -> Result<&Scoreboard, CommandSyntaxError> {
    source
        .server()
        .scoreboards
        .get(source.world().domain())
        .ok_or_else(|| {
            CommandSyntaxError::dynamic(format!(
                "Domain '{}' has no command scoreboard",
                source.world().domain()
            ))
        })
}

/// Maps a scoreboard failure onto a command error.
pub(super) fn scoreboard_error(error: ScoreboardError) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(error.to_string()))
}

fn objective_criteria(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<ObjectiveCriteria, CommandSyntaxError> {
    let criteria_name = context.objective_criteria(name)?;
    ObjectiveCriteria::parse(criteria_name).ok_or_else(|| {
        CommandSyntaxError::dynamic(
            translations::ARGUMENT_CRITERIA_INVALID
                .message([TextComponent::from(criteria_name.to_owned())])
                .component(),
        )
    })
}

fn objective_argument(
    context: &SteelCommandContext<CommandSource>,
    scoreboard: &Scoreboard,
    name: &str,
) -> Result<ScoreboardObjective, CommandSyntaxError> {
    let objective_name = context.objective_name(name)?;
    require_objective(scoreboard, objective_name)
}

fn writable_objective_argument(
    context: &SteelCommandContext<CommandSource>,
    scoreboard: &Scoreboard,
    name: &str,
) -> Result<ScoreboardObjective, CommandSyntaxError> {
    let objective = objective_argument(context, scoreboard, name)?;
    if objective.is_read_only() {
        return Err(CommandSyntaxError::dynamic(
            translations::ARGUMENTS_OBJECTIVE_READONLY
                .message([TextComponent::from(objective.name().to_owned())])
                .component(),
        ));
    }
    Ok(objective)
}

fn require_objective(
    scoreboard: &Scoreboard,
    name: &str,
) -> Result<ScoreboardObjective, CommandSyntaxError> {
    scoreboard.objective(name).ok_or_else(|| {
        CommandSyntaxError::dynamic(
            translations::ARGUMENTS_OBJECTIVE_NOT_FOUND
                .message([TextComponent::from(name.to_owned())])
                .component(),
        )
    })
}

fn objective_snapshot(scoreboard: &Scoreboard) -> Vec<ScoreboardObjective> {
    scoreboard
        .objective_names()
        .into_iter()
        .filter_map(|name| scoreboard.objective(&name))
        .collect()
}

/// Vanilla `ObjectiveCriteria.TRIGGER` identity check.
pub(super) fn is_trigger(objective: &ScoreboardObjective) -> bool {
    objective.criteria().name() == ObjectiveCriteria::trigger().name()
}

/// Vanilla `ComponentUtils.formatList` with the default gray separator.
pub(super) fn format_list(values: impl IntoIterator<Item = TextComponent>) -> TextComponent {
    let mut values = values.into_iter();
    let Some(first) = values.next() else {
        return TextComponent::plain("");
    };
    let Some(second) = values.next() else {
        return first;
    };

    let mut result = TextComponent::plain("")
        .add_child(first)
        .add_child(separator());
    result = result.add_child(second);
    for value in values {
        result = result.add_child(separator()).add_child(value);
    }
    result
}

fn separator() -> TextComponent {
    TextComponent::plain(", ").color(Color::Gray)
}
