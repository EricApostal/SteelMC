//! Vanilla player and address ban commands.
//!
//! Mirrors `BanPlayerCommands`, `BanIpCommands`, `BanListCommands`,
//! `PardonCommand`, and `PardonIpCommand`, backed by the ban lists in
//! [`crate::server::player_lists`] through `Server::bans()` and
//! `Server::ip_bans()`.

use std::{future::Future, net::IpAddr, str::FromStr, sync::Arc};

use steel_utils::{Identifier, translations};
use text_components::TextComponent;
use tokio::{sync::oneshot, task::JoinHandle};
use uuid::Uuid;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandResultSuspension, CommandResultSuspensionPoll, CommandSource,
        CommandSuspensionOrder, GameProfileArgument, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, SteelSuggestionContext, argument, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use crate::{
    permission::PermissionLevel,
    player::Player,
    server::{
        Server,
        player_lists::{IpBanEntry, UserBanEntry, ip_from_socket_address},
    },
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("ban"), |_| ban_command())
        .at_level(PermissionLevel::Admins)
}

pub(super) fn ban_ip_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("ban-ip"), |_| ban_ip_command())
        .at_level(PermissionLevel::Admins)
}

pub(super) fn ban_list_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("banlist"), |_| {
        ban_list_command()
    })
    .at_level(PermissionLevel::Admins)
}

pub(super) fn pardon_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("pardon"), |_| pardon_command())
        .at_level(PermissionLevel::Admins)
}

pub(super) fn pardon_ip_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("pardon-ip"), |_| {
        pardon_ip_command()
    })
    .at_level(PermissionLevel::Admins)
}

fn ban_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("ban").then(
        argument("targets", SteelArgumentType::game_profile())
            .executes_suspended(|context| start_ban(context, None))
            .then(
                argument("reason", ArgumentType::greedy_string()).executes_suspended(|context| {
                    let reason = context.string("reason")?.to_owned();
                    start_ban(context, Some(reason))
                }),
            ),
    )
}

fn ban_ip_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("ban-ip").then(
        argument("target", ArgumentType::word())
            .executes_suspended(|context| start_ban_ip(context, None))
            .then(
                argument("reason", ArgumentType::greedy_string()).executes_suspended(|context| {
                    let reason = context.string("reason")?.to_owned();
                    start_ban_ip(context, Some(reason))
                }),
            ),
    )
}

fn ban_list_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("banlist")
        .executes(|context| show_ban_list(context, BanListFilter::All))
        .then(literal("ips").executes(|context| show_ban_list(context, BanListFilter::Addresses)))
        .then(literal("players").executes(|context| show_ban_list(context, BanListFilter::Players)))
}

fn pardon_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("pardon").then(
        argument("targets", SteelArgumentType::game_profile())
            .suggests(
                |context: &SteelSuggestionContext<'_, CommandSource>,
                 builder: &mut SuggestionsBuilder<'_>| {
                    suggest_list(builder, context.source().server().bans().names());
                },
            )
            .executes_suspended(start_pardon),
    )
}

fn pardon_ip_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("pardon-ip").then(
        argument("target", ArgumentType::word())
            .suggests(
                |context: &SteelSuggestionContext<'_, CommandSource>,
                 builder: &mut SuggestionsBuilder<'_>| {
                    suggest_list(builder, context.source().server().ip_bans().user_list());
                },
            )
            .executes_suspended(start_pardon_ip),
    )
}

/// One command outcome produced by asynchronous moderation work.
///
/// Shared with `whitelist.rs`, which performs the same
/// resolve-then-mutate shape for its profile arguments.
pub(super) struct ModerationOutcome {
    /// Success messages for the command source, in vanilla order.
    pub(super) messages: Vec<TextComponent>,
    /// Vanilla command result.
    pub(super) result: i32,
}

/// A command result produced by a spawned asynchronous task.
pub(super) struct ModerationSuspension {
    source: CommandSource,
    receiver: oneshot::Receiver<Result<ModerationOutcome, CommandSyntaxError>>,
    task: Option<JoinHandle<()>>,
}

impl ModerationSuspension {
    /// Runs `work` on the async runtime and reports its result on a later tick.
    pub(super) fn spawn(
        source: CommandSource,
        work: impl Future<Output = Result<ModerationOutcome, CommandSyntaxError>> + Send + 'static,
    ) -> Self {
        let (sender, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = sender.send(work.await);
        });
        Self {
            source,
            receiver,
            task: Some(task),
        }
    }
}

impl CommandResultSuspension for ModerationSuspension {
    fn order(&self) -> CommandSuspensionOrder {
        CommandSuspensionOrder::Global
    }

