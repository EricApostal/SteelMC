//! Vanilla server allowlist command.
//!
//! Mirrors `WhitelistCommand`, backed by the allowlist in
//! [`crate::server::player_lists`] through `Server::allowlist()`.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, GameProfileArgument, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, SteelSuggestionContext, argument, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use super::ban::{ModerationOutcome, ModerationSuspension, count_result};
use crate::{permission::PermissionLevel, server::player_lists::AllowListEntry};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("whitelist"), |_| command())
        .at_level(PermissionLevel::Admins)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("whitelist")
        .then(literal("on").executes_suspended(start_enable))
        .then(literal("off").executes_suspended(start_disable))
        .then(literal("list").executes(show_list))
        .then(
            literal("add").then(
                argument("targets", SteelArgumentType::game_profile())
                    .suggests(suggest_unlisted_online_players)
                    .executes_suspended(start_add_players),
            ),
        )
        .then(
            literal("remove").then(
                argument("targets", SteelArgumentType::game_profile())
                    .suggests(
                        |context: &SteelSuggestionContext<'_, CommandSource>,
                         builder: &mut SuggestionsBuilder<'_>| {
                            suggest_list(builder, context.source().server().allowlist().names());
                        },
                    )
                    .executes_suspended(start_remove_players),
            ),
        )
        .then(literal("reload").executes_suspended(start_reload))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn start_enable(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        enable_whitelist(&source)
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn start_disable(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        disable_whitelist(&source)
    }))
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn start_reload(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        reload_whitelist(&source).await
    }))
}

fn start_add_players(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let targets = context.game_profile_argument("targets")?.clone();
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        add_players(&source, targets).await
    }))
}

fn start_remove_players(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let targets = context.game_profile_argument("targets")?.clone();
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        remove_players(&source, targets).await
    }))
}

/// Vanilla `WhitelistCommand.enableWhitelist`.
fn enable_whitelist(source: &CommandSource) -> Result<ModerationOutcome, CommandSyntaxError> {
    let server = source.server();
    if server.allowlist().is_enabled() {
        return Err(already_enabled());
    }

    server.allowlist().set_enabled(true);
    server.kick_unlisted_players();
    Ok(single_message(TextComponent::from(
        &translations::COMMANDS_WHITELIST_ENABLED,
    )))
}

/// Vanilla `WhitelistCommand.disableWhitelist`.
fn disable_whitelist(source: &CommandSource) -> Result<ModerationOutcome, CommandSyntaxError> {
    let server = source.server();
    if !server.allowlist().is_enabled() {
        return Err(already_disabled());
    }

    server.allowlist().set_enabled(false);
    Ok(single_message(TextComponent::from(
        &translations::COMMANDS_WHITELIST_DISABLED,
    )))
}

/// Vanilla `WhitelistCommand.reload`.
async fn reload_whitelist(source: &CommandSource) -> Result<ModerationOutcome, CommandSyntaxError> {
    let server = source.server();
    if let Err(error) = server.allowlist().reload().await {
        log::warn!("Failed to reload the allowlist: {error}");
    }
    server.kick_unlisted_players();
    Ok(single_message(TextComponent::from(
        &translations::COMMANDS_WHITELIST_RELOADED,
    )))
}

/// Vanilla `WhitelistCommand.addPlayers`.
async fn add_players(
    source: &CommandSource,
    targets: GameProfileArgument,
) -> Result<ModerationOutcome, CommandSyntaxError> {
    let profiles = targets.resolve(source).await?;
    let server = source.server();
    let mut messages = Vec::new();
    let mut success = 0i32;

    for profile in profiles {
        if server.allowlist().is_allowed(profile.uuid) {
            continue;
        }
        let entry = AllowListEntry::new(profile.uuid, profile.name.clone());
        if server.allowlist().add(entry).await {
            success += 1;
            messages.push(
                translations::COMMANDS_WHITELIST_ADD_SUCCESS
                    .message([TextComponent::plain(profile.name.clone())])
                    .component(),
            );
        }
    }

    if success == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_WHITELIST_ADD_FAILED,
        )));
    }
    Ok(ModerationOutcome {
        messages,
        result: success,
    })
}

/// Vanilla `WhitelistCommand.removePlayers`.
async fn remove_players(
    source: &CommandSource,
    targets: GameProfileArgument,
) -> Result<ModerationOutcome, CommandSyntaxError> {
    let profiles = targets.resolve(source).await?;
    let server = source.server();
    let mut messages = Vec::new();
    let mut success = 0i32;

    for profile in profiles {
        if !server.allowlist().is_allowed(profile.uuid) {
            continue;
        }
        if server.allowlist().remove(profile.uuid).await {
            success += 1;
            messages.push(
                translations::COMMANDS_WHITELIST_REMOVE_SUCCESS
                    .message([TextComponent::plain(profile.name.clone())])
                    .component(),
            );
        }
    }

    if success == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_WHITELIST_REMOVE_FAILED,
        )));
    }

    server.kick_unlisted_players();
    Ok(ModerationOutcome {
        messages,
        result: success,
    })
}

/// Vanilla `WhitelistCommand.showList`.
fn show_list(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let names = source.server().allowlist().names();
    if names.is_empty() {
        let message = TextComponent::from(&translations::COMMANDS_WHITELIST_NONE);
        source.send_success(&message, false);
    } else {
        let message = translations::COMMANDS_WHITELIST_LIST
            .message([
                TextComponent::plain(names.len().to_string()),
                TextComponent::plain(names.join(", ")),
            ])
            .component();
        source.send_success(&message, false);
    }

    count_result(names.len())
}

fn suggest_unlisted_online_players(
    context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let server = context.source().server();
    let names = server
        .get_players()
        .into_iter()
        .filter(|player| !server.allowlist().is_allowed(player.gameprofile.id()))
        .map(|player| player.gameprofile.name().to_owned());
    suggest_list(builder, names);
}

fn single_message(message: TextComponent) -> ModerationOutcome {
    ModerationOutcome {
        messages: vec![message],
        result: 1,
    }
}

fn already_enabled() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_WHITELIST_ALREADY_ON,
    ))
}

fn already_disabled() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_WHITELIST_ALREADY_OFF,
    ))
}
