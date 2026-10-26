//! Vanilla `/reload` command.
//!
//! Mirrors `ReloadCommand`: re-reads every enabled data pack and re-runs the
//! reload listeners, then reports `commands.reload.success`; a listener failure
//! reports `commands.reload.failure` and leaves the previous state installed. On
//! success it also pushes the reloaded state to every client
//! ([`datapack::resend_clients`]), matching `PlayerList.reloadResources`.
//!
//! Vanilla runs the reload on the server thread through
//! `MinecraftServer.reloadResources`, so a heavy pack can hitch one tick. Steel
//! calls [`DatapackManager::reload`] from the command executor for the same
//! reason: the registries it publishes are read by the tick, and publishing them
//! from another thread would race chunk and entity work.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, literal},
    registration::CommandRegistration,
};
use crate::{datapack, permission::PermissionLevel};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("reload"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("reload").executes(reload)
}

/// Vanilla `ReloadCommand`'s executor.
fn reload(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    source.send_success(
        &TextComponent::from(&translations::COMMANDS_RELOAD_SUCCESS),
        true,
    );

    match datapack::datapacks().reload() {
        Ok(summary) => {
            tracing::info!(
                packs = summary.packs_loaded,
                pack_ids = ?summary.pack_ids,
                new_packs = ?summary.new_packs,
                functions = summary.functions_loaded,
                tags = summary.tags_loaded,
                elapsed = ?summary.elapsed,
                "Reloaded data packs"
            );
            datapack::resend_clients(source.server());
            // Vanilla returns 0: the reload itself is asynchronous feedback.
            Ok(0)
        }
        Err(error) => {
            log::warn!("Failed to execute reload: {error}");
            Err(CommandSyntaxError::dynamic(TextComponent::from(
                &translations::COMMANDS_RELOAD_FAILURE,
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use crate::command::builtins::create_dispatcher;

    #[test]
    fn reload_root_is_executable() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let root = dispatcher.children(dispatcher.root()).and_then(|children| {
            children.iter().copied().find(|child| {
                dispatcher
                    .node(*child)
                    .is_some_and(|node| node.name() == "reload")
            })
        });
        let Some(root) = root else {
            panic!("reload root should exist");
        };
        let Some(node) = dispatcher.node(root) else {
            panic!("reload root should exist");
        };
        assert!(node.is_executable());
        assert!(
            dispatcher.children(root).is_some_and(<[_]>::is_empty),
            "vanilla `/reload` is a bare literal with no subcommands"
        );
    }
}
