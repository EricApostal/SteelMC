//! Vanilla `/me` emote command.
//!
//! Mirrors `EmoteCommands`: the action is broadcast to every online player with
//! the `minecraft:emote_command` chat type, which the client renders through
//! `chat.type.emote`.

use steel_registry::{RegistryEntry, vanilla_chat_types};
use steel_utils::{Identifier, translations};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use super::say::{broadcast_command_chat, source_display_name};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("me"), |_| command()).default_access()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("me").then(argument("action", SteelArgumentType::message()).executes(emote))
}

fn emote(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let text = context.message("action")?;
    let logged = translations::CHAT_TYPE_EMOTE
        .message([source_display_name(source), text.clone()])
        .component();
    broadcast_command_chat(
        source,
        &text,
        vanilla_chat_types::EMOTE_COMMAND.id() as i32,
        &logged,
    );
    Ok(1)
}
