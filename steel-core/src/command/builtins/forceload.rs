//! Vanilla chunk force-load command.

use steel_utils::{ChunkPos, Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        ColumnPos, CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        argument, literal,
    },
    registration::CommandRegistration,
};
use crate::world::World;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Vanilla `ForceLoadCommand.MAX_CHUNK_LIMIT`.
const MAX_CHUNK_LIMIT: i64 = 256;

/// Vanilla's `ColumnPosArgument` bounds check (`BlockPosArgument.ERROR_OUT_OF_WORLD`).
const MAX_HORIZONTAL_COORDINATE: i32 = 30_000_000;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("forceload"), |_| command())
}

fn command() -> Builder {
    literal("forceload")
        .then(
            literal("add").then(
                argument("from", SteelArgumentType::column_pos())
                    .executes(|context| change_force_load(context, "from", "from", true))
                    .then(
                        argument("to", SteelArgumentType::column_pos())
                            .executes(|context| change_force_load(context, "from", "to", true)),
                    ),
            ),
        )
        .then(
            literal("remove")
                .then(
                    argument("from", SteelArgumentType::column_pos())
                        .executes(|context| change_force_load(context, "from", "from", false))
                        .then(
                            argument("to", SteelArgumentType::column_pos()).executes(|context| {
                                change_force_load(context, "from", "to", false)
                            }),
                        ),
                )
                .then(literal("all").executes(remove_all)),
        )
        .then(
            literal("query").executes(list_force_load).then(
                argument("pos", SteelArgumentType::column_pos())
                    .executes(|context| query_force_load(context, "pos")),
            ),
        )
}

fn dimension_name(world: &World) -> String {
    world.key.to_string()
}

fn chunk_pos_name(pos: ChunkPos) -> String {
    format!("[{}, {}]", pos.0.x, pos.0.y)
}

fn query_force_load(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<i32, CommandSyntaxError> {
    let world = context.source().world();
    let chunk_pos = to_chunk_pos(context.column_pos(name)?);
    if !world.chunk_map.force_loaded_chunks().contains(&chunk_pos) {
        let message = translations::COMMANDS_FORCELOAD_QUERY_FAILURE
            .message([
                TextComponent::from(chunk_pos_name(chunk_pos)),
                TextComponent::from(dimension_name(world)),
            ])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }

    let message = translations::COMMANDS_FORCELOAD_QUERY_SUCCESS
        .message([
            TextComponent::from(chunk_pos_name(chunk_pos)),
            TextComponent::from(dimension_name(world)),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(1)
}

fn list_force_load(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let world = context.source().world();
    let forced_chunks = world.chunk_map.force_loaded_chunks();
    let chunk_count = forced_chunks.len();
    let dimension = dimension_name(world);
    if chunk_count > 0 {
        let chunk_list = forced_chunks
            .iter()
            .map(|pos| chunk_pos_name(*pos))
            .collect::<Vec<_>>()
            .join(", ");
        let message = if chunk_count == 1 {
            translations::COMMANDS_FORCELOAD_LIST_SINGLE
                .message([
                    TextComponent::from(dimension),
                    TextComponent::from(chunk_list),
                ])
                .component()
        } else {
            translations::COMMANDS_FORCELOAD_LIST_MULTIPLE
                .message([
                    TextComponent::from(chunk_count.to_string()),
                    TextComponent::from(dimension),
                    TextComponent::from(chunk_list),
                ])
                .component()
        };
        context.source().send_success(&message, false);
    } else {
        let message = translations::COMMANDS_FORCELOAD_ADDED_NONE
            .message([TextComponent::from(dimension)])
            .component();
        context.source().send_failure(message);
    }

    i32::try_from(chunk_count).map_err(|_| {
        CommandSyntaxError::dynamic("Force-loaded chunk count exceeds the command result range")
    })
}

fn remove_all(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let world = context.source().world();
    for chunk in world.chunk_map.force_loaded_chunks() {
        world.chunk_map.set_chunk_forced(chunk, false);
    }
    let message = translations::COMMANDS_FORCELOAD_REMOVED_ALL
        .message([TextComponent::from(dimension_name(world))])
        .component();
    context.source().send_success(&message, true);
    Ok(0)
}

fn change_force_load(
    context: &SteelCommandContext<CommandSource>,
    from_name: &str,
    to_name: &str,
    add: bool,
) -> Result<i32, CommandSyntaxError> {
    let from = context.column_pos(from_name)?;
    let to = context.column_pos(to_name)?;
    let min_x = from.x.min(to.x);
    let min_z = from.z.min(to.z);
    let max_x = from.x.max(to.x);
    let max_z = from.z.max(to.z);
    if min_x < -MAX_HORIZONTAL_COORDINATE
        || min_z < -MAX_HORIZONTAL_COORDINATE
        || max_x >= MAX_HORIZONTAL_COORDINATE
        || max_z >= MAX_HORIZONTAL_COORDINATE
    {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENT_POS_OUTOFWORLD,
        )));
    }

    let min_chunk_x = min_x >> 4;
    let min_chunk_z = min_z >> 4;
    let max_chunk_x = max_x >> 4;
    let max_chunk_z = max_z >> 4;
    let chunk_count =
        i64::from(max_chunk_x - min_chunk_x + 1) * i64::from(max_chunk_z - min_chunk_z + 1);
    if chunk_count > MAX_CHUNK_LIMIT {
        let message = translations::COMMANDS_FORCELOAD_TOOBIG
            .message([
                TextComponent::from(MAX_CHUNK_LIMIT.to_string()),
                TextComponent::from(chunk_count.to_string()),
            ])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }

    let world = context.source().world();
    let dimension = dimension_name(world);
    let mut changed = Vec::new();
    for x in min_chunk_x..=max_chunk_x {
        for z in min_chunk_z..=max_chunk_z {
            let chunk = ChunkPos::new(x, z);
            if world.chunk_map.set_chunk_forced(chunk, add) {
                changed.push(chunk);
            }
        }
    }
    let min_chunk = ChunkPos::new(min_chunk_x, min_chunk_z);
    let max_chunk = ChunkPos::new(max_chunk_x, max_chunk_z);

    send_force_load_feedback(
        context.source(),
        &changed,
        add,
        &dimension,
        min_chunk,
        max_chunk,
    )
}

