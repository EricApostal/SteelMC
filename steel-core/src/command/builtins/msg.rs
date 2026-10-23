//! Vanilla `/msg` direct-message command.
//!
//! Mirrors `MsgCommand` (registered as `msg`, `tell`, and `w`). The sender sees
//! the `minecraft:msg_command_outgoing` chat type, naming the target, and each
//! target sees `minecraft:msg_command_incoming`, naming the sender.

use steel_protocol::packets::game::{CDisguisedChat, ChatTypeBound};
use steel_registry::{RegistryEntry, vanilla_chat_types};
use steel_utils::{Identifier, translations};
use text_components::{Modifier, TextComponent, format::Color};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use super::say::{bound_chat_type, entity_display_name, source_display_name};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("msg"), |_| command())
        .alias("tell")
        .alias("w")
        .default_access()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("msg").then(
        argument("targets", SteelArgumentType::players())
            .then(argument("message", SteelArgumentType::message()).executes(send_message)),
    )
}

fn send_message(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;
    let Ok(result) = i32::try_from(targets.len()) else {
        return Err(CommandSyntaxError::dynamic(
            "Target player count exceeds the command result range",
        ));
    };

    let source = context.source();
    let content = context.message("message")?;
    let sender_name = source_display_name(source);
    let incoming_chat_type = bound_chat_type(
        vanilla_chat_types::MSG_COMMAND_INCOMING.id() as i32,
        sender_name.clone(),
        None,
    );
    let outgoing_chat_type_id = vanilla_chat_types::MSG_COMMAND_OUTGOING.id() as i32;

    for target in &targets {
        let target_name = entity_display_name(source, target.as_ref());
        let outgoing_chat_type = bound_chat_type(
            outgoing_chat_type_id,
            sender_name.clone(),
            Some(target_name.clone()),
        );
        send_outgoing(source, &content, outgoing_chat_type, &target_name);
        target.send_packet(CDisguisedChat::new(
            &content,
            incoming_chat_type.clone(),
            target.as_ref(),
        ));
    }

    Ok(result)
}

/// Mirrors `CommandSourceStack.sendChatMessage`.
///
/// A player source receives the outgoing chat type as a chat packet; an
/// entity-less source renders the same decoration into its own output.
fn send_outgoing(
    source: &CommandSource,
    content: &TextComponent,
    outgoing_chat_type: ChatTypeBound,
    target_name: &TextComponent,
) {
    if let Some(player) = source.player() {
        if !source.is_silent() {
            player.send_packet(CDisguisedChat::new(
                content,
                outgoing_chat_type,
                player.as_ref(),
            ));
        }
        return;
    }
    source.send_system_message(&decorated_outgoing(target_name, content));
}

/// Vanilla `ChatTypeDecoration.decorate` for `MSG_COMMAND_OUTGOING`.
fn decorated_outgoing(target_name: &TextComponent, content: &TextComponent) -> TextComponent {
    translations::COMMANDS_MESSAGE_DISPLAY_OUTGOING
        .message([target_name.clone(), content.clone()])
        .component()
        .color(Color::Gray)
        .italic(true)
}