    fn poll(&mut self) -> CommandResultSuspensionPoll {
        match self.receiver.try_recv() {
            Ok(result) => {
                self.task = None;
                CommandResultSuspensionPoll::Ready(result.map(|outcome| {
                    for message in &outcome.messages {
                        self.source.send_success(message, true);
                    }
                    outcome.result
                }))
            }
            Err(oneshot::error::TryRecvError::Empty) => CommandResultSuspensionPoll::Pending,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.task = None;
                CommandResultSuspensionPoll::Ready(Err(CommandSyntaxError::dynamic(
                    "Ban command task ended without a result",
                )))
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn start_ban(
    context: &SteelCommandContext<CommandSource>,
    reason: Option<String>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let targets = context.game_profile_argument("targets")?.clone();
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        ban_players(&source, targets, reason).await
    }))
}

/// Vanilla `BanPlayerCommands.banPlayers`.
async fn ban_players(
    source: &CommandSource,
    targets: GameProfileArgument,
    reason: Option<String>,
) -> Result<ModerationOutcome, CommandSyntaxError> {
    let profiles = targets.resolve(source).await?;
    let server = source.server();
    let source_name = text_name(source);
    let mut messages = Vec::new();
    let mut count = 0i32;

    for profile in profiles {
        if server.bans().is_banned(profile.uuid) {
            continue;
        }
        let entry = UserBanEntry::new(
            profile.uuid,
            profile.name.clone(),
            None,
            source_name.clone(),
            None,
            reason.clone(),
        );
        let entry_reason = entry.reason_message();
        if server.bans().add(entry).await {
            count += 1;
            messages.push(
                translations::COMMANDS_BAN_SUCCESS
                    .message([TextComponent::plain(profile.name.clone()), entry_reason])
                    .component(),
            );
            if let Some(player) = online_player(server, profile.uuid) {
                player.disconnect(translations::MULTIPLAYER_DISCONNECT_BANNED.msg());
            }
        }
    }

    if count == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BAN_FAILED,
        )));
    }
    Ok(ModerationOutcome {
        messages,
        result: count,
    })
}

fn start_pardon(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let targets = context.game_profile_argument("targets")?.clone();
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        pardon_players(&source, targets).await
    }))
}

/// Vanilla `PardonCommand.pardonPlayers`.
async fn pardon_players(
    source: &CommandSource,
    targets: GameProfileArgument,
) -> Result<ModerationOutcome, CommandSyntaxError> {
    let profiles = targets.resolve(source).await?;
    let server = source.server();
    let mut messages = Vec::new();
    let mut count = 0i32;

    for profile in profiles {
        if !server.bans().is_banned(profile.uuid) {
            continue;
        }
        if server.bans().remove(profile.uuid).await {
            count += 1;
            messages.push(
                translations::COMMANDS_PARDON_SUCCESS
                    .message([TextComponent::plain(profile.name.clone())])
                    .component(),
            );
        }
    }

    if count == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PARDON_FAILED,
        )));
    }
    Ok(ModerationOutcome {
        messages,
        result: count,
    })
}

fn start_ban_ip(
    context: &SteelCommandContext<CommandSource>,
    reason: Option<String>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let target = context.string("target")?.to_owned();
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        ban_ip_or_name(&source, &target, reason).await
    }))
}

/// Vanilla `BanIpCommands.banIpOrName`.
async fn ban_ip_or_name(
    source: &CommandSource,
    target: &str,
    reason: Option<String>,
) -> Result<ModerationOutcome, CommandSyntaxError> {
    if IpAddr::from_str(target).is_ok() {
        return ban_ip(source, target, reason.as_deref()).await;
    }

    let server = source.server();
    let Some(player) = online_player_by_name(server, target) else {
        return Err(invalid_ip());
    };
    let Some(address) = player_address(&player) else {
        return Err(invalid_ip());
    };
    ban_ip(source, &address, reason.as_deref()).await
}

/// Vanilla `BanIpCommands.banIp`.
async fn ban_ip(
    source: &CommandSource,
    address: &str,
    reason: Option<&str>,
) -> Result<ModerationOutcome, CommandSyntaxError> {
    let server = source.server();
    if server.ip_bans().is_banned_ip(address) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BANIP_FAILED,
        )));
    }

    let players = players_with_address(server, address);
    let entry = IpBanEntry::new(
        address.to_owned(),
        None,
        text_name(source),
        None,
        reason.map(str::to_owned),
    );
    let entry_reason = entry.reason_message();
    server.ip_bans().add(entry).await;

    let mut messages = vec![
        translations::COMMANDS_BANIP_SUCCESS
            .message([TextComponent::plain(address.to_owned()), entry_reason])
            .component(),
    ];
    if !players.is_empty() {
        messages.push(
            translations::COMMANDS_BANIP_INFO
                .message([
                    TextComponent::plain(players.len().to_string()),
                    TextComponent::plain(joined_names(&players)),
                ])
                .component(),
        );
    }

    for player in &players {
        player.disconnect(translations::MULTIPLAYER_DISCONNECT_IP_BANNED.msg());
    }

    Ok(ModerationOutcome {
        messages,
        result: count_result(players.len())?,
    })
}

