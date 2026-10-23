//! Vanilla `/kick` command.
//!
//! Mirrors `KickCommand`: disconnects the target players with the given
//! reason component (default `multiplayer.disconnect.kicked`), then reports
//! each kick back to the source with `commands.kick.success`.
//!
//! Vanilla guards against kicking outside a published server and against
//! kicking the singleplayer owner; Steel is always dedicated (the server is
//! always "published" and there is no singleplayer owner), so both guards are
//! vacuous here. They are documented here for parity rather than checked.

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
use crate::{entity::Entity, permission::PermissionLevel};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("kick"), |_| command())
        .at_level(PermissionLevel::Admins)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("kick").then(
        argument("targets", SteelArgumentType::players())
            .executes(|context| kick(context, default_reason()))
            .then(
                argument("reason", SteelArgumentType::message())
                    .executes(|context| kick(context, context.message("reason")?)),
            ),
    )
}

/// Vanilla's default kick reason.
fn default_reason() -> TextComponent {
    TextComponent::from(&translations::MULTIPLAYER_DISCONNECT_KICKED)
}

fn kick(
    context: &SteelCommandContext<CommandSource>,
    reason: TextComponent,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;
    let Ok(result) = i32::try_from(targets.len()) else {
        return Err(CommandSyntaxError::dynamic(
            "Target player count exceeds the command result range",
        ));
    };

    let message = reason.clone();
    for target in &targets {
        // Vanilla calls `ServerPlayer.connection.disconnect(reason)` here;
        // Steel's equivalent is `Player::disconnect`.
        target.disconnect(message.clone());
    }

    for target in &targets {
        context.source().send_success(
            &translations::COMMANDS_KICK_SUCCESS
                .message([target.display_name(), reason.clone()])
                .component(),
            true,
        );
    }
    Ok(result)
}
