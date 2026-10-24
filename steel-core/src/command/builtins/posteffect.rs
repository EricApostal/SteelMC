//! Vanilla `/posteffect` command.
//!
//! Mirrors `PostEffectCommand`: `add`, `remove` and `clear` apply per-player
//! client post-processing effect identifiers (vanilla `ServerPlayer.postEffects`),
//! and `list` prints the effects currently active on a single player. Each
//! mutation immediately sends the vanilla `ClientboundPostEffectsPacket` (Steel
//! `CPostEffects`) to the affected players, and each verb follows vanilla's
//! `CommandResponseTracker` feedback: one changed player uses the `single`
//! message, several use `multiple`, and zero changes throw the verb's
//! `failed` error.

use std::sync::Arc;

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
use crate::{entity::Entity, permission::PermissionLevel, player::Player};

/// Changed targets produced by `CommandResponseTracker.track`.
type Targets = Vec<Arc<Player>>;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("posteffect"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("posteffect")
        .then(
            literal("add").then(
                argument("targets", SteelArgumentType::players()).then(
                    argument("posteffect", SteelArgumentType::identifier_argument())
                        .executes(add_post_effect),
                ),
            ),
        )
        .then(
            literal("remove").then(
                argument("targets", SteelArgumentType::players()).then(
                    argument("posteffect", SteelArgumentType::identifier_argument())
                        .executes(remove_post_effect),
                ),
            ),
        )
        .then(
            literal("clear").then(
                argument("targets", SteelArgumentType::players()).executes(clear_post_effects),
            ),
        )
        .then(list_command())
}

fn list_command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("list")
        .then(argument("target", SteelArgumentType::player()).executes(list_post_effects))
}

fn add_post_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;
    let effect = context.identifier_argument("posteffect")?.clone();

    let changed: Targets = targets
        .iter()
        .filter(|player| player.add_post_effect(&effect))
        .cloned()
        .collect();

    if changed.is_empty() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_POSTEFFECT_ADD_FAILED
                .msg()
                .component(),
        ));
    }

    let message = if changed.len() == 1 {
        translations::COMMANDS_POSTEFFECT_ADD_SUCCESS_SINGLE
            .message([
                TextComponent::plain(effect.to_string()),
                changed[0].display_name(),
            ])
            .component()
    } else {
        translations::COMMANDS_POSTEFFECT_ADD_SUCCESS_MULTIPLE
            .message([
                TextComponent::plain(effect.to_string()),
                TextComponent::from(changed.len().to_string()),
            ])
            .component()
    };
    context.source().send_success(&message, true);
    Ok(changed.len() as i32)
}

fn remove_post_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;
    let effect = context.identifier_argument("posteffect")?.clone();

    let changed: Targets = targets
        .iter()
        .filter(|player| player.remove_post_effect(&effect))
        .cloned()
        .collect();

    if changed.is_empty() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_POSTEFFECT_REMOVE_FAILED
                .msg()
                .component(),
        ));
    }

    let message = if changed.len() == 1 {
        translations::COMMANDS_POSTEFFECT_REMOVE_SUCCESS_SINGLE
            .message([
                TextComponent::plain(effect.to_string()),
                changed[0].display_name(),
            ])
            .component()
    } else {
        translations::COMMANDS_POSTEFFECT_REMOVE_SUCCESS_MULTIPLE
            .message([
                TextComponent::plain(effect.to_string()),
                TextComponent::from(changed.len().to_string()),
            ])
            .component()
    };
    context.source().send_success(&message, true);
    Ok(changed.len() as i32)
}

fn clear_post_effects(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;

    let changed: Targets = targets
        .iter()
        .filter(|player| player.clear_post_effects())
        .cloned()
        .collect();

    if changed.is_empty() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_POSTEFFECT_CLEAR_FAILED
                .msg()
                .component(),
        ));
    }

    let message = if changed.len() == 1 {
        translations::COMMANDS_POSTEFFECT_CLEAR_SUCCESS_SINGLE
            .message([changed[0].display_name()])
            .component()
    } else {
        translations::COMMANDS_POSTEFFECT_CLEAR_SUCCESS_MULTIPLE
            .message([TextComponent::from(changed.len().to_string())])
            .component()
    };
    context.source().send_success(&message, true);
    Ok(changed.len() as i32)
}

fn list_post_effects(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let target = context.player("target")?;
    let effects = target.post_effects();

    if effects.is_empty() {
        context.source().send_success(
            &translations::COMMANDS_POSTEFFECT_LIST_EMPTY
                .message([target.display_name()])
                .component(),
            true,
        );
    } else {
        let joined = effects
            .iter()
            .map(Identifier::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        context.source().send_success(
            &translations::COMMANDS_POSTEFFECT_LIST_SUCCESS
                .message([
                    target.display_name(),
                    TextComponent::from(effects.len().to_string()),
                    TextComponent::plain(joined),
                ])
                .component(),
            true,
        );
    }
    Ok(effects.len() as i32)
}
