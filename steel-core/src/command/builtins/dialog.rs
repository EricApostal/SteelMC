//! Vanilla dialog command.
//!
//! Mirrors `DialogCommand`: `dialog show <targets> <dialog>` sends
//! `ClientboundShowDialogPacket` to every target and `dialog clear <targets>`
//! sends `ClientboundClearDialogPacket`, with vanilla's
//! `CommandResponseTracker` single/multiple feedback and result.
//!
//! Vanilla's `dialog` argument is a `ResourceOrIdArgument`, i.e. it accepts
//! either a `minecraft:dialog` registry key or an inline dialog definition.
//! Steel's `steel-registry` `Dialog` model only decodes the `dialog_list` and
//! `server_links` variants, so only the registry-reference form can be parsed
//! here; inline definitions fail with the standard unknown-resource error until
//! the full dialog codec lands in `steel-registry`.

use std::sync::Arc;

use steel_protocol::packets::common::{CClearDialog, CShowDialog};
use steel_utils::{Identifier, translations};
use text_components::{TextComponent, translation::Translation};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity;
use crate::permission::PermissionLevel;
use crate::player::Player;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("dialog"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("dialog")
        .then(
            literal("show").then(
                argument("targets", SteelArgumentType::players())
                    .then(argument("dialog", SteelArgumentType::dialog()).executes(show_dialog)),
            ),
        )
        .then(
            literal("clear")
                .then(argument("targets", SteelArgumentType::players()).executes(clear_dialog)),
        )
}

/// Vanilla `DialogCommand.showDialog`.
fn show_dialog(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;
    let dialog = context.dialog("dialog")?;

    for target in &targets {
        target.send_packet(CShowDialog::new(dialog));
    }

    respond(
        context,
        &targets,
        &translations::COMMANDS_DIALOG_SHOW_SINGLE,
        &translations::COMMANDS_DIALOG_SHOW_MULTIPLE,
    );
    target_count(&targets)
}

/// Vanilla `DialogCommand.clearDialog`.
fn clear_dialog(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;

    for target in &targets {
        target.send_packet(CClearDialog {});
    }

    respond(
        context,
        &targets,
        &translations::COMMANDS_DIALOG_CLEAR_SINGLE,
        &translations::COMMANDS_DIALOG_CLEAR_MULTIPLE,
    );
    target_count(&targets)
}

/// Vanilla `CommandResponseTracker.sendFeedback`: the single form names the
/// target, the multiple form counts them. Both are broadcast to admins.
fn respond(
    context: &SteelCommandContext<CommandSource>,
    targets: &[Arc<Player>],
    single: &Translation<1>,
    multiple: &Translation<1>,
) {
    let message = match targets {
        [target] => single.message([target.display_name()]).component(),
        _ => multiple
            .message([TextComponent::from(targets.len().to_string())])
            .component(),
    };
    context.source().send_success(&message, true);
}

fn target_count(targets: &[Arc<Player>]) -> Result<i32, CommandSyntaxError> {
    i32::try_from(targets.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Target player count exceeds the command result range")
    })
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::super::create_dispatcher;
    use super::*;
    use crate::command::brigadier::{CommandDispatcher, NodeId};

    type Dispatcher = CommandDispatcher<CommandSource, SteelCommandRuntime>;

    fn child(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> NodeId {
        let Some(children) = dispatcher.children(parent) else {
            panic!("parent node should exist");
        };
        let Some(child) = children.iter().copied().find(|child| {
            dispatcher
                .node(*child)
                .is_some_and(|node| node.name() == name)
        }) else {
            panic!("child `{name}` should exist");
        };
        child
    }

    #[test]
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the private CommandNode type cannot be named from this module"
    )]
    fn dialog_root_matches_vanillas_show_and_clear_subtrees() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };

        let root = child(&dispatcher, dispatcher.root(), "dialog");
        assert!(
            dispatcher
                .node(root)
                .is_some_and(|node| node.is_restricted())
        );

        let show = child(&dispatcher, root, "show");
        let show_targets = child(&dispatcher, show, "targets");
        assert_eq!(
            dispatcher
                .node(show_targets)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::players())
        );
        let show_dialog = child(&dispatcher, show_targets, "dialog");
        assert_eq!(
            dispatcher
                .node(show_dialog)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::dialog())
        );
        assert!(
            dispatcher
                .node(show_dialog)
                .is_some_and(|node| node.is_executable())
        );

        let clear = child(&dispatcher, root, "clear");
        let clear_targets = child(&dispatcher, clear, "targets");
        assert_eq!(
            dispatcher
                .node(clear_targets)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::players())
        );
        assert!(
            dispatcher
                .node(clear_targets)
                .is_some_and(|node| node.is_executable())
        );
    }
}
