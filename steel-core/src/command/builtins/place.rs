//! Vanilla structure, template, jigsaw, and feature placement command.
//!
//! Vanilla's `PlaceCommand` writes straight into the executing `ServerLevel`
//! (`StructureStart.placeInChunk`, `StructureTemplate.placeInWorld`,
//! `JigsawPlacement.generateJigsaw`). Steel's placement layer is generic over
//! [`TemplateLevelAccess`](crate::worldgen::template::TemplateLevelAccess), so
//! the structure, template, and jigsaw branches drive the same worldgen code
//! through [`LiveLevel`], which writes into the live world's chunks.
//!
//! `place feature` is the one branch Steel cannot serve: the placed-feature
//! pipeline is written against
//! [`WorldGenRegion`](crate::worldgen::region::WorldGenRegion), and a region
//! built over live chunks rejects every write (`WorldGenAccessMode::capture`
//! marks `ChunkStatus::Full` chunks read-only). The branch therefore fails
//! explicitly instead of reporting a placement that never happened.

use std::sync::Arc;

use glam::IVec3;
use steel_registry::structure::LiquidSettingsData;
use steel_registry::structure_processor::StructureProcessorKind;
use steel_registry::{REGISTRY, Registry, RegistryExt as _};
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::translations::{
    ARGUMENT_POS_UNLOADED, COMMANDS_PLACE_JIGSAW_FAILED, COMMANDS_PLACE_JIGSAW_INVALID,
    COMMANDS_PLACE_JIGSAW_SUCCESS, COMMANDS_PLACE_STRUCTURE_FAILED,
    COMMANDS_PLACE_STRUCTURE_INVALID, COMMANDS_PLACE_STRUCTURE_SUCCESS,
    COMMANDS_PLACE_TEMPLATE_FAILED, COMMANDS_PLACE_TEMPLATE_INVALID,
    COMMANDS_PLACE_TEMPLATE_SUCCESS,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BoundingBox, ChunkPos, Identifier, Rotation};
use steel_worldgen::biomes::obfuscate_biome_seed;
use steel_worldgen::structure::{StructureBlockIgnore, StructureMirror};
use text_components::{TextComponent, translation::Translation};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use super::execute::loaded_block_position;
use crate::world::World;
use crate::world::spawn::natural::runtime_random;
use crate::worldgen::generator::ChunkGenerator as _;
use crate::worldgen::structure::{place_jigsaw_pieces, place_structure_start};
use crate::worldgen::template::{
    LiveLevel, StructurePlaceSettings, StructureProcessorRandom, StructureTemplate,
};

/// Vanilla `BoundingBox.infinite()`: the box `/place template` places through,
/// because the command writes the whole template instead of a chunk slice.
const INFINITE_PLACEMENT_BOX: BoundingBox =
    BoundingBox::new(IVec3::splat(i32::MIN), IVec3::splat(i32::MAX));

/// Vanilla `/place template`'s `strict` flags: `Block.UPDATE_KNOWN_SHAPE |
/// UPDATE_SUPPRESS_DROPS | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS |
/// UPDATE_SKIP_ON_PLACE` (vanilla's literal `816`).
const STRICT_TEMPLATE_FLAGS: UpdateFlags = UpdateFlags::UPDATE_KNOWN_SHAPE
    .union(UpdateFlags::UPDATE_SUPPRESS_DROPS)
    .union(UpdateFlags::UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS)
    .union(UpdateFlags::UPDATE_SKIP_ON_PLACE);

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("place"), |_| command())
}

