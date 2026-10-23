//! Vanilla `team` command.
//!
//! Mirrors `TeamCommand`, backed by [`crate::scoreboard::Scoreboard`].

use steel_utils::{Identifier, translations};
use text_components::{Modifier, TextComponent, format::Color};

use super::{
    super::{
        brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
        execution::{
            CommandSource, ScoreHolderWildcard, SteelArgumentType, SteelCommandContext,
            SteelCommandRuntime, argument, literal,
        },
        registration::CommandRegistration,
    },
    scoreboard::{ElementType, ScoreTracker, format_list, scoreboard_error, source_scoreboard},
};
use crate::scoreboard::{
    CollisionRule, PlayerTeam, ScoreHolder, Scoreboard, ScoreboardError, TeamVisibility,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("team"), |_| command())
}

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

fn command() -> Builder {
    literal("team")
        .then(
            literal("list")
                .executes(list_teams)
                .then(argument("team", SteelArgumentType::team()).executes(list_members)),
        )
        .then(
            literal("add").then(
                argument("team", ArgumentType::word())
                    .executes(add_team_default_name)
                    .then(
                        argument("displayName", SteelArgumentType::component())
                            .executes(add_team_display_name),
                    ),
            ),
        )
        .then(
            literal("remove")
                .then(argument("team", SteelArgumentType::team()).executes(remove_team)),
        )
        .then(
            literal("empty").then(argument("team", SteelArgumentType::team()).executes(empty_team)),
        )
        .then(
            literal("join").then(
                argument("team", SteelArgumentType::team())
                    .executes(join_owning_entity)
                    .then(
                        argument("members", SteelArgumentType::score_holders())
                            .executes(join_members),
                    ),
            ),
        )
        .then(
            literal("leave").then(
                argument("members", SteelArgumentType::score_holders()).executes(leave_members),
            ),
        )
        .then(
            literal("modify").then(
                argument("team", SteelArgumentType::team())
                    .then(
                        literal("displayName").then(
                            argument("displayName", SteelArgumentType::component())
                                .executes(set_display_name),
                        ),
                    )
                    .then(
                        literal("color")
                            .then(literal("reset").executes(clear_color))
                            .then(
                                argument("value", SteelArgumentType::team_color())
                                    .executes(set_color),
                            ),
                    )
                    .then(literal("friendlyFire").then(
                        argument("allowed", ArgumentType::bool()).executes(set_friendly_fire),
                    ))
                    .then(literal("seeFriendlyInvisibles").then(
                        argument("allowed", ArgumentType::bool()).executes(set_friendly_sight),
                    ))
                    .then(
                        literal("nametagVisibility")
                            .then(literal("never").executes(|context| {
                                set_name_tag_visibility(context, TeamVisibility::Never)
                            }))
                            .then(literal("hideForOtherTeams").executes(|context| {
                                set_name_tag_visibility(context, TeamVisibility::HideForOtherTeams)
                            }))
                            .then(literal("hideForOwnTeam").executes(|context| {
                                set_name_tag_visibility(context, TeamVisibility::HideForOwnTeam)
                            }))
                            .then(literal("always").executes(|context| {
                                set_name_tag_visibility(context, TeamVisibility::Always)
                            })),
                    )
                    .then(
                        literal("deathMessageVisibility")
                            .then(literal("never").executes(|context| {
                                set_death_message_visibility(context, TeamVisibility::Never)
                            }))
                            .then(literal("hideForOtherTeams").executes(|context| {
                                set_death_message_visibility(
                                    context,
                                    TeamVisibility::HideForOtherTeams,
                                )
                            }))
                            .then(literal("hideForOwnTeam").executes(|context| {
                                set_death_message_visibility(
                                    context,
                                    TeamVisibility::HideForOwnTeam,
                                )
                            }))
                            .then(literal("always").executes(|context| {
                                set_death_message_visibility(context, TeamVisibility::Always)
                            })),
                    )
                    .then(
                        literal("collisionRule")
                            .then(
                                literal("never").executes(|context| {
                                    set_collision(context, CollisionRule::Never)
                                }),
                            )
                            .then(literal("pushOwnTeam").executes(|context| {
                                set_collision(context, CollisionRule::PushOwnTeam)
                            }))
                            .then(literal("pushOtherTeams").executes(|context| {
                                set_collision(context, CollisionRule::PushOtherTeams)
                            }))
                            .then(
                                literal("always").executes(|context| {
                                    set_collision(context, CollisionRule::Always)
                                }),
                            ),
                    )
                    .then(literal("prefix").then(
                        argument("prefix", SteelArgumentType::component()).executes(set_prefix),
                    ))
                    .then(literal("suffix").then(
                        argument("suffix", SteelArgumentType::component()).executes(set_suffix),
                    )),
            ),
        )
}

