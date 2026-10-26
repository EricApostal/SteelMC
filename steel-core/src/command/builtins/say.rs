//! Vanilla `/say` broadcast command.
//!
//! Mirrors `SayCommand`: the message is broadcast to every online player with
//! the `minecraft:say_command` chat type, which the client renders through
//! `chat.type.announcement` when the source has no player and the source is
//! logged to the server chat log.

use steel_protocol::packets::game::{CDisguisedChat, ChatTypeBound};
use steel_registry::{RegistryEntry, vanilla_chat_types};
use steel_utils::text::DisplayResolutor;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::{entity::Entity, permission::PermissionLevel, scoreboard::ScoreHolder};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("say"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("say").then(argument("message", SteelArgumentType::message()).executes(say))
}

fn say(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let text = context.message("message")?;
    // Vanilla `PlayerList.broadcastChatMessage` logs the decorated message.
    let logged = translations::CHAT_TYPE_ANNOUNCEMENT
        .message([source_display_name(source), text.clone()])
        .component();
    broadcast_command_chat(
        source,
        &text,
        vanilla_chat_types::SAY_COMMAND.id() as i32,
        &logged,
    );
    Ok(1)
}

/// Returns the vanilla display name of a command source.
///
/// Mirrors `CommandSourceStack.getDisplayName`: entity sources use
/// [`Entity::display_name`] with their team formatting applied, and entity-less
/// sources use the literal `Server`/`Rcon` name vanilla gives them.
pub(super) fn source_display_name(source: &CommandSource) -> TextComponent {
    source.entity().map_or_else(
        || TextComponent::plain(source.sender().to_string()),
        |entity| entity_display_name(source, entity.as_ref()),
    )
}

/// Mirrors `Entity.getDisplayName`, including the holder's team formatting.
pub(super) fn entity_display_name(source: &CommandSource, entity: &dyn Entity) -> TextComponent {
    let display_name = entity.display_name();
    let Some(scoreboard) = source.server().scoreboards.get(source.world().domain()) else {
        return display_name;
    };
    scoreboard.formatted_holder_name(&ScoreHolder::with_display_name(
        entity.scoreboard_name(),
        Some(display_name),
    ))
}

/// Builds vanilla's `ChatType.Bound` for a registry chat type.
pub(super) const fn bound_chat_type(
    chat_type_id: i32,
    sender_name: TextComponent,
    target_name: Option<TextComponent>,
) -> ChatTypeBound {
    ChatTypeBound {
        registry_id: chat_type_id,
        sender_name,
        target_name,
    }
}

/// Mirrors `PlayerList.broadcastChatMessage` for a command chat type.
///
/// `logged` is vanilla's server-side rendering of the same message, which is
/// written to the chat log because the decorated line is normally rendered by
/// the client.
pub(super) fn broadcast_command_chat(
    source: &CommandSource,
    content: &TextComponent,
    chat_type_id: i32,
    logged: &TextComponent,
) {
    let sender_name = source_display_name(source);
    let chat_type = bound_chat_type(chat_type_id, sender_name, None);
    for player in source.server().get_players() {
        player.send_packet(CDisguisedChat::new(
            content,
            chat_type.clone(),
            player.as_ref(),
        ));
    }
    steel_utils::chat!(
        source_text_name(source),
        "{}",
        logged.to_plain(&DisplayResolutor)
    );
}

/// Returns the plain text name vanilla uses for a source in log output.
pub(super) fn source_text_name(source: &CommandSource) -> String {
    source.entity().map_or_else(
        || source.sender().to_string(),
        |entity| entity.plain_text_name(),
    )
}