#[expect(
    clippy::too_many_lines,
    reason = "one flat builder mirrors vanilla PlaceCommand.register's argument tree"
)]
fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("place")
        .then(
            literal("feature").then(
                argument("feature", SteelArgumentType::feature())
                    .executes(|context| place_feature(context, source_block_pos(context)))
                    .then(
                        argument("pos", SteelArgumentType::block_pos()).executes(|context| {
                            let pos = loaded_block_position(context, "pos")?;
                            place_feature(context, pos)
                        }),
                    ),
            ),
        )
        .then(
            literal("jigsaw").then(
                argument("pool", SteelArgumentType::template_pool()).then(
                    argument("target", SteelArgumentType::identifier_argument()).then(
                        argument("max_depth", ArgumentType::integer(1, 20))
                            .executes(|context| place_jigsaw(context, source_block_pos(context)))
                            .then(
                                argument("position", SteelArgumentType::block_pos()).executes(
                                    |context| {
                                        let pos = loaded_block_position(context, "position")?;
                                        place_jigsaw(context, pos)
                                    },
                                ),
                            ),
                    ),
                ),
            ),
        )
        .then(
            literal("structure").then(
                argument("structure", SteelArgumentType::structure())
                    .executes(|context| place_structure(context, source_block_pos(context)))
                    .then(
                        argument("pos", SteelArgumentType::block_pos()).executes(|context| {
                            let pos = loaded_block_position(context, "pos")?;
                            place_structure(context, pos)
                        }),
                    ),
            ),
        )
        .then(
            literal("template").then(
                argument("template", SteelArgumentType::identifier_argument())
                    .executes(|context| {
                        place_template(
                            context,
                            source_block_pos(context),
                            TemplateOptions::default(),
                        )
                    })
                    .then(
                        argument("pos", SteelArgumentType::block_pos())
                            .executes(|context| {
                                let pos = loaded_block_position(context, "pos")?;
                                place_template(context, pos, TemplateOptions::default())
                            })
                            .then(
                                argument("rotation", SteelArgumentType::template_rotation())
                                    .executes(|context| {
                                        let pos = loaded_block_position(context, "pos")?;
                                        let rotation = context.template_rotation("rotation")?;
                                        place_template(
                                            context,
                                            pos,
                                            TemplateOptions::default().with_rotation(rotation),
                                        )
                                    })
                                    .then(
                                        argument("mirror", SteelArgumentType::template_mirror())
                                            .executes(|context| {
                                                let pos = loaded_block_position(context, "pos")?;
                                                let rotation =
                                                    context.template_rotation("rotation")?;
                                                let mirror = context.template_mirror("mirror")?;
                                                place_template(
                                                    context,
                                                    pos,
                                                    TemplateOptions::default()
                                                        .with_rotation(rotation)
                                                        .with_mirror(mirror),
                                                )
                                            })
                                            .then(
                                                argument(
                                                    "integrity",
                                                    ArgumentType::float(0.0, 1.0),
                                                )
                                                .executes(|context| {
                                                    let options =
                                                        template_options(context, false, 0)?;
                                                    let pos =
                                                        loaded_block_position(context, "pos")?;
                                                    place_template(context, pos, options)
                                                })
                                                .then(
                                                    argument(
                                                        "seed",
                                                        ArgumentType::integer(i32::MIN, i32::MAX),
                                                    )
                                                    .executes(|context| {
                                                        let seed = context.integer("seed")?;
                                                        let options =
                                                            template_options(context, false, seed)?;
                                                        let pos =
                                                            loaded_block_position(context, "pos")?;
                                                        place_template(context, pos, options)
                                                    })
                                                    .then(
                                                        argument("strict", ArgumentType::bool())
                                                            .executes(|context| {
                                                                let strict =
                                                                    context.boolean("strict")?;
                                                                let seed =
                                                                    context.integer("seed")?;
                                                                let options = template_options(
                                                                    context, strict, seed,
                                                                )?;
                                                                let pos = loaded_block_position(
                                                                    context, "pos",
                                                                )?;
                                                                place_template(
                                                                    context, pos, options,
                                                                )
                                                            }),
                                                    ),
                                                ),
                                            ),
                                    ),
                            ),
                    ),
            ),
        )
}

/// Vanilla's default `pos`: the executing source's block position.
fn source_block_pos(context: &SteelCommandContext<CommandSource>) -> BlockPos {
    BlockPos::from(context.source().position())
}

/// The `/place template` arguments past `pos`, as vanilla passes them.
#[derive(Debug, Clone, Copy)]
struct TemplateOptions {
    rotation: Rotation,
    mirror: StructureMirror,
    integrity: f32,
    seed: i32,
    strict: bool,
}

impl Default for TemplateOptions {
    fn default() -> Self {
        Self {
            rotation: Rotation::None,
            mirror: StructureMirror::None,
            integrity: 1.0,
            seed: 0,
            strict: false,
        }
    }
}

impl TemplateOptions {
    const fn with_rotation(mut self, rotation: Rotation) -> Self {
        self.rotation = rotation;
        self
    }

