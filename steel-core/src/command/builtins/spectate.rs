//! Vanilla spectator camera command.

use std::sync::Arc;

use steel_protocol::packets::game::CSetCamera;
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
use crate::{
    entity::{Entity as _, SharedEntity},
    player::Player,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("spectate"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("spectate").executes(unspectate_source).then(
        argument("target", SteelArgumentType::entity())
            .executes(spectate_source_player)
            .then(argument("player", SteelArgumentType::player()).executes(spectate_target_player)),
    )
}

/// `spectate` — the source player stops spectating.
fn unspectate_source(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let player = source_player(context)?;
    spectate(context, None, &player)
}

/// `spectate <target>` — the source player spectates `target`.
fn spectate_source_player(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let player = source_player(context)?;
    let target = context.entity("target")?;
    spectate(context, Some(target), &player)
}

/// `spectate <target> <player>` — `player` spectates `target`.
fn spectate_target_player(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let player = context.player("player")?;
    let target = context.entity("target")?;
    spectate(context, Some(target), &player)
}

fn source_player(
    context: &SteelCommandContext<CommandSource>,
) -> Result<Arc<Player>, CommandSyntaxError> {
    context.source().player().cloned().ok_or_else(|| {
        CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_PLAYER,
        ))
    })
}

/// Vanilla `SpectateCommand.spectate`: `None` resets the camera to the player.
fn spectate(
    context: &SteelCommandContext<CommandSource>,
    target: Option<SharedEntity>,
    player: &Arc<Player>,
) -> Result<i32, CommandSyntaxError> {
    if let Some(target) = &target
        && target.id() == player.id()
    {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_SPECTATE_SELF,
        )));
    }
    if !player.is_spectator() {
        let message = translations::COMMANDS_SPECTATE_NOT_SPECTATOR
            .message([player.display_name()])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }
    if let Some(target) = &target
        && target.entity_type().client_tracking_range == 0
    {
        let message = translations::COMMANDS_SPECTATE_CANNOT_SPECTATE
            .message([target.display_name()])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }

    // Vanilla `ServerPlayer.setCamera(null)` restores the player's own camera.
    let camera_id = target
        .as_ref()
        .map_or_else(|| player.id(), |target| target.id());
    player.send_packet(CSetCamera { camera_id });

    let message = match &target {
        Some(target) => translations::COMMANDS_SPECTATE_SUCCESS_STARTED
            .message([target.display_name()])
            .component(),
        None => TextComponent::from(&translations::COMMANDS_SPECTATE_SUCCESS_STOPPED),
    };
    context.source().send_success(&message, false);
    Ok(1)
}
