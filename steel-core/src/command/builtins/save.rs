//! Vanilla save commands.
//!
//! `save-all [flush]` mirrors `SaveAllCommand` + `MinecraftServer.saveEverything`:
//! player data, command-owned saved data, and every loaded world's level data,
//! saved data, and dirty chunks.
//!
//! Steel's persistence path always joins its writes before the future resolves
//! (`World::cleanup`, `PlayerDataStorage::save`, `ChunkMap::save_all_chunks`), so
//! `save-all` and `save-all flush` both return only after the data reached the
//! disk-backed storage. Vanilla's non-flush form schedules the same writes and
//! returns early; Steel therefore never needs a separate flush step.
//!
//! `save-off` and `save-on` mirror `SaveOffCommand`/`SaveOnCommand`: they toggle
//! vanilla's per-level `noSave` flag (see `World::no_save`) which makes the
//! periodic autosave skip those worlds. `/save-all` is always forced, so it
//! saves `noSave` worlds anyway, exactly like vanilla.

use std::sync::Arc;

use steel_utils::{Identifier, translations};
use tokio::{runtime::Builder as RuntimeBuilder, sync::oneshot, task::spawn_blocking};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandResultSuspension, CommandResultSuspensionPoll, CommandSource,
        CommandSuspensionOrder, SteelCommandContext, SteelCommandRuntime, literal,
    },
    registration::CommandRegistration,
};
use crate::{permission::PermissionLevel, server::Server};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("save-all"), |_| command())
        .at_level(PermissionLevel::Owners)
}

/// Vanilla `SaveOffCommand`: disables the periodic autosave on every world.
pub(super) fn save_off_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("save-off"), |_| {
        literal("save-off").executes(save_off)
    })
    .at_level(PermissionLevel::Owners)
}

/// Vanilla `SaveOnCommand`: re-enables the periodic autosave on every world.
pub(super) fn save_on_registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("save-on"), |_| {
        literal("save-on").executes(save_on)
    })
    .at_level(PermissionLevel::Owners)
}

fn save_off(c: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let server = Arc::clone(c.source().server());
    if !server.set_auto_save(false) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SAVE_ALREADY_OFF.msg().component(),
        ));
    }
    c.source().send_success(
        &translations::COMMANDS_SAVE_DISABLED.msg().component(),
        true,
    );
    Ok(1)
}

fn save_on(c: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let server = Arc::clone(c.source().server());
    if !server.set_auto_save(true) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SAVE_ALREADY_ON.msg().component(),
        ));
    }
    c.source()
        .send_success(&translations::COMMANDS_SAVE_ENABLED.msg().component(), true);
    Ok(1)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("save-all")
        .executes_suspended(start_save_all)
        .then(literal("flush").executes_suspended(start_save_all))
}

/// Starts vanilla's `saveEverything(true, flush, true)` and reports `commands.save.saving`.
///
/// The save runs off the tick thread because it awaits file writes, so the
/// command result is produced by [`SaveAllTask`] once the writes complete.
#[expect(
    clippy::unnecessary_wraps,
    reason = "Command executors use a shared fallible callback signature."
)]
fn start_save_all(
    context: &SteelCommandContext<CommandSource>,
) -> Result<SaveAllTask, CommandSyntaxError> {
    let source = context.source().clone();
    let server = Arc::clone(source.server());
    source.send_success(&translations::COMMANDS_SAVE_SAVING.msg().component(), false);

    let (sender, receiver) = oneshot::channel();
    // `World::cleanup` holds `parking_lot` guards across awaits, so its future is
    // not `Send` and cannot be spawned on the async runtime. Run the save on the
    // blocking pool with its own current-thread runtime instead: the command
    // still resumes across ticks, and the server tick thread is never blocked
    // (vanilla blocks it for the whole save).
    drop(spawn_blocking(move || {
        let saved = match RuntimeBuilder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime.block_on(run_save_all(&server)),
            Err(error) => {
                log::error!("save-all could not start its save runtime: {error}");
                false
            }
        };
        let _ = sender.send(saved);
    }));

    Ok(SaveAllTask { source, receiver })
}

/// Saves players, command-owned data, and every loaded world.
///
/// Returns vanilla `MinecraftServer.saveAllChunks`'s structural result: `true`
/// when at least one level was saved, `false` when the server has no levels.
/// `/save-all` maps to `saveEverything(true, flush, true)`: the forced flag
/// makes worlds whose `/save-off` `noSave` flag is set save anyway.
async fn run_save_all(server: &Arc<Server>) -> bool {
    server.save_everything(true).await
}

/// In-flight `save-all`, resumed once the background save finishes.
struct SaveAllTask {
    source: CommandSource,
    receiver: oneshot::Receiver<bool>,
}

impl CommandResultSuspension for SaveAllTask {
    fn order(&self) -> CommandSuspensionOrder {
        // Vanilla blocks the server thread for the whole save, so later commands
        // must not observe a half-saved world.
        CommandSuspensionOrder::Global
    }

    fn poll(&mut self) -> CommandResultSuspensionPoll {
        match self.receiver.try_recv() {
            Ok(saved) => CommandResultSuspensionPoll::Ready(finish(&self.source, saved)),
            Err(oneshot::error::TryRecvError::Empty) => CommandResultSuspensionPoll::Pending,
            Err(oneshot::error::TryRecvError::Closed) => {
                CommandResultSuspensionPoll::Ready(Err(save_failed()))
            }
        }
    }
}

fn finish(source: &CommandSource, saved: bool) -> Result<i32, CommandSyntaxError> {
    if !saved {
        return Err(save_failed());
    }
    source.send_success(&translations::COMMANDS_SAVE_SUCCESS.msg().component(), true);
    Ok(1)
}

fn save_failed() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(translations::COMMANDS_SAVE_FAILED.msg().component())
}
