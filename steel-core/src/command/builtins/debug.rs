//! Vanilla debug command.
//!
//! Steel has no sampling profiler, so `debug start`/`debug stop` measure the
//! tick window with the live server tick counter and wall clock instead. The
//! command surface, messages and result values match vanilla's
//! `DebugCommand.start`/`DebugCommand.stop`.

use std::time::Instant;

use steel_utils::locks::SyncMutex;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, literal},
    registration::CommandRegistration,
};

/// An in-progress `/debug start` measurement window.
#[derive(Debug, Clone, Copy)]
struct ProfilerWindow {
    /// Wall-clock instant the window started.
    started: Instant,
    /// Server tick count when the window started.
    start_tick: u64,
}

/// The server-wide `/debug start` window.
static WINDOW: SyncMutex<Option<ProfilerWindow>> = SyncMutex::new(None);

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("debug"), |_| command())
        .subcommand_permission(["start"])
        .subcommand_permission(["stop"])
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("debug")
        .then(literal("start").executes(start_profiler))
        .then(literal("stop").executes(stop_profiler))
}

/// Vanilla `DebugCommand.start`.
fn start_profiler(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let mut window = WINDOW.lock();
    if window.is_some() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DEBUG_ALREADY_RUNNING
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
        &TextComponent::from(&translations::COMMANDS_DEBUG_STARTED),
        true,
    );
    Ok(0)
}

/// Vanilla `DebugCommand.stop`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the reported ticks-per-second is truncated into the command result like vanilla's (int) cast"
)]
fn stop_profiler(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let Some(window) = WINDOW.lock().take() else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DEBUG_NOT_RUNNING.msg().component(),
        ));
    };

    let elapsed = window.started.elapsed().as_secs_f64();
    let ticks = context
        .source()
        .server()
        .current_tick()
        .saturating_sub(window.start_tick);
    let ticks_per_second = if elapsed > 0.0 {
        ticks as f64 / elapsed
    } else {
        0.0
    };

    let message = translations::COMMANDS_DEBUG_STOPPED
        .message([
            format!("{elapsed:.2}"),
            ticks.to_string(),
            format!("{ticks_per_second:.2}"),
        ])
        .component();
    context.source().send_success(&message, true);
    Ok(ticks_per_second as i32)
}