    const fn with_mirror(mut self, mirror: StructureMirror) -> Self {
        self.mirror = mirror;
        self
    }

    /// Vanilla's template update flags: `UPDATE_CLIENTS`, plus the full known-
    /// shape set when `strict`.
    const fn update_flags(self) -> UpdateFlags {
        if self.strict {
            UpdateFlags::UPDATE_CLIENTS.union(STRICT_TEMPLATE_FLAGS)
        } else {
            UpdateFlags::UPDATE_CLIENTS
        }
    }
}

/// Reads the trailing `rotation`/`mirror`/`integrity` nodes of `/place template`.
fn template_options(
    context: &SteelCommandContext<CommandSource>,
    strict: bool,
    seed: i32,
) -> Result<TemplateOptions, CommandSyntaxError> {
    Ok(TemplateOptions {
        rotation: context.template_rotation("rotation")?,
        mirror: context.template_mirror("mirror")?,
        integrity: context.float("integrity")?,
        seed,
        strict,
    })
}

/// Reasons a live placement can be rejected.
///
/// The command layer maps each variant to vanilla's message for the branch it
/// belongs to, in [`command_error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaceError {
    /// The requested id is not registered (vanilla `ERROR_*_INVALID`).
    Invalid,
    /// The placement itself produced nothing (vanilla `ERROR_*_FAILED`).
    Failed,
    /// A chunk the placement touches is not loaded (vanilla
    /// `BlockPosArgument.ERROR_NOT_LOADED`).
    NotLoaded,
    /// The placed-feature pipeline cannot run over live chunks.
    #[expect(dead_code, reason = "vanilla PlaceError variant")]
    LiveFeatureUnsupported,
}

/// Why `place feature` cannot write into a live world, as shown to operators.
const LIVE_FEATURE_UNSUPPORTED: &str = "Failed to place feature: Steel cannot run the \
placed-feature pipeline against live chunks (a world-generation region over \
live chunks rejects every block write), so /place feature is not supported \
until the feature layer goes level-generic";

/// Translates a placement failure into vanilla's command error for the branch.
fn command_error(
    error: PlaceError,
    id: &Identifier,
    invalid: &'static Translation<1>,
    failed: &'static Translation<0>,
) -> CommandSyntaxError {
    match error {
        PlaceError::Invalid => CommandSyntaxError::dynamic(
            invalid
                .message([TextComponent::plain(id.to_string())])
                .component(),
        ),
        PlaceError::Failed => CommandSyntaxError::dynamic(TextComponent::from(failed)),
        PlaceError::NotLoaded => {
            CommandSyntaxError::dynamic(TextComponent::from(&ARGUMENT_POS_UNLOADED))
        }
        PlaceError::LiveFeatureUnsupported => {
            log::warn!("{LIVE_FEATURE_UNSUPPORTED}");
            CommandSyntaxError::dynamic(TextComponent::plain(LIVE_FEATURE_UNSUPPORTED))
        }
    }
}

/// Vanilla `PlaceCommand.checkLoaded`: every chunk in the inclusive rectangle
/// must already be a full chunk in the live world.
fn ensure_chunks_loaded(
    world: &Arc<World>,
    min_chunk_x: i32,
    min_chunk_z: i32,
    max_chunk_x: i32,
    max_chunk_z: i32,
) -> Result<(), PlaceError> {
    for chunk_x in min_chunk_x..=max_chunk_x {
        for chunk_z in min_chunk_z..=max_chunk_z {
            if !world.is_full_chunk_loaded_at(BlockPos::new(chunk_x * 16, 0, chunk_z * 16)) {
                return Err(PlaceError::NotLoaded);
            }
        }
    }
    Ok(())
}

/// `/place feature`: fails explicitly; the placed-feature pipeline is still
/// `WorldGenRegion`-only and cannot write into live chunks.
fn place_feature(
    context: &SteelCommandContext<CommandSource>,
    pos: BlockPos,
) -> Result<i32, CommandSyntaxError> {
    let key = context.identifier("feature")?;
    // `key` is parsed so the argument node still validates the id shape; the
    // branch cannot write anything either way.
    let _ = (key, pos);
    log::warn!("{LIVE_FEATURE_UNSUPPORTED}");
    Err(CommandSyntaxError::dynamic(TextComponent::plain(
        LIVE_FEATURE_UNSUPPORTED,
    )))
}