fn start_pardon_ip(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ModerationSuspension, CommandSyntaxError> {
    let target = context.string("target")?.to_owned();
    let source = context.source().clone();
    Ok(ModerationSuspension::spawn(source.clone(), async move {
        pardon_ip(&source, &target).await
    }))
}

/// Vanilla `PardonIpCommand.unban`.
async fn pardon_ip(
    source: &CommandSource,
    target: &str,
) -> Result<ModerationOutcome, CommandSyntaxError> {
    if IpAddr::from_str(target).is_err() {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PARDONIP_INVALID,
        )));
    }

    let server = source.server();
    if !server.ip_bans().is_banned_ip(target) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PARDONIP_FAILED,
        )));
    }

    server.ip_bans().remove(target).await;
    Ok(ModerationOutcome {
        messages: vec![
            translations::COMMANDS_PARDONIP_SUCCESS
                .message([TextComponent::plain(target.to_owned())])
                .component(),
        ],
        result: 1,
    })
}

#[derive(Clone, Copy)]
enum BanListFilter {
    All,
    Addresses,
    Players,
}

/// Vanilla `BanListCommands.showList`.
fn show_ban_list(
    context: &SteelCommandContext<CommandSource>,
    filter: BanListFilter,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let server = source.server();
    let mut entries = Vec::new();
    if matches!(filter, BanListFilter::All | BanListFilter::Players) {
        entries.extend(server.bans().entries().into_iter().map(|entry| {
            (
                entry.display_name(),
                entry.source().to_owned(),
                entry.reason_message(),
            )
        }));
    }
    if matches!(filter, BanListFilter::All | BanListFilter::Addresses) {
        entries.extend(server.ip_bans().entries().into_iter().map(|entry| {
            (
                entry.display_name(),
                entry.source().to_owned(),
                entry.reason_message(),
            )
        }));
    }

    if entries.is_empty() {
        let message = TextComponent::from(&translations::COMMANDS_BANLIST_NONE);
        source.send_success(&message, false);
    } else {
        let message = translations::COMMANDS_BANLIST_LIST
            .message([TextComponent::plain(entries.len().to_string())])
            .component();
        source.send_success(&message, false);

        for (display_name, entry_source, reason) in &entries {
            let message = translations::COMMANDS_BANLIST_ENTRY
                .message([
                    display_name.clone(),
                    TextComponent::plain(entry_source.clone()),
                    reason.clone(),
                ])
                .component();
            source.send_success(&message, false);
        }
    }

    count_result(entries.len())
}

fn invalid_ip() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(&translations::COMMANDS_BANIP_INVALID))
}

/// Vanilla `CommandSourceStack.getTextName`.
fn text_name(source: &CommandSource) -> String {
    source.player().map_or_else(
        || "Server".to_owned(),
        |player| player.gameprofile.name().to_owned(),
    )
}

fn online_player(server: &Arc<Server>, uuid: Uuid) -> Option<Arc<Player>> {
    server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.id() == uuid)
}

/// Vanilla `PlayerList.getPlayerByName`.
fn online_player_by_name(server: &Arc<Server>, name: &str) -> Option<Arc<Player>> {
    server
        .get_players()
        .into_iter()
        .find(|player| player.gameprofile.name().eq_ignore_ascii_case(name))
}

/// Vanilla `PlayerList.getPlayersWithAddress`.
fn players_with_address(server: &Arc<Server>, address: &str) -> Vec<Arc<Player>> {
    let mut players = Vec::new();
    for player in server.get_players() {
        if player_address(&player).as_deref() == Some(address) {
            players.push(player);
        }
    }
    players
}

/// Vanilla `ServerPlayer.getIpAddress`.
///
/// Missing dependency: `Player` never records the accepted connection's remote
/// address, so `Player::remote_address(&self) -> Option<SocketAddr>` does not
/// exist yet. Owned by the listener/session lane, not the ban storage lane.
fn player_address(player: &Player) -> Option<String> {
    player
        .remote_address()
        .map(|address| ip_from_socket_address(&address))
}

/// Vanilla `EntitySelector.joinNames`.
fn joined_names(players: &[Arc<Player>]) -> String {
    players
        .iter()
        .map(|player| player.gameprofile.name().to_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Converts a collected count to the vanilla command result range.
pub(super) fn count_result(count: usize) -> Result<i32, CommandSyntaxError> {
    i32::try_from(count).map_err(|_| {
        CommandSyntaxError::dynamic("Ban entry count exceeds the command result range")
    })
}
