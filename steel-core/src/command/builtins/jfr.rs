//! Vanilla `/jfr` command.
//!
//! Mirrors `JfrCommand`: `/jfr start` begins a JFR recording and `/jfr stop`
//! writes and reports the recording file. Both forms require `LEVEL_OWNERS`.
//!
//! Deliberate difference: JFR is a JVM facility with no Rust counterpart, so
//! Steel behaves exactly like vanilla on a JVM where Flight Recorder cannot
//! start — `/jfr start` reports `commands.jfr.start.failed` and `/jfr stop`
//! reports `commands.jfr.dump.failed`. The vanilla success messages
//! (`commands.jfr.started`, `commands.jfr.stopped`) are unreachable because no
//! recording can exist. `/debug` and `/perf` provide Steel's profiling surface.

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, literal},
    registration::CommandRegistration,
};
use crate::permission::PermissionLevel;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("jfr"), |_| command())
        .at_level(PermissionLevel::Owners)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("jfr")
        .then(literal("start").executes(start_recording))
        .then(literal("stop").executes(stop_recording))
}

/// Vanilla `JfrCommand.startJfr` when `JvmProfiler.start` refuses to start.
fn start_recording(
    _context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    Err(start_failed())
}

/// Vanilla `JfrCommand.stopJfr` when no recording can be dumped.
fn stop_recording(
    _context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    Err(dump_failed())
}

fn start_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_JFR_START_FAILED,
    ))
}

/// `commands.jfr.dump.failed` carries the failure reason.
fn dump_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_JFR_DUMP_FAILED
            .message([TextComponent::plain(
                "Flight Recorder is not available on this server",
            )])
            .component(),
    )
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use crate::command::builtins::create_dispatcher;

    #[test]
    fn jfr_graph_exposes_start_and_stop() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let root = dispatcher.children(dispatcher.root()).and_then(|children| {
            children.iter().copied().find(|child| {
                dispatcher
                    .node(*child)
                    .is_some_and(|node| node.name() == "jfr")
            })
        });
        let Some(root) = root else {
            panic!("jfr root should exist");
        };
        let names = dispatcher
            .children(root)
            .map(|children| {
                children
                    .iter()
                    .filter_map(|child| dispatcher.node(*child))
                    .map(|node| {
                        assert!(node.is_executable());
                        node.name()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert_eq!(names, ["start", "stop"]);
    }
}
