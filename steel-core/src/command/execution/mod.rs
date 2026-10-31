//! Tick-owned Minecraft command execution.

use std::sync::Arc;

mod argument;
mod biome;
mod block;
mod coordinates;
mod item;
mod item_predicate;
mod nbt;
mod permission;
mod poi;
mod profile;
mod queue;
mod runtime;
mod score;
mod selector;
mod source;
mod structure;
mod suggestions;
mod text;
mod world;

pub(crate) use argument::SteelArgumentType;
pub(crate) use argument::{
    ColumnPos, ItemModifierRef, SlotSourceArg, SlotSourceName, SteelArgumentParser,
    SteelArgumentSuggestionContext,
};
pub(crate) use biome::BiomeOrTag;
pub(crate) use block::{BlockInput, BlockPredicate, placement_flags};
pub(crate) use coordinates::Coordinates;
pub(crate) use item_predicate::ItemPredicate;
pub(crate) use permission::PermissionGroupName;
pub(crate) use poi::PoiTypeOrTag;
pub(crate) use profile::GameProfileArgument;
pub use queue::CommandSuspensionOrder;
pub(crate) use queue::{
    ChainModifiers, CommandExecutionContext, CommandResultSuspension, CommandResultSuspensionPoll,
    ExecutionControl, ExecutionStop,
};
#[cfg_attr(
    not(test),
    expect(
        unused_imports,
        reason = "custom scheduler extension hooks are retained for keyed command runtimes"
    )
)]
pub(crate) use queue::{CommandSuspension, CommandSuspensionPoll, EntryAction, Frame};
pub(crate) use runtime::{CustomCommandExecutor, CustomModifierExecutor};
pub(crate) use runtime::{
    SteelCommandContext, SteelCommandRuntime, SteelContextChain, SteelExecutor, SteelModifier,
    SteelSuggestionContext, argument, literal,
};
pub(crate) use score::{IntRange, ScoreHolderArgument, ScoreHolderWildcard};
#[cfg(test)]
pub(crate) use selector::parse_entity_selector_text;
pub(crate) use source::{
    CommandArgumentSource, CommandOutputCapture, CommandPermissionSource, CommandResultCallback,
    CommandSource, ExecutionCommandSource,
};
pub(crate) use structure::StructureOrTagKey;
pub(crate) use suggestions::{FixedSuggestionProvider, suggest_list};
pub(crate) use text::CommandTextResolver;
pub(crate) use world::WorldArgument;

/// Runs one command string against `source` immediately, mirroring vanilla
/// `Commands.performPrefixedCommand`.
///
/// Command blocks execute inside the block tick and need the outcome synchronously (the
/// success count gates the chain), so this bypasses [`crate::server::Server::submit_command`]'s
/// next-tick queue and runs the parsed chain on the calling thread, exactly like datapack
/// functions do. Parse failures are reported to the source like vanilla.
///
/// Vanilla attaches the result callback to the *source* (`CommandSourceStack.withCallback`)
/// and passes `CommandResultCallback.EMPTY` as the top-level consumer, because only `/return`
/// reports through the consumer while ordinary commands report through the source. The
/// scheduler mirrors that split (`complete_command_result` calls `source.callback()`), so the
/// callback must be installed with [`ExecutionCommandSource::with_callback`].
pub(crate) fn perform_prefixed_command(
    source: &Arc<CommandSource>,
    command: &str,
    callback: CommandResultCallback,
) {
    let source = Arc::new(source.with_callback(callback));
    let command = command.strip_prefix('/').unwrap_or(command);
    let chain = {
        let dispatcher = source.server().command_dispatcher();
        let parse = dispatcher.parse(command, source.as_ref().clone());
        dispatcher.context_chain(parse)
    };
    let chain = match chain {
        Ok(chain) => chain,
        Err(error) => {
            source.handle_error(&error, false);
            return;
        }
    };

    let mut execution = CommandExecutionContext::for_source(&source);
    execution.queue_initial_command(
        chain,
        source.as_ref().clone(),
        CommandResultCallback::empty(),
    );
    let stop = execution.run();
    if stop != ExecutionStop::Completed {
        // Vanilla command blocks never suspend; Steel's scheduler can, and its drop
        // cancels the remaining work like a failing vanilla command.
        tracing::warn!(
            ?stop,
            command,
            "command block execution stopped before completing in one tick"
        );
    }
}

#[cfg(test)]
mod argument_tests;
#[cfg(test)]
mod tests;
