//! Vanilla `/teammsg` team-message command.
//!
//! Mirrors `TeamMsgCommand` (with the `tm` alias). The message reaches the
//! sender and every online player on the sender entity's team, using the
//! `minecraft:team_msg_command_incoming` chat type for other team members and
//! `minecraft:team_msg_command_outgoing` for the sender. When the sender is no
//! entity or not on a team, vanilla fails with `commands.teammsg.failed.noteam`.

use steel_protocol::packets::game::CDisguisedChat;
use steel_registry::{RegistryEntry, vanilla_chat_types};
use steel_utils::{Identifier, translations};
use text_components::{
    Modifier as _, TextComponent,
    interactivity::{ClickEvent, HoverEvent},
};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use super::say::{bound_chat_type, source_display_name};
use crate::{entity::Entity, scoreboard::ScoreHolder};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("teammsg"), |_| command())
        .alias("tm")
        .default_access()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("teammsg").then(argument("message", SteelArgumentType::message()).executes(teammsg))
}

fn teammsg(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let self_entity = source.entity().cloned().ok_or_else(|| {
        CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_ENTITY,
        ))
    })?;
    let team_name = entity_team_name(source, self_entity.as_ref()).ok_or_else(|| {
        CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_TEAMMSG_FAILED_NOTEAM,
        ))
    })?;
    let text = context.message("message")?;

    let sender_name = source_display_name(source);
    let shown_team = suggestive_team_name(source, &team_name);
    let incoming_chat_type = bound_chat_type(
        vanilla_chat_types::TEAM_MSG_COMMAND_INCOMING.id() as i32,
        sender_name.clone(),
        Some(shown_team.clone()),
    );
    let outgoing_chat_type = bound_chat_type(
        vanilla_chat_types::TEAM_MSG_COMMAND_OUTGOING.id() as i32,
        sender_name,
        Some(shown_team),
    );

    let receivers = source
        .server()
        .get_players()
        .into_iter()
        .filter(|player| {
            self_entity.uuid() == player.uuid()
                || entity_team_name(source, player.as_ref()).as_deref() == Some(team_name.as_str())
        })
        .collect::<Vec<_>>();
    let Ok(result) = i32::try_from(receivers.len()) else {
        return Err(CommandSyntaxError::dynamic(
            "Team message receiver count exceeds the command result range",
        ));
    };

    for receiver in &receivers {
        let is_sender = self_entity.uuid() == receiver.uuid();
        let chat_type = if is_sender {
            outgoing_chat_type.clone()
        } else {
            incoming_chat_type.clone()
        };
        receiver.send_packet(CDisguisedChat::new(&text, chat_type, receiver.as_ref()));
    }

    Ok(result)
}

/// Returns the scoreboard team holding an entity in this domain.
fn entity_team_name(source: &CommandSource, entity: &dyn Entity) -> Option<String> {
    source
        .server()
        .scoreboards
        .get(source.world().domain())?
        .holder_team_name(&ScoreHolder::new(entity.scoreboard_name()))
}

/// Returns the team display name with vanilla's hover/click suggestion style.
///
/// Mirrors `TeamMsgCommand.SUGGEST_STYLE`: hover `chat.type.team.hover` and a
/// `/teammsg ` suggestion click.
fn suggestive_team_name(source: &CommandSource, team_name: &str) -> TextComponent {
    source
        .server()
        .scoreboards
        .get(source.world().domain())
        .and_then(|scoreboard| scoreboard.team_display_name(team_name))
        .unwrap_or_else(|| TextComponent::plain(team_name.to_owned()))
        .hover_event(HoverEvent::show_text(&translations::CHAT_TYPE_TEAM_HOVER))
        .click_event(ClickEvent::suggest_command("/teammsg "))
}