fn list_teams(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let teams = all_teams(scoreboard);
    let source = context.source();
    if teams.is_empty() {
        source.send_success(
            &translations::COMMANDS_TEAM_LIST_TEAMS_EMPTY
                .msg()
                .component(),
            false,
        );
    } else {
        let listed = format_list(teams.iter().map(PlayerTeam::formatted_display_name));
        let message = translations::COMMANDS_TEAM_LIST_TEAMS_SUCCESS
            .message([TextComponent::from(teams.len().to_string()), listed])
            .component();
        source.send_success(&message, false);
    }
    Ok(teams.len() as i32)
}

fn list_members(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let members = scoreboard.team_holders(team.name());
    let source = context.source();
    if members.is_empty() {
        let message = translations::COMMANDS_TEAM_LIST_MEMBERS_EMPTY
            .message([team.formatted_display_name()])
            .component();
        source.send_success(&message, false);
    } else {
        let listed = format_list(members.iter().map(|member| green_member(member)));
        let message = translations::COMMANDS_TEAM_LIST_MEMBERS_SUCCESS
            .message([
                team.formatted_display_name(),
                TextComponent::from(members.len().to_string()),
                listed,
            ])
            .component();
        source.send_success(&message, false);
    }
    Ok(members.len() as i32)
}

fn add_team_default_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let name = context.string("team")?.to_owned();
    let display_name = TextComponent::plain(name.clone());
    add_team(context, name, display_name)
}

fn add_team_display_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let name = context.string("team")?.to_owned();
    let display_name = context.text_component("displayName")?.clone();
    add_team(context, name, display_name)
}

