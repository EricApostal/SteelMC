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
//! `save-off` and `save-on` need vanilla's per-level `noSave` flag plus the
//! periodic world autosave it gates; neither foundation exists yet, so this
//! module only registers `save-all` for now.

use std::sync::Arc;

use steel_utils::{Identifier, translations};
use tokio::sync::oneshot;

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

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("save-all")
        .executes_suspended(start_save_all)
        .then(literal("flush").executes_suspended(start_save_all))
}

/// Starts vanilla's `saveEverything(true, flush, true)` and reports `commands.save.saving`.
///
/// The save runs off the tick thread because it awaits file writes, so the
/// command result is produced by [`SaveAllTask`] once the writes complete.
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
    drop(tokio::task::spawn_blocking(move || {
        let saved = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
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
async fn run_save_all(server: &Arc<Server>) -> bool {
    for player in server.get_players() {
        if let Err(error) = server.player_data_storage.save(&player).await {
            log::error!(
                "Failed to save player {} during save-all: {error}",
                player.gameprofile.id()
            );
        }
    }

    let command_data = server.save_command_data().await;
    match command_data.scoreboards {
        Ok(saved) => tracing::debug!(saved, "save-all stored domain scoreboards"),
        Err(error) => log::error!("save-all failed to store domain scoreboards: {error}"),
    }
    match command_data.storage {
        Ok(saved) => tracing::debug!(saved, "save-all stored domain command storage"),
        Err(error) => log::error!("save-all failed to store domain command storage: {error}"),
    }

    let mut chunks_saved = 0usize;
    let mut levels_saved = 0usize;
    for world in server.worlds.values() {
        world.cleanup(&mut chunks_saved).await;
        levels_saved += 1;
    }
    tracing::debug!(chunks_saved, levels_saved, "save-all finished");
    levels_saved > 0
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
