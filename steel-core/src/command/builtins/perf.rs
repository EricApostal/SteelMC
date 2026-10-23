//! Vanilla metrics-profiling command.
//!
//! Steel has no JFR/Minecraft-metrics recorder, so `perf start`/`perf stop`
//! measure the tick window with the live server tick counter and wall clock —
//! the same mechanism as [`super::debug`]. The command surface, messages and
//! result values match vanilla's `PerfCommand`.

use std::time::Instant;

use steel_utils::locks::SyncMutex;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, literal},
    registration::CommandRegistration,
};
use crate::permission::PermissionLevel;

/// An in-progress `/perf start` measurement window.
#[derive(Debug, Clone, Copy)]
struct ProfilerWindow {
    /// Wall-clock instant the window started.
    started: Instant,
    /// Server tick count when the window started.
    start_tick: u64,
}

/// The server-wide `/perf start` window.
static WINDOW: SyncMutex<Option<ProfilerWindow>> = SyncMutex::new(None);

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("perf"), |_| command())
        .at_level(PermissionLevel::Owners)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("perf")
        .then(literal("start").executes(start_profiling))
        .then(literal("stop").executes(stop_profiling))
}

/// Vanilla `PerfCommand.startProfilingDedicatedServer`.
fn start_profiling(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let mut window = WINDOW.lock();
    if window.is_some() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_PERF_ALREADY_RUNNING
                .msg()
                .component(),
        ));
    }

    *window = Some(ProfilerWindow {
        started: Instant::now(),
        start_tick: context.source().server().current_tick(),
    });
    drop(window);
    context.source().send_success(
        &TextComponent::from(&translations::COMMANDS_PERF_STARTED),
        false,
    );
    Ok(0)
}

/// Vanilla `PerfCommand.stopProfilingDedicatedServer`.
fn stop_profiling(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let Some(window) = WINDOW.lock().take() else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_PERF_NOT_RUNNING.msg().component(),
        ));
    };

    let elapsed = window.started.elapsed().as_secs_f64();
    let ticks = context
        .source()
        .server()
        .current_tick()
        .saturating_sub(window.start_tick);

    if ticks > 0 {
        let message = translations::COMMANDS_PERF_STOPPED
            .message([
                format!("{elapsed:.2}"),
                ticks.to_string(),
                format!("{:.2}", ticks as f64 / elapsed.max(f64::EPSILON)),
            ])
            .component();
        context.source().send_success(&message, false);
    }
    Ok(0)
}