fn add_team(
    context: &SteelCommandContext<CommandSource>,
    name: String,
    display_name: TextComponent,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    if scoreboard.team(&name).is_some() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_ADD_DUPLICATE.msg().component(),
        ));
    }

    let team = scoreboard
        .add_team(name.clone())
        .map_err(scoreboard_error)?;
    scoreboard
        .set_team_display_name(team.name(), display_name)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, &name)?;
    let message = translations::COMMANDS_TEAM_ADD_SUCCESS
        .message([team.formatted_display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(scoreboard.team_names().len() as i32)
}

fn remove_team(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    scoreboard
        .remove_team(team.name())
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_TEAM_REMOVE_SUCCESS
        .message([team.formatted_display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(scoreboard.team_names().len() as i32)
}

fn empty_team(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let members = scoreboard.team_holders(team.name());
    if members.is_empty() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_EMPTY_UNCHANGED
                .msg()
                .component(),
        ));
    }

    for member in &members {
        let holder = ScoreHolder::new(member.clone());
        scoreboard
            .remove_holder_from_team(&holder, &team)
            .map_err(scoreboard_error)?;
    }
    let message = translations::COMMANDS_TEAM_EMPTY_SUCCESS
        .message([
            TextComponent::from(members.len().to_string()),
            team.formatted_display_name(),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(members.len() as i32)
}

fn join_owning_entity(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let entity = source.entity().cloned().ok_or_else(|| {
        CommandSyntaxError::dynamic(translations::PERMISSIONS_REQUIRES_ENTITY.msg().component())
    })?;
    join_team(context, vec![ScoreHolder::new(entity.scoreboard_name())])
}

fn join_members(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let members = context.score_holders("members", ScoreHolderWildcard::Tracked)?;
    join_team(context, members)
}

fn join_team(
    context: &SteelCommandContext<CommandSource>,
    members: Vec<ScoreHolder>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let mut tracker = ScoreTracker::default();
    for member in &members {
        // Vanilla tracks whether the team's member set actually changed.
        let changed = scoreboard.holder_team_name(member).as_deref() != Some(team.name());
        scoreboard
            .add_holder_to_team(member, &team)
            .map_err(scoreboard_error)?;
        tracker.track(member.clone(), i32::from(changed));
    }

    let message = match tracker.first(ElementType::NonZero) {
        Some(holder) => translations::COMMANDS_TEAM_JOIN_SUCCESS_SINGLE
            .message([
                holder.feedback_display_name(),
                team.formatted_display_name(),
            ])
            .component(),
        None => translations::COMMANDS_TEAM_JOIN_SUCCESS_MULTIPLE
            .message([
                TextComponent::from(tracker.count(ElementType::NonZero).to_string()),
                team.formatted_display_name(),
            ])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn leave_members(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let members = context.score_holders("members", ScoreHolderWildcard::Tracked)?;
    let mut tracker = ScoreTracker::default();
    for member in &members {
        let removed = leave_any_team(scoreboard, member).map_err(scoreboard_error)?;
        tracker.track(member.clone(), i32::from(removed));
    }

    let message = match tracker.first(ElementType::NonZero) {
        Some(holder) => translations::COMMANDS_TEAM_LEAVE_SUCCESS_SINGLE
            .message([holder.feedback_display_name()])
            .component(),
        None => translations::COMMANDS_TEAM_LEAVE_SUCCESS_MULTIPLE
            .message([TextComponent::from(members.len().to_string())])
            .component(),
    };
    context.source().send_success(&message, true);
    Ok(tracker.total_value())
}

fn set_display_name(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let display_name = context.text_component("displayName")?.clone();
    if team.display_name() == &display_name {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_OPTION_NAME_UNCHANGED
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_team_display_name(team.name(), display_name)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let message = translations::COMMANDS_TEAM_OPTION_NAME_SUCCESS
        .message([team.formatted_display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_color(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let color = context.team_color("value")?;
    if team.color() == Some(color) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_OPTION_COLOR_UNCHANGED
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_team_color(team.name(), Some(color))
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let message = translations::COMMANDS_TEAM_OPTION_COLOR_SUCCESS
        .message([
            team.formatted_display_name(),
            TextComponent::from(color.as_str()),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn clear_color(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    if team.color().is_none() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_OPTION_COLOR_UNCHANGED
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_team_color(team.name(), None)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let message = translations::COMMANDS_TEAM_OPTION_COLOR_CLEAR_SUCCESS
        .message([team.formatted_display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_friendly_fire(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let allowed = context.boolean("allowed")?;
    if team.allow_friendly_fire() == allowed {
        let key = if allowed {
            &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADY_ENABLED
        } else {
            &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ALREADY_DISABLED
        };
        return Err(CommandSyntaxError::dynamic(key.msg().component()));
    }

    scoreboard
        .set_team_friendly_fire(team.name(), allowed)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let key = if allowed {
        &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_ENABLED
    } else {
        &translations::COMMANDS_TEAM_OPTION_FRIENDLYFIRE_DISABLED
    };
    let message = key.message([team.formatted_display_name()]).component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_friendly_sight(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let allowed = context.boolean("allowed")?;
    if team.see_friendly_invisibles() == allowed {
        let key = if allowed {
            &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_ALREADY_ENABLED
        } else {
            &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_ALREADY_DISABLED
        };
        return Err(CommandSyntaxError::dynamic(key.msg().component()));
    }

    scoreboard
        .set_team_see_friendly_invisibles(team.name(), allowed)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let key = if allowed {
        &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_ENABLED
    } else {
        &translations::COMMANDS_TEAM_OPTION_SEE_FRIENDLY_INVISIBLES_DISABLED
    };
    let message = key.message([team.formatted_display_name()]).component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_name_tag_visibility(
    context: &SteelCommandContext<CommandSource>,
    visibility: TeamVisibility,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    if team.name_tag_visibility() == visibility {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_OPTION_NAMETAG_VISIBILITY_UNCHANGED
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_team_nametag_visibility(team.name(), visibility)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let message = translations::COMMANDS_TEAM_OPTION_NAMETAG_VISIBILITY_SUCCESS
        .message([
            team.formatted_display_name(),
            visibility_display_name(visibility),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_death_message_visibility(
    context: &SteelCommandContext<CommandSource>,
    visibility: TeamVisibility,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    if team.death_message_visibility() == visibility {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_OPTION_DEATH_MESSAGE_VISIBILITY_UNCHANGED
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_team_death_message_visibility(team.name(), visibility)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let message = translations::COMMANDS_TEAM_OPTION_DEATH_MESSAGE_VISIBILITY_SUCCESS
        .message([
            team.formatted_display_name(),
            visibility_display_name(visibility),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_collision(
    context: &SteelCommandContext<CommandSource>,
    rule: CollisionRule,
) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    if team.collision_rule() == rule {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_TEAM_OPTION_COLLISION_RULE_UNCHANGED
                .msg()
                .component(),
        ));
    }

    scoreboard
        .set_team_collision_rule(team.name(), rule)
        .map_err(scoreboard_error)?;
    let team = require_team(scoreboard, team.name())?;
    let message = translations::COMMANDS_TEAM_OPTION_COLLISION_RULE_SUCCESS
        .message([team.formatted_display_name(), collision_display_name(rule)])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn set_prefix(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let prefix = context.text_component("prefix")?.clone();
    scoreboard
        .set_team_prefix(team.name(), prefix.clone())
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_TEAM_OPTION_PREFIX_SUCCESS
        .message([prefix])
        .component();
    context.source().send_success(&message, false);
    Ok(1)
}

fn set_suffix(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let scoreboard = source_scoreboard(context)?;
    let team = team_argument(context, scoreboard, "team")?;
    let suffix = context.text_component("suffix")?.clone();
    scoreboard
        .set_team_suffix(team.name(), suffix.clone())
        .map_err(scoreboard_error)?;
    let message = translations::COMMANDS_TEAM_OPTION_SUFFIX_SUCCESS
        .message([suffix])
        .component();
    context.source().send_success(&message, false);
    Ok(1)
}

/// Vanilla `TeamArgument.getTeam`: resolves the parsed team name to its options.
fn team_argument(
    context: &SteelCommandContext<CommandSource>,
    scoreboard: &Scoreboard,
    name: &str,
) -> Result<PlayerTeam, CommandSyntaxError> {
    require_team(scoreboard, context.team_argument(name)?)
}

fn require_team(scoreboard: &Scoreboard, name: &str) -> Result<PlayerTeam, CommandSyntaxError> {
    scoreboard.team(name).ok_or_else(|| team_not_found(name))
}

fn team_not_found(name: &str) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::TEAM_NOT_FOUND
            .message([TextComponent::from(name.to_owned())])
            .component(),
    )
}

/// Vanilla `Scoreboard.removePlayerFromTeam(player)`: leaves whatever team it is on.
fn leave_any_team(scoreboard: &Scoreboard, holder: &ScoreHolder) -> Result<bool, ScoreboardError> {
    let Some(team_name) = scoreboard.holder_team_name(holder) else {
        return Ok(false);
    };
    let team = scoreboard
        .team(&team_name)
        .ok_or(ScoreboardError::MissingTeam(team_name))?;
    scoreboard.remove_holder_from_team(holder, &team)?;
    Ok(true)
}

fn all_teams(scoreboard: &Scoreboard) -> Vec<PlayerTeam> {
    scoreboard
        .team_names()
        .into_iter()
        .filter_map(|name| scoreboard.team(&name))
        .collect()
}

fn green_member(member: &str) -> TextComponent {
    TextComponent::from(member.to_owned()).color(Color::Green)
}

/// Vanilla `Team.Visibility.getDisplayName`.
fn visibility_display_name(visibility: TeamVisibility) -> TextComponent {
    match visibility {
        TeamVisibility::Always => translations::TEAM_VISIBILITY_ALWAYS.msg().component(),
        TeamVisibility::Never => translations::TEAM_VISIBILITY_NEVER.msg().component(),
        TeamVisibility::HideForOtherTeams => translations::TEAM_VISIBILITY_HIDE_FOR_OTHER_TEAMS
            .msg()
            .component(),
        TeamVisibility::HideForOwnTeam => translations::TEAM_VISIBILITY_HIDE_FOR_OWN_TEAM
            .msg()
            .component(),
    }
}

/// Vanilla `Team.CollisionRule.getDisplayName`.
fn collision_display_name(rule: CollisionRule) -> TextComponent {
    match rule {
        CollisionRule::Always => translations::TEAM_COLLISION_ALWAYS.msg().component(),
        CollisionRule::Never => translations::TEAM_COLLISION_NEVER.msg().component(),
        CollisionRule::PushOtherTeams => translations::TEAM_COLLISION_PUSH_OTHER_TEAMS
            .msg()
            .component(),
        CollisionRule::PushOwnTeam => translations::TEAM_COLLISION_PUSH_OWN_TEAM.msg().component(),
    }
}