/// `/place structure`: vanilla `PlaceCommand.placeStructure`.
fn place_structure(
    context: &SteelCommandContext<CommandSource>,
    pos: BlockPos,
) -> Result<i32, CommandSyntaxError> {
    let key = context.identifier("structure")?;
    let world = context.source().world();
    place_structure_in_world(world, key, pos).map_err(|error| {
        command_error(
            error,
            key,
            &COMMANDS_PLACE_STRUCTURE_INVALID,
            &COMMANDS_PLACE_STRUCTURE_FAILED,
        )
    })?;

    context.source().send_success(
        &COMMANDS_PLACE_STRUCTURE_SUCCESS
            .message([
                TextComponent::plain(key.to_string()),
                TextComponent::plain(pos.x().to_string()),
                TextComponent::plain(pos.y().to_string()),
                TextComponent::plain(pos.z().to_string()),
            ])
            .component(),
        true,
    );
    Ok(1)
}

/// Generates the structure start at `pos` and places it into live chunks.
///
/// Separates vanilla `Structure.generate` + `StructureStart.placeInChunk` from
/// message handling so tests can drive the exact placement routine.
pub(crate) fn place_structure_in_world(
    world: &Arc<World>,
    structure: &Identifier,
    pos: BlockPos,
) -> Result<(), PlaceError> {
    // Vanilla `ResourceKeyArgument.getStructure`: the id must name a structure.
    let Some(structure_ref) = REGISTRY.structures.by_key(structure) else {
        return Err(PlaceError::Invalid);
    };

    // Vanilla `Structure.generate`: the structure's own generation point, without
    // the structure-set placement and frequency checks that bind normal starts
    // to their set chunks.
    let chunk_pos = ChunkPos::new(pos.x() >> 4, pos.z() >> 4);
    let Some(mut start) = world
        .chunk_map
        .world_gen_context
        .generator
        .generate_structure_start(chunk_pos, &structure_ref.key)
    else {
        return Err(PlaceError::Failed);
    };

    let Some(bounding_box) = start.bounding_box else {
        return Err(PlaceError::Failed);
    };
    ensure_chunks_loaded(
        world,
        bounding_box.min_x() >> 4,
        bounding_box.min_z() >> 4,
        bounding_box.max_x() >> 4,
        bounding_box.max_z() >> 4,
    )?;

    let mut level = LiveLevel::new(world);
    let mut random = WorldgenRandom::from_source(runtime_random());
    let registry: &Registry = &REGISTRY;
    // Unlike vanilla's `placeInChunk` (which cannot fail), Steel reports a
    // failed placement when no piece landed a block, so a silent no-op never
    // masquerades as success.
    if !place_structure_start(
        &mut level,
        registry,
        &mut start,
        &mut random,
        obfuscate_biome_seed(world.seed()),
    ) {
        return Err(PlaceError::Failed);
    }
    Ok(())
}

/// `/place jigsaw`: vanilla `PlaceCommand.placeJigsaw`.
fn place_jigsaw(
    context: &SteelCommandContext<CommandSource>,
    pos: BlockPos,
) -> Result<i32, CommandSyntaxError> {
    let pool = context.identifier("pool")?;
    let target = context.identifier("target")?;
    let max_depth = context.integer("max_depth")?;
    let world = context.source().world();
    place_jigsaw_in_world(world, pool, target, max_depth, pos).map_err(|error| {
        // The pool hosts the registry check, so the invalid message names it.
        command_error(
            error,
            pool,
            &COMMANDS_PLACE_JIGSAW_INVALID,
            &COMMANDS_PLACE_JIGSAW_FAILED,
        )
    })?;

    context.source().send_success(
        &COMMANDS_PLACE_JIGSAW_SUCCESS
            .message([
                TextComponent::plain(pos.x().to_string()),
                TextComponent::plain(pos.y().to_string()),
                TextComponent::plain(pos.z().to_string()),
            ])
            .component(),
        true,
    );
    Ok(1)
}

