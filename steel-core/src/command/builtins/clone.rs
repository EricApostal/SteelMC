//! Vanilla block-region clone command.

use std::{collections::VecDeque, sync::Arc};

use simdnbt::owned::NbtCompound;
use steel_registry::{
    blocks::block_state_ext::BlockStateExt as _, vanilla_blocks,
    vanilla_game_rules::MAX_BLOCK_MODIFICATIONS,
};
use steel_utils::{
    BlockPos, BlockStateId, BoundingBox, Identifier, translations, types::UpdateFlags,
};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        BlockPredicate, CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime,
        argument, literal,
    },
    registration::CommandRegistration,
};
use super::execute::condition::ensure_region_chunks_loaded;
use crate::world::World;

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Vanilla `Block.UPDATE_KNOWN_SHAPE | UPDATE_SUPPRESS_DROPS |
/// UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS | UPDATE_SKIP_ON_PLACE`.
const STRICT_FLAGS: UpdateFlags = UpdateFlags::UPDATE_KNOWN_SHAPE
    .union(UpdateFlags::UPDATE_SUPPRESS_DROPS)
    .union(UpdateFlags::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS)
    .union(UpdateFlags::UPDATE_SKIP_ON_PLACE);

/// Vanilla `Block.UPDATE_NEIGHBORS | Block.UPDATE_CLIENTS`.
const STANDARD_FLAGS: UpdateFlags =
    UpdateFlags::UPDATE_NEIGHBORS.union(UpdateFlags::UPDATE_CLIENTS);

/// Which world one of `begin`/`end`/`destination` resolves in.
#[derive(Clone, Copy)]
enum DimensionSource {
    /// The command source's current level.
    Source,
    /// A `DimensionArgument`-style world argument of this name.
    Argument(&'static str),
}

/// Vanilla `CloneCommands.Mode`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloneMode {
    Force,
    Move,
    Normal,
}