fn send_force_load_feedback(
    source: &CommandSource,
    changed: &[ChunkPos],
    add: bool,
    dimension: &str,
    min_chunk: ChunkPos,
    max_chunk: ChunkPos,
) -> Result<i32, CommandSyntaxError> {
    if changed.is_empty() {
        let message = if add {
            TextComponent::from(&translations::COMMANDS_FORCELOAD_ADDED_FAILURE)
        } else {
            TextComponent::from(&translations::COMMANDS_FORCELOAD_REMOVED_FAILURE)
        };
        return Err(CommandSyntaxError::dynamic(message));
    }

    if changed.len() == 1 {
        let translation = if add {
            &translations::COMMANDS_FORCELOAD_ADDED_SINGLE
        } else {
            &translations::COMMANDS_FORCELOAD_REMOVED_SINGLE
        };
        let message = translation
            .message([
                TextComponent::from(chunk_pos_name(changed[0])),
                TextComponent::plain(dimension.to_owned()),
            ])
            .component();
        source.send_success(&message, true);
    } else {
        let translation = if add {
            &translations::COMMANDS_FORCELOAD_ADDED_MULTIPLE
        } else {
            &translations::COMMANDS_FORCELOAD_REMOVED_MULTIPLE
        };
        let message = translation
            .message([
                TextComponent::from(changed.len().to_string()),
                TextComponent::plain(dimension.to_owned()),
                TextComponent::from(chunk_pos_name(min_chunk)),
                TextComponent::from(chunk_pos_name(max_chunk)),
            ])
            .component();
        source.send_success(&message, true);
    }

    i32::try_from(changed.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Changed chunk count exceeds the command result range")
    })
}

fn to_chunk_pos(pos: ColumnPos) -> ChunkPos {
    ChunkPos::new(pos.x >> 4, pos.z >> 4)
}