/// Assembles the pool's jigsaw pieces at `pos` and places them into live chunks.
fn place_jigsaw_in_world(
    world: &Arc<World>,
    pool: &Identifier,
    target: &Identifier,
    max_depth: i32,
    pos: BlockPos,
) -> Result<(), PlaceError> {
    // Vanilla `ResourceKeyArgument` + `JigsawPlacement.generateJigsaw`: the pool
    // must name a template pool.
    let Some(generator) = world
        .chunk_map
        .world_gen_context
        .generator
        .structure_generator()
    else {
        return Err(PlaceError::Failed);
    };
    if !generator.template_pools().contains_key(pool) {
        return Err(PlaceError::Invalid);
    }
    // Vanilla `checkLoaded` for the single target chunk.
    ensure_chunks_loaded(
        world,
        pos.x() >> 4,
        pos.z() >> 4,
        pos.x() >> 4,
        pos.z() >> 4,
    )?;

    if !place_jigsaw_pieces(world, pool, target, max_depth, pos) {
        return Err(PlaceError::Failed);
    }
    Ok(())
}

/// `/place template`: vanilla `PlaceCommand.placeTemplate`.
fn place_template(
    context: &SteelCommandContext<CommandSource>,
    pos: BlockPos,
    options: TemplateOptions,
) -> Result<i32, CommandSyntaxError> {
    let key = context.identifier("template")?;
    let world = context.source().world();
    place_template_in_world(world, key, pos, options).map_err(|error| {
        command_error(
            error,
            key,
            &COMMANDS_PLACE_TEMPLATE_INVALID,
            &COMMANDS_PLACE_TEMPLATE_FAILED,
        )
    })?;

    context.source().send_success(
        &COMMANDS_PLACE_TEMPLATE_SUCCESS
            .message([
                TextComponent::plain(key.to_string()),
                TextComponent::plain(pos.x().to_string()),
                TextComponent::plain(pos.y().to_string()),
                TextComponent::plain(pos.z().to_string()),
            ])
            .component(),
        true,
    );
    Ok(1)
}