impl CloneMode {
    /// Vanilla `Mode.canOverlap`.
    const fn can_overlap(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

/// Vanilla's block-selection predicate for one clone invocation.
#[derive(Clone, Copy)]
enum CloneFilter {
    All,
    Air,
    Predicate(&'static str),
}

impl CloneFilter {
    /// Vanilla `CloneCommands.FILTER_AIR` / the parsed `filtered` predicate.
    fn matches(
        self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        predicate: Option<&BlockPredicate>,
    ) -> bool {
        match self {
            Self::All => true,
            Self::Air => !state.is_air(),
            Self::Predicate(_) => predicate.is_some_and(|predicate| predicate.matches(world, pos)),
        }
    }
}

/// One destination block captured from the source region.
#[derive(Clone, Copy)]
struct CloneBlockInfo {
    pos: BlockPos,
    state: BlockStateId,
    previous_state_at_destination: BlockStateId,
}

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("clone"), |_| command())
}

fn command() -> Builder {
    literal("clone")
        .then(begin_end_destination_and_mode_suffix(
            DimensionSource::Source,
        ))
        .then(literal("from").then(
            argument("sourceDimension", SteelArgumentType::world()).then(
                begin_end_destination_and_mode_suffix(DimensionSource::Argument("sourceDimension")),
            ),
        ))
}

fn begin_end_destination_and_mode_suffix(from_dimension: DimensionSource) -> Builder {
    argument("begin", SteelArgumentType::block_pos()).then(
        argument("end", SteelArgumentType::block_pos())
            .then(destination_and_strict_suffix(
                from_dimension,
                DimensionSource::Source,
            ))
            .then(literal("to").then(
                argument("targetDimension", SteelArgumentType::world()).then(
                    destination_and_strict_suffix(
                        from_dimension,
                        DimensionSource::Argument("targetDimension"),
                    ),
                ),
            )),
    )
}

fn destination_and_strict_suffix(
    from_dimension: DimensionSource,
    to_dimension: DimensionSource,
) -> Builder {
    mode_suffix(
        from_dimension,
        to_dimension,
        false,
        argument("destination", SteelArgumentType::block_pos()),
    )
    .then(mode_suffix(
        from_dimension,
        to_dimension,
        true,
        literal("strict"),
    ))
}

fn mode_suffix(
    from_dimension: DimensionSource,
    to_dimension: DimensionSource,
    strict: bool,
    builder: Builder,
) -> Builder {
    builder
        .executes(move |context| {
            execute_clone(
                context,
                from_dimension,
                to_dimension,
                CloneFilter::All,
                CloneMode::Normal,
                strict,
            )
        })
        .then(wrap_with_clone_mode(
            from_dimension,
            to_dimension,
            strict,
            CloneFilter::All,
            literal("replace"),
        ))
        .then(wrap_with_clone_mode(
            from_dimension,
            to_dimension,
            strict,
            CloneFilter::Air,
            literal("masked"),
        ))
        .then(literal("filtered").then(wrap_with_clone_mode(
            from_dimension,
            to_dimension,
            strict,
            CloneFilter::Predicate("filter"),
            argument("filter", SteelArgumentType::block_predicate()),
        )))
}

fn wrap_with_clone_mode(
    from_dimension: DimensionSource,
    to_dimension: DimensionSource,
    strict: bool,
    filter: CloneFilter,
    builder: Builder,
) -> Builder {
    builder
        .executes(move |context| {
            execute_clone(
                context,
                from_dimension,
                to_dimension,
                filter,
                CloneMode::Normal,
                strict,
            )
        })
        .then(literal("force").executes(move |context| {
            execute_clone(
                context,
                from_dimension,
                to_dimension,
                filter,
                CloneMode::Force,
                strict,
            )
        }))
        .then(literal("move").executes(move |context| {
            execute_clone(
                context,
                from_dimension,
                to_dimension,
                filter,
                CloneMode::Move,
                strict,
            )
        }))
        .then(literal("normal").executes(move |context| {
            execute_clone(
                context,
                from_dimension,
                to_dimension,
                filter,
                CloneMode::Normal,
                strict,
            )
        }))
}

fn execute_clone(
    context: &SteelCommandContext<CommandSource>,
    from_dimension: DimensionSource,
    to_dimension: DimensionSource,
    filter: CloneFilter,
    mode: CloneMode,
    strict: bool,
) -> Result<i32, CommandSyntaxError> {
    let from_world = resolve_dimension(context, from_dimension)?;
    let to_world = resolve_dimension(context, to_dimension)?;
    let start_pos = loaded_block_position_in(&from_world, context, "begin")?;
    let end_pos = loaded_block_position_in(&from_world, context, "end")?;
    let destination_pos = loaded_block_position_in(&to_world, context, "destination")?;
    let predicate = match filter {
        CloneFilter::Predicate(name) => Some(context.block_predicate(name)?),
        CloneFilter::All | CloneFilter::Air => None,
    };

    clone_region(
        context.source(),
        &from_world,
        start_pos,
        end_pos,
        &to_world,
        destination_pos,
        filter,
        predicate,
        mode,
        strict,
    )
}

fn resolve_dimension(
    context: &SteelCommandContext<CommandSource>,
    dimension: DimensionSource,
) -> Result<Arc<World>, CommandSyntaxError> {
    match dimension {
        DimensionSource::Source => Ok(Arc::clone(context.source().world())),
        DimensionSource::Argument(name) => context.world_argument(name)?.resolve(context.source()),
    }
}

/// Vanilla `BlockPosArgument.getLoadedBlockPos(context, level, name)`.
fn loaded_block_position_in(
    world: &World,
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<BlockPos, CommandSyntaxError> {
    let position = context.coordinates(name)?.block_pos(context.source());
    if !world.is_full_chunk_loaded_at(position) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENT_POS_UNLOADED,
        )));
    }
    if !world.is_in_valid_bounds(position) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENT_POS_OUTOFWORLD,
        )));
    }
    Ok(position)
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors Vanilla CloneCommands.clone's parameter list"
)]
#[expect(
    clippy::too_many_lines,
    reason = "mirrors Vanilla's single-pass clone algorithm"
)]
fn clone_region(
    source: &CommandSource,
    from_world: &Arc<World>,
    start_pos: BlockPos,
    end_pos: BlockPos,
    to_world: &Arc<World>,
    destination_pos: BlockPos,
    filter: CloneFilter,
    predicate: Option<&BlockPredicate>,
    mode: CloneMode,
    strict: bool,
) -> Result<i32, CommandSyntaxError> {
    let from = BoundingBox::from_corners(start_pos, end_pos);
    let destination = BoundingBox::from_corners(
        destination_pos,
        destination_pos.offset(from.width() - 1, from.height() - 1, from.depth() - 1),
    );
    if !mode.can_overlap() && Arc::ptr_eq(from_world, to_world) && destination.intersects(from) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_CLONE_OVERLAP,
        )));
    }

    let area = i64::from(from.width()) * i64::from(from.height()) * i64::from(from.depth());
    let limit = source.world().get_game_rule(&MAX_BLOCK_MODIFICATIONS);
    if area > i64::from(limit) {
        return Err(area_too_large(limit, area));
    }

    ensure_region_chunks_loaded(from_world, &from)?;
    ensure_region_chunks_loaded(to_world, &destination)?;

    let offset = BlockPos::new(
        destination.min_x() - from.min_x(),
        destination.min_y() - from.min_y(),
        destination.min_z() - from.min_z(),
    );
    let mut solid_list = Vec::new();
    let mut block_entity_list = Vec::new();
    let mut other_blocks_list = Vec::new();
    let mut block_entity_data: Vec<(BlockPos, NbtCompound)> = Vec::new();
    let mut clear_blocks_list = VecDeque::new();

    for z in from.min_z()..=from.max_z() {
        for y in from.min_y()..=from.max_y() {
            for x in from.min_x()..=from.max_x() {
                let source_pos = BlockPos::new(x, y, z);
                let block_state = from_world.get_block_state(source_pos);
                if !filter.matches(from_world, source_pos, block_state, predicate) {
                    continue;
                }

                let destination_pos = source_pos.offset(offset.x(), offset.y(), offset.z());
                let info = CloneBlockInfo {
                    pos: destination_pos,
                    state: block_state,
                    previous_state_at_destination: to_world.get_block_state(destination_pos),
                };
                if let Some(block_entity) = from_world.get_block_entity(source_pos) {
                    block_entity_data.push((destination_pos, block_entity.save_custom_only()));
                    block_entity_list.push(info);
                    clear_blocks_list.push_back(source_pos);
                } else if !block_state.is_solid_render() && !block_state.is_solid() {
                    other_blocks_list.push(info);
                    clear_blocks_list.push_front(source_pos);
                } else {
                    solid_list.push(info);
                    clear_blocks_list.push_back(source_pos);
                }
            }
        }
    }

    let default_update_flags = if strict {
        UpdateFlags::UPDATE_CLIENTS.union(STRICT_FLAGS)
    } else {
        UpdateFlags::UPDATE_CLIENTS
    };
    let barrier_flags = default_update_flags.union(STRICT_FLAGS);
    if mode == CloneMode::Move {
        for pos in &clear_blocks_list {
            from_world.set_block(*pos, vanilla_blocks::BARRIER.default_state(), barrier_flags);
        }

        let standard_update_flags = if strict {
            default_update_flags
        } else {
            STANDARD_FLAGS
        };
        for pos in &clear_blocks_list {
            from_world.set_block(
                *pos,
                vanilla_blocks::AIR.default_state(),
                standard_update_flags,
            );
        }
    }

    let mut block_info_list = solid_list;
    block_info_list.extend(block_entity_list);
    block_info_list.extend(other_blocks_list);

    for info in block_info_list.iter().rev() {
        to_world.set_block(
            info.pos,
            vanilla_blocks::BARRIER.default_state(),
            barrier_flags,
        );
    }

    let mut count = 0;
    for info in &block_info_list {
        if to_world.set_block(info.pos, info.state, default_update_flags) {
            count += 1;
        }
    }

    for (pos, tag) in &block_entity_data {
        if let Some(block_entity) = to_world.get_block_entity(*pos) {
            block_entity
                .load_with_owned_components(tag)
                .map_err(|error| {
                    CommandSyntaxError::dynamic(format!(
                        "Failed to copy block entity data to {pos:?}: {error}"
                    ))
                })?;
            block_entity.set_changed();
        }
    }
    for info in &block_info_list {
        to_world.set_block(info.pos, info.state, default_update_flags);
    }

    if !strict {
        for info in block_info_list.iter().rev() {
            to_world.update_neighbors_on_block_set(info.pos, info.previous_state_at_destination);
        }
    }

    to_world.copy_scheduled_ticks_from_area(from_world, from, offset.0);

    if count == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_CLONE_FAILED,
        )));
    }

    let message = translations::COMMANDS_CLONE_SUCCESS
        .message([TextComponent::from(count.to_string())])
        .component();
    source.send_success(&message, true);
    Ok(count)
}

fn area_too_large(limit: i32, area: i64) -> CommandSyntaxError {
    let message = translations::COMMANDS_CLONE_TOOBIG
        .message([
            TextComponent::from(limit.to_string()),
            TextComponent::from(area.to_string()),
        ])
        .component();
    CommandSyntaxError::dynamic(message)
}