/// Loads the template and places it into live chunks with vanilla's settings.
fn place_template_in_world(
    world: &Arc<World>,
    template_key: &Identifier,
    pos: BlockPos,
    options: TemplateOptions,
) -> Result<(), PlaceError> {
    // Vanilla `StructureTemplateManager.get`: the id must name a bundled template.
    let Ok(template) = StructureTemplate::load_vanilla(&REGISTRY, template_key) else {
        return Err(PlaceError::Invalid);
    };

    // Vanilla `checkLoaded` spans `pos` through `pos + template size`.
    let size = template.unrotated_size();
    let end = pos.offset(size.x, size.y, size.z);
    ensure_chunks_loaded(
        world,
        pos.x() >> 4,
        pos.z() >> 4,
        end.x() >> 4,
        end.z() >> 4,
    )?;

    // Vanilla `StructurePlaceSettings` for `placeTemplate`: mirror/rotation from
    // the arguments, an infinite bounding box, known-shape from `strict`, and a
    // `BlockRotProcessor` plus a seeded random only when `integrity < 1.0`.
    let block_rot = [StructureProcessorKind::BlockRot {
        rottable_blocks: None,
        integrity: options.integrity,
    }];
    let processors: &[StructureProcessorKind] = if options.integrity < 1.0 {
        &block_rot
    } else {
        &[]
    };
    let settings = StructurePlaceSettings {
        mirror: options.mirror,
        rotation: options.rotation,
        rotation_pivot: BlockPos::ZERO,
        bounding_box: INFINITE_PLACEMENT_BOX,
        processors,
        block_ignore: StructureBlockIgnore::None,
        late_block_ignore: StructureBlockIgnore::None,
        replace_jigsaws: false,
        projection: None,
        processor_random: if options.integrity < 1.0 {
            StructureProcessorRandom::Placement
        } else {
            StructureProcessorRandom::Positional
        },
        liquid_settings: LiquidSettingsData::ApplyWaterlogging,
    };

    let mut level = LiveLevel::new(world);
    let mut random = WorldgenRandom::from_seed(i64::from(options.seed) as u64);
    if !template.place_in_world(
        &mut level,
        &REGISTRY,
        pos,
        pos,
        &settings,
        &mut random,
        options.update_flags(),
    ) {
        return Err(PlaceError::Failed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::super::create_dispatcher;
    use super::*;
    use crate::bootstrap::init_globals;
    use crate::command::brigadier::{CommandDispatcher, NodeId};
    use crate::command::execution::{SteelArgumentType, SteelCommandRuntime};

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
            panic!("child {name} should exist");
        };
        child
    }

    fn assert_executable(dispatcher: &Dispatcher, node: NodeId) {
        let Some(node) = dispatcher.node(node) else {
            panic!("command node should exist");
        };
        assert!(node.is_executable());
    }

    #[test]
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the private CommandNode type cannot be named from this module"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one table-shaped test keeps the whole /place graph on the same contract"
    )]
    fn place_graph_has_vanilla_subcommands_and_typed_optional_arguments() {
        init_globals();
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let place = child(&dispatcher, dispatcher.root(), "place");

        let feature = child(&dispatcher, place, "feature");
        let feature_id = child(&dispatcher, feature, "feature");
        assert_eq!(
            dispatcher
                .node(feature_id)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::feature())
        );
        assert_executable(&dispatcher, feature_id);
        let feature_pos = child(&dispatcher, feature_id, "pos");
        assert_eq!(
            dispatcher
                .node(feature_pos)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::block_pos())
        );
        assert_executable(&dispatcher, feature_pos);

        let structure = child(&dispatcher, place, "structure");
        let structure_id = child(&dispatcher, structure, "structure");
        assert_eq!(
            dispatcher
                .node(structure_id)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::structure())
        );
        assert_executable(&dispatcher, structure_id);
        let structure_pos = child(&dispatcher, structure_id, "pos");
        assert_eq!(
            dispatcher
                .node(structure_pos)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::block_pos())
        );
        assert_executable(&dispatcher, structure_pos);

        let jigsaw = child(&dispatcher, place, "jigsaw");
        let pool = child(&dispatcher, jigsaw, "pool");
        assert_eq!(
            dispatcher.node(pool).and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::template_pool())
        );
        assert!(
            !dispatcher
                .node(pool)
                .is_some_and(|node| node.is_executable())
        );
        let target = child(&dispatcher, pool, "target");
        assert_eq!(
            dispatcher
                .node(target)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::identifier_argument())
        );
        assert!(
            !dispatcher
                .node(target)
                .is_some_and(|node| node.is_executable())
        );
        let max_depth = child(&dispatcher, target, "max_depth");
        assert_eq!(
            dispatcher
                .node(max_depth)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::from(ArgumentType::integer(1, 20)))
        );
        assert_executable(&dispatcher, max_depth);
        let position = child(&dispatcher, max_depth, "position");
        assert_eq!(
            dispatcher
                .node(position)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::block_pos())
        );
        assert_executable(&dispatcher, position);

        let template = child(&dispatcher, place, "template");
        let template_id = child(&dispatcher, template, "template");
        assert_eq!(
            dispatcher
                .node(template_id)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::identifier_argument())
        );
        assert_executable(&dispatcher, template_id);
        let template_pos = child(&dispatcher, template_id, "pos");
        assert_executable(&dispatcher, template_pos);
        let rotation = child(&dispatcher, template_pos, "rotation");
        assert_eq!(
            dispatcher
                .node(rotation)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::template_rotation())
        );
        assert_executable(&dispatcher, rotation);
        let mirror = child(&dispatcher, rotation, "mirror");
        assert_eq!(
            dispatcher
                .node(mirror)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::template_mirror())
        );
        assert_executable(&dispatcher, mirror);
        let integrity = child(&dispatcher, mirror, "integrity");
        assert_eq!(
            dispatcher
                .node(integrity)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::from(ArgumentType::float(0.0, 1.0)))
        );
        assert_executable(&dispatcher, integrity);
        let seed = child(&dispatcher, integrity, "seed");
        assert_eq!(
            dispatcher.node(seed).and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::from(ArgumentType::integer(
                i32::MIN,
                i32::MAX
            )))
        );
        assert_executable(&dispatcher, seed);
        let strict = child(&dispatcher, seed, "strict");
        assert_eq!(
            dispatcher
                .node(strict)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::from(ArgumentType::bool()))
        );
        assert_executable(&dispatcher, strict);
        assert!(dispatcher.children(strict).is_some_and(<[_]>::is_empty));
    }
}
