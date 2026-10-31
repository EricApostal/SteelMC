//! Vanilla `/data` command.
//!
//! Mirrors `DataCommands` for the `get`, `merge`, `modify` and `remove` verbs
//! across vanilla's three `DataAccessor` providers: `entity`, `block` and
//! `storage`. `modify` covers the `insert`/`prepend`/`append`/`set`/`merge`
//! operations with `value` sources; the `from`/`string` forms are not wired
//! yet, and `compute` waits on a loot-context subsystem for vanilla's
//! `ResourceOrIdArgument` number providers.

use std::sync::Arc;

use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::{
    BlockPos, Identifier,
    nbt::{
        NbtNumeric as _, NbtPath, NbtPathMutationError, merge_nbt_compounds, nbt_compounds_equal,
        to_canonical_snbt,
    },
    text::command_nbt_component,
    translations,
};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use super::execute::loaded_block_position;
use crate::{
    block_entity::SharedBlockEntity, command::storage::CommandStorage, entity::SharedEntity,
    entity::ai::behavior::villager, permission::PermissionLevel, world::World,
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Vanilla `NbtPathArgument.NbtPath.isTooDeep`'s maximum depth.
const MAX_NBT_DEPTH: usize = 512;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("data"), |_| command())
        // Vanilla `DataCommands.register` requires `LEVEL_GAMEMASTERS`.
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> Builder {
    let mut root = literal("data");
    // Vanilla's `TARGET_PROVIDERS` order is entity, block, storage.
    for target in [DataTarget::Entity, DataTarget::Block, DataTarget::Storage] {
        root = root
            .then(merge_target(target))
            .then(get_target(target))
            .then(remove_target(target))
            .then(modify_target(target));
    }
    root
}

/// One vanilla `/data` target provider, matching `DataCommands.TARGET_PROVIDERS`.
#[derive(Clone, Copy)]
pub(super) enum DataTarget {
    Entity,
    Block,
    Storage,
}

impl DataTarget {
    /// Provider literal, matching `EntityDataAccessor`/`BlockDataAccessor`/`StorageDataAccessor`.
    pub(super) const fn provider(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Block => "block",
            Self::Storage => "storage",
        }
    }

    /// Provider argument type. Vanilla's `ArgProvider.buildList("target", ...)`
    /// names every provider argument `target`.
    pub(super) fn argument_type(self) -> SteelArgumentType {
        match self {
            Self::Entity => SteelArgumentType::entity(),
            Self::Block => SteelArgumentType::block_pos(),
            Self::Storage => SteelArgumentType::storage_key(),
        }
    }
}

/// Vanilla `DataAccessor` for one resolved `/data` target.
pub(super) enum DataAccessor<'a> {
    Entity(SharedEntity),
    Block {
        position: BlockPos,
        block_entity: SharedBlockEntity,
        world: &'a Arc<World>,
    },
    Storage {
        id: Identifier,
        storage: &'a CommandStorage,
    },
}

impl DataAccessor<'_> {
    /// Vanilla `DataAccessor#getData`.
    pub(super) fn data(&self) -> NbtCompound {
        match self {
            Self::Entity(entity) => entity.nbt_for_data_compare(),
            Self::Block { block_entity, .. } => block_entity.save_with_full_metadata(),
            Self::Storage { id, storage } => storage.get(id),
        }
    }

    /// Vanilla `DataAccessor#setData`.
    fn set_data(&self, data: NbtCompound) -> Result<(), CommandSyntaxError> {
        match self {
            // Vanilla `EntityDataAccessor#setData` rejects players outright.
            Self::Entity(entity) => {
                if entity.as_player().is_some() {
                    return Err(CommandSyntaxError::dynamic(TextComponent::from(
                        &translations::COMMANDS_DATA_ENTITY_INVALID,
                    )));
                }
                entity.load_with_owned_components(&data).map_err(|error| {
                    let id = entity.id();
                    CommandSyntaxError::dynamic(format!(
                        "Failed to apply NBT to entity {id}: {error}"
                    ))
                })?;
                // Vanilla `Entity#postDataManipulated`; only villagers override it.
                if let Some(villager_entity) = villager::as_villager(entity) {
                    villager_entity.post_data_manipulated();
                }
                Ok(())
            }
            // Vanilla `BlockDataAccessor#setData` reloads, marks the chunk dirty
            // and re-sends the block entity.
            Self::Block {
                position,
                block_entity,
                world,
            } => {
                block_entity
                    .load_with_owned_components(&data)
                    .map_err(|error| {
                        CommandSyntaxError::dynamic(format!(
                            "Failed to apply block entity NBT at {position:?}: {error}"
                        ))
                    })?;
                block_entity.set_changed();
                world.send_block_updated(*position);
                Ok(())
            }
            Self::Storage { id, storage } => {
                storage.set(id.clone(), data);
                Ok(())
            }
        }
    }

    /// Vanilla `DataAccessor#getModifiedSuccess`.
    fn modified_message(&self) -> TextComponent {
        match self {
            Self::Entity(entity) => translations::COMMANDS_DATA_ENTITY_MODIFIED
                .message([entity.display_name()])
                .component(),
            Self::Block { position, .. } => translations::COMMANDS_DATA_BLOCK_MODIFIED
                .message([
                    TextComponent::from(position.x().to_string()),
                    TextComponent::from(position.y().to_string()),
                    TextComponent::from(position.z().to_string()),
                ])
                .component(),
            Self::Storage { id, .. } => translations::COMMANDS_DATA_STORAGE_MODIFIED
                .message([TextComponent::from(id.to_string())])
                .component(),
        }
    }

    /// Vanilla `DataAccessor#getPrintSuccess(Tag)`.
    fn query_message(&self, tag: &NbtTag) -> TextComponent {
        let data = command_nbt_component(tag, false);
        match self {
            Self::Entity(entity) => translations::COMMANDS_DATA_ENTITY_QUERY
                .message([entity.display_name(), data])
                .component(),
            Self::Block { position, .. } => translations::COMMANDS_DATA_BLOCK_QUERY
                .message([
                    TextComponent::from(position.x().to_string()),
                    TextComponent::from(position.y().to_string()),
                    TextComponent::from(position.z().to_string()),
                    data,
                ])
                .component(),
            Self::Storage { id, .. } => translations::COMMANDS_DATA_STORAGE_QUERY
                .message([TextComponent::from(id.to_string()), data])
                .component(),
        }
    }

    /// Vanilla `DataAccessor#getPrintSuccess(NbtPath, double, int)`.
    fn numeric_message(&self, path: &NbtPath, scale: f64, value: i32) -> TextComponent {
        let path = TextComponent::from(path.as_str().to_owned());
        // Vanilla formats the scale with `String.format(Locale.ROOT, "%.2f", scale)`.
        let scale = TextComponent::from(format!("{scale:.2}"));
        let value = TextComponent::from(value.to_string());
        match self {
            Self::Entity(entity) => translations::COMMANDS_DATA_ENTITY_GET
                .message([path, entity.display_name(), scale, value])
                .component(),
            Self::Block { position, .. } => translations::COMMANDS_DATA_BLOCK_GET
                .message([
                    path,
                    TextComponent::from(position.x().to_string()),
                    TextComponent::from(position.y().to_string()),
                    TextComponent::from(position.z().to_string()),
                    scale,
                    value,
                ])
                .component(),
            Self::Storage { id, .. } => translations::COMMANDS_DATA_STORAGE_GET
                .message([path, TextComponent::from(id.to_string()), scale, value])
                .component(),
        }
    }
}

/// Builds the `get` subtree for one provider, matching `DataCommands.register`.
fn get_target(target: DataTarget) -> Builder {
    literal("get").then(
        literal(target.provider()).then(
            argument("target", target.argument_type())
                .executes(move |context| get_full(context, target))
                .then(
                    argument("path", SteelArgumentType::nbt_path())
                        .executes(move |context| get_tag(context, target))
                        .then(
                            argument("scale", ArgumentType::double(f64::MIN, f64::MAX))
                                .executes(move |context| get_numeric(context, target)),
                        ),
                ),
        ),
    )
}

/// Builds the `merge` subtree for one provider, matching `DataCommands.register`.
fn merge_target(target: DataTarget) -> Builder {
    literal("merge").then(
        literal(target.provider()).then(
            argument("target", target.argument_type()).then(
                argument("nbt", SteelArgumentType::nbt_compound())
                    .executes(move |context| merge_data(context, target)),
            ),
        ),
    )
}

/// Builds the `remove` subtree for one provider, matching `DataCommands.register`.
fn remove_target(target: DataTarget) -> Builder {
    literal("remove").then(
        literal(target.provider()).then(
            argument("target", target.argument_type()).then(
                argument("path", SteelArgumentType::nbt_path())
                    .executes(move |context| remove_data(context, target)),
            ),
        ),
    )
}

/// Builds the `modify` subtree for one provider, mirroring
/// `DataCommands.decorateModification`: every manipulator accepts the `value`,
/// `from` and `string` sources.
fn modify_target(target: DataTarget) -> Builder {
    let indexed = decoration_sources(
        argument("index", ArgumentType::integer(i32::MIN, i32::MAX)),
        DataManipulator::Insert,
        target,
    );
    let target_path = argument("targetPath", SteelArgumentType::nbt_path())
        .then(literal("insert").then(indexed))
        .then(modification_branches(DataManipulator::Prepend, target))
        .then(modification_branches(DataManipulator::Append, target))
        .then(modification_branches(DataManipulator::Set, target))
        .then(modification_branches(DataManipulator::Merge, target));
    literal("modify").then(
        literal(target.provider())
            .then(argument("target", target.argument_type()).then(target_path)),
    )
}

/// One manipulator literal with all three source branches.
fn modification_branches(manipulator: DataManipulator, target: DataTarget) -> Builder {
    decoration_sources(literal(manipulator.name()), manipulator, target)
}

/// Vanilla `DataCommands.decorateModification`: the decorator node (a manipulator
/// literal, or `insert`'s `index` argument) gets the three *sibling* sources
/// `value`/`from`/`string`, each with its own provider chain.
fn decoration_sources(
    decorator: Builder,
    manipulator: DataManipulator,
    target: DataTarget,
) -> Builder {
    decorator
        .then(
            argument("value", SteelArgumentType::nbt_tag())
                .executes(move |context| manipulate_value(context, target, manipulator)),
        )
        .then(from_branch(manipulator, target))
        .then(string_branch(manipulator, target))
}

/// `from <provider> <source> [<sourcePath>]`.
fn from_branch(manipulator: DataManipulator, target: DataTarget) -> Builder {
    let mut branch = literal("from");
    for source in [DataTarget::Entity, DataTarget::Block, DataTarget::Storage] {
        branch = branch.then(
            literal(source.provider()).then(
                argument("source", source.argument_type())
                    .executes(move |context| {
                        manipulate_from(context, target, manipulator, source, false)
                    })
                    .then(
                        argument("sourcePath", SteelArgumentType::nbt_path()).executes(
                            move |context| {
                                manipulate_from(context, target, manipulator, source, true)
                            },
                        ),
                    ),
            ),
        );
    }
    branch
}

/// `string <provider> <source> [<sourcePath>] <start> [<end>]`.
fn string_branch(manipulator: DataManipulator, target: DataTarget) -> Builder {
    let mut branch = literal("string");
    for source in [DataTarget::Entity, DataTarget::Block, DataTarget::Storage] {
        branch = branch.then(
            literal(source.provider()).then(
                argument("source", source.argument_type())
                    .executes(move |context| {
                        manipulate_string(context, target, manipulator, source, false, None, None)
                    })
                    .then(
                        argument("sourcePath", SteelArgumentType::nbt_path())
                            .executes(move |context| {
                                manipulate_string(
                                    context,
                                    target,
                                    manipulator,
                                    source,
                                    true,
                                    None,
                                    None,
                                )
                            })
                            .then(
                                argument("start", ArgumentType::integer(i32::MIN, i32::MAX))
                                    .executes(move |context| {
                                        let start = context.integer("start")?;
                                        manipulate_string(
                                            context,
                                            target,
                                            manipulator,
                                            source,
                                            true,
                                            Some(start),
                                            None,
                                        )
                                    })
                                    .then(
                                        argument("end", ArgumentType::integer(i32::MIN, i32::MAX))
                                            .executes(move |context| {
                                                let start = context.integer("start")?;
                                                let end = context.integer("end")?;
                                                manipulate_string(
                                                    context,
                                                    target,
                                                    manipulator,
                                                    source,
                                                    true,
                                                    Some(start),
                                                    Some(end),
                                                )
                                            }),
                                    ),
                            ),
                    ),
            ),
        );
    }
    branch
}

/// Resolves the provider's `DataAccessor` the way vanilla's `access(context)` does.
///
/// `/function ... with` reuses this, exactly like vanilla's shared `SOURCE_PROVIDERS`.
pub(super) fn resolve(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
) -> Result<DataAccessor<'_>, CommandSyntaxError> {
    resolve_named(context, target, "target")
}

/// Resolves a provider's `DataAccessor` for a named target/source argument.
fn resolve_named<'a>(
    context: &'a SteelCommandContext<CommandSource>,
    target: DataTarget,
    name: &str,
) -> Result<DataAccessor<'a>, CommandSyntaxError> {
    match target {
        DataTarget::Entity => Ok(DataAccessor::Entity(context.entity(name)?)),
        DataTarget::Block => {
            let position = loaded_block_position(context, name)?;
            let source = context.source();
            let block_entity = source
                .world()
                .get_block_entity(position)
                .ok_or_else(block_not_a_block_entity)?;
            Ok(DataAccessor::Block {
                position,
                block_entity,
                world: source.world(),
            })
        }
        DataTarget::Storage => {
            let id = context.identifier(name)?.clone();
            let source = context.source();
            let domain = source.world().domain();
            let storage = source
                .server()
                .command_storage
                .get(domain)
                .ok_or_else(|| missing_command_storage(domain))?;
            Ok(DataAccessor::Storage { id, storage })
        }
    }
}

/// Vanilla `DataCommands.getData(source, accessor)`.
fn get_full(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
) -> Result<i32, CommandSyntaxError> {
    let accessor = resolve(context, target)?;
    let data = NbtTag::Compound(accessor.data());
    let message = accessor.query_message(&data);
    context.source().send_success(&message, false);
    Ok(1)
}

/// Vanilla `DataCommands.getData(source, accessor, path)`.
fn get_tag(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
) -> Result<i32, CommandSyntaxError> {
    let accessor = resolve(context, target)?;
    let path = context.nbt_path("path")?;
    let data = NbtTag::Compound(accessor.data());
    let Some(tag) = single_tag(path, &data)? else {
        return Err(unknown_path(path));
    };

    let result = tag_result(&tag);
    let message = accessor.query_message(&tag);
    context.source().send_success(&message, false);
    Ok(result)
}

/// Vanilla `DataCommands.getNumeric`.
fn get_numeric(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
) -> Result<i32, CommandSyntaxError> {
    let accessor = resolve(context, target)?;
    let path = context.nbt_path("path")?;
    let data = NbtTag::Compound(accessor.data());
    let Some(tag) = single_tag(path, &data)? else {
        return Err(invalid_number(path));
    };
    let Some(value) = tag.codec_f64() else {
        return Err(invalid_number(path));
    };

    let scale = context.double("scale")?;
    let result = (value * scale).floor() as i32;
    let message = accessor.numeric_message(path, scale, result);
    context.source().send_success(&message, false);
    Ok(result)
}

/// Vanilla `DataCommands.mergeData`.
fn merge_data(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
) -> Result<i32, CommandSyntaxError> {
    let accessor = resolve(context, target)?;
    // Vanilla's `CompoundTagArgument` parses a compound, never another value.
    let nbt = context.nbt_compound("nbt")?;
    if compound_is_too_deep(nbt, 0) {
        return Err(data_too_deep());
    }

    let old = accessor.data();
    let mut result = old.clone();
    merge_nbt_compounds(&mut result, nbt);
    if nbt_compounds_equal(&old, &result) {
        return Err(merge_unchanged());
    }

    accessor.set_data(result)?;
    let message = accessor.modified_message();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Vanilla `DataCommands.removeData`.
fn remove_data(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
) -> Result<i32, CommandSyntaxError> {
    let accessor = resolve(context, target)?;
    let path = context.nbt_path("path")?;
    let mut root = NbtTag::Compound(accessor.data());
    let count = path.remove(&mut root);
    if count == 0 {
        return Err(merge_unchanged());
    }
    let NbtTag::Compound(data) = root else {
        return Err(CommandSyntaxError::dynamic(
            "Removing an NBT path replaced the compound root",
        ));
    };

    accessor.set_data(data)?;
    let result = i32::try_from(count).unwrap_or(i32::MAX);
    let message = accessor.modified_message();
    context.source().send_success(&message, true);
    Ok(result)
}

/// One vanilla `DataCommands$DataManipulator` for the `value` branch of
/// `DataCommands.register`'s `insert`/`prepend`/`append`/`set`/`merge`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DataManipulator {
    Set,
    Merge,
    Insert,
    Prepend,
    Append,
}

impl DataManipulator {
    /// Manipulator literal, matching `DataCommands.register`'s `BiConsumer`.
    const fn name(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Merge => "merge",
            Self::Insert => "insert",
            Self::Prepend => "prepend",
            Self::Append => "append",
        }
    }

    /// Insert offset for the list manipulators: `prepend` is `insert(0)`,
    /// `append` is `insert(-1)`, `insert` reads the `index` argument.
    fn insert_index(
        self,
        context: &SteelCommandContext<CommandSource>,
    ) -> Result<i32, CommandSyntaxError> {
        match self {
            Self::Insert => context.integer("index"),
            Self::Append => Ok(-1),
            // `prepend` is `insert(0)`; `set`/`merge` never prepend either.
            Self::Prepend | Self::Set | Self::Merge => Ok(0),
        }
    }
}

/// Vanilla `DataCommands.manipulateData` for the `value` source: wraps
/// `NbtTagArgument.getNbtTag`'s tag in a singleton list like
/// `DataCommands$19` and runs the manipulator.
fn manipulate_value(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
    manipulator: DataManipulator,
) -> Result<i32, CommandSyntaxError> {
    let value = context.nbt_tag("value")?.clone();
    manipulate_data(context, target, manipulator, vec![value])
}

/// Vanilla `DataCommands.getSingletonSource` / `resolveSourcePath` for the
/// `from` source: the source accessor's whole compound, or the tags its
/// `sourcePath` selects.
fn manipulate_from(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
    manipulator: DataManipulator,
    source: DataTarget,
    with_path: bool,
) -> Result<i32, CommandSyntaxError> {
    let values = source_values(context, source, with_path)?;
    manipulate_data(context, target, manipulator, values)
}

/// Vanilla `DataCommands`' `string <source> [<sourcePath>] <start> [<end>]`:
/// stringify the selected tags and slice each string by Java's UTF-16 offsets.
fn manipulate_string(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
    manipulator: DataManipulator,
    source: DataTarget,
    with_path: bool,
    start: Option<i32>,
    end: Option<i32>,
) -> Result<i32, CommandSyntaxError> {
    let values = source_values(context, source, with_path)?;
    let values = values
        .iter()
        .map(|tag| {
            let text = get_as_text(tag)?;
            substring(&text, start, end).map(|text| NbtTag::String(text.into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    manipulate_data(context, target, manipulator, values)
}

/// The source tags vanilla's `getSingletonSource`/`resolveSourcePath` produce.
fn source_values(
    context: &SteelCommandContext<CommandSource>,
    source: DataTarget,
    with_path: bool,
) -> Result<Vec<NbtTag>, CommandSyntaxError> {
    let accessor = resolve_named(context, source, "source")?;
    let data = NbtTag::Compound(accessor.data());
    if with_path {
        Ok(context.nbt_path("sourcePath")?.get(&data))
    } else {
        Ok(vec![data])
    }
}

/// Vanilla `DataCommands.getAsText`: strings stay unquoted, numeric tags use
/// their SNBT `toString`, and every other tag is `ERROR_EXPECTED_VALUE`.
fn get_as_text(tag: &NbtTag) -> Result<String, CommandSyntaxError> {
    match tag {
        NbtTag::String(value) => Ok(value.to_string_lossy().into_owned()),
        NbtTag::Byte(_)
        | NbtTag::Short(_)
        | NbtTag::Int(_)
        | NbtTag::Long(_)
        | NbtTag::Float(_)
        | NbtTag::Double(_) => to_canonical_snbt(tag).ok_or_else(|| expected_value(tag)),
        _ => Err(expected_value(tag)),
    }
}

/// Vanilla `DataCommands.ERROR_EXPECTED_VALUE` (`commands.data.modify.expected_value`).
fn expected_value(tag: &NbtTag) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_DATA_MODIFY_EXPECTED_VALUE
            .message([command_nbt_component(tag, false)])
            .component(),
    )
}

/// Vanilla `DataCommands.substring(String, Integer, Integer)`: Java's
/// `String.substring` offsets over UTF-16 code units, with the invalid-range
/// error carrying the resolved offsets.
fn substring(
    value: &str,
    start: Option<i32>,
    end: Option<i32>,
) -> Result<String, CommandSyntaxError> {
    let units: Vec<u16> = value.encode_utf16().collect();
    let length = i32::try_from(units.len()).unwrap_or(i32::MAX);
    let start = start.map_or(0, |start| offset(start, length));
    let end = end.map_or(length, |end| offset(end, length));
    if start < 0 || end > length || start > end {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_DATA_MODIFY_INVALID_SUBSTRING
                .message([start.to_string(), end.to_string()])
                .component(),
        ));
    }
    let start = usize::try_from(start).unwrap_or(0);
    let end = usize::try_from(end).unwrap_or(0);
    String::from_utf16(&units[start..end]).map_err(|_| {
        CommandSyntaxError::dynamic("NBT string contains an unpaired UTF-16 surrogate")
    })
}

/// Vanilla `DataCommands.getOffset`.
const fn offset(index: i32, length: i32) -> i32 {
    if index < 0 { length + index } else { index }
}

/// Vanilla `DataCommands.manipulateData`: applies one `DataManipulator` to the
/// target's compound and writes it back, failing with `ERROR_MERGE_UNCHANGED`
/// (`commands.data.merge.failed`) when nothing changed.
fn manipulate_data(
    context: &SteelCommandContext<CommandSource>,
    target: DataTarget,
    manipulator: DataManipulator,
    values: Vec<NbtTag>,
) -> Result<i32, CommandSyntaxError> {
    let accessor = resolve(context, target)?;
    let path = context.nbt_path("targetPath")?.clone();
    let mut root = NbtTag::Compound(accessor.data());

    // Vanilla `DataCommands$9`/`$10`/`$11` read the index from the context
    // (`insert`) or hard-code it (`prepend` 0, `append` -1).
    let modified = match manipulator {
        DataManipulator::Set => set_data_tag(&path, &mut root, &values)?,
        DataManipulator::Merge => merge_data_tags(&path, &mut root, &values)?,
        DataManipulator::Insert | DataManipulator::Prepend | DataManipulator::Append => {
            insert_data_tags(
                &path,
                &mut root,
                manipulator.insert_index(context)?,
                &values,
            )?
        }
    };

    if modified == 0 {
        return Err(merge_unchanged());
    }
    let NbtTag::Compound(data) = root else {
        return Err(CommandSyntaxError::dynamic(
            "Modifying an NBT path replaced the compound root",
        ));
    };

    accessor.set_data(data)?;
    let message = accessor.modified_message();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Vanilla `NbtPath.set` manipulator: the value branch always supplies one tag,
/// mirroring `Iterables.getLast(list)` in `DataCommands$12`.
fn set_data_tag(
    path: &NbtPath,
    root: &mut NbtTag,
    values: &[NbtTag],
) -> Result<usize, CommandSyntaxError> {
    let Some(value) = values.last() else {
        return Ok(0);
    };
    path.set(root, value.clone()).map_err(mutation_error)
}

/// Vanilla `DataCommands$13` (merge): every value is checked for depth, must be
/// a compound, is merged into one accumulator, and that accumulator is merged
/// into every tag `getOrCreate` selects, counting real changes.
fn merge_data_tags(
    path: &NbtPath,
    root: &mut NbtTag,
    values: &[NbtTag],
) -> Result<usize, CommandSyntaxError> {
    let mut merged = NbtCompound::new();
    for value in values {
        // Vanilla `DataCommands$13` checks each value with `isTooDeep(value, 0)`.
        if tag_is_too_deep(value, 0) {
            return Err(data_too_deep());
        }
        let NbtTag::Compound(compound) = value else {
            return Err(expected_object(value));
        };
        merge_nbt_compounds(&mut merged, compound);
    }

    // Vanilla `DataCommands$13` gets the whole selection back when there is
    // nothing to create and errors on the first non-compound it finds.
    for selected in path.get(root) {
        if !matches!(selected, NbtTag::Compound(_)) {
            return Err(expected_object(&selected));
        }
    }

    path.get_or_create(root, NbtTag::Compound(NbtCompound::new()), |tag| {
        // The pre-check above guarantees every selected tag is a compound:
        // creation only inserts compound defaults.
        let NbtTag::Compound(selected) = tag else {
            return Ok(false);
        };
        let before = selected.clone();
        merge_nbt_compounds(selected, &merged);
        Ok(*selected != before)
    })
    .map_err(mutation_error)
}

/// Vanilla `DataCommands$9`/`$10`/`$11` (insert/prepend/append): values are
/// checked for depth inside `NbtPath.insert`, then inserted so the first copy
/// stays with the first collection, matching `NbtPath.insert`.
fn insert_data_tags(
    path: &NbtPath,
    root: &mut NbtTag,
    index: i32,
    values: &[NbtTag],
) -> Result<usize, CommandSyntaxError> {
    path.insert(index, root, values).map_err(mutation_error)
}

/// Vanilla `DataCommands.ERROR_EXPECTED_OBJECT` (`commands.data.modify.expected_object`).
fn expected_object(tag: &NbtTag) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_DATA_MODIFY_EXPECTED_OBJECT
            .message([command_nbt_component(tag, false)])
            .component(),
    )
}

/// Maps a path mutation error to its vanilla translation.
fn mutation_error(error: NbtPathMutationError) -> CommandSyntaxError {
    match error {
        NbtPathMutationError::TooDeep => data_too_deep(),
        NbtPathMutationError::NothingFound(prefix) => CommandSyntaxError::dynamic(
            translations::ARGUMENTS_NBTPATH_NOTHING_FOUND
                .message([TextComponent::from(prefix)])
                .component(),
        ),
        NbtPathMutationError::ExpectedList(tag) => CommandSyntaxError::dynamic(
            translations::COMMANDS_DATA_MODIFY_EXPECTED_LIST
                .message([TextComponent::from(tag)])
                .component(),
        ),
        NbtPathMutationError::InvalidIndex(index) => CommandSyntaxError::dynamic(
            translations::COMMANDS_DATA_MODIFY_INVALID_INDEX
                .message([index.to_string()])
                .component(),
        ),
    }
}

/// Vanilla `DataCommands.getSingleTag` without vanilla's `EndTag` placeholder.
///
/// `None` stands in for a path that selected nothing; vanilla selects
/// `EndTag.INSTANCE` in that case and lets each caller pick its own error.
pub(super) fn single_tag(
    path: &NbtPath,
    data: &NbtTag,
) -> Result<Option<NbtTag>, CommandSyntaxError> {
    let mut tags = path.get(data);
    if tags.len() > 1 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_DATA_GET_MULTIPLE,
        )));
    }
    Ok(tags.pop())
}

/// Vanilla `DataCommands.getData`'s value switch for one selected tag.
fn tag_result(tag: &NbtTag) -> i32 {
    match tag {
        // `NumericTag.doubleValue()` floored through `Mth.floor`.
        NbtTag::Byte(value) => f64::from(*value).floor() as i32,
        NbtTag::Short(value) => f64::from(*value).floor() as i32,
        NbtTag::Int(value) => f64::from(*value).floor() as i32,
        NbtTag::Long(value) => (*value as f64).floor() as i32,
        NbtTag::Float(value) => f64::from(*value).floor() as i32,
        NbtTag::Double(value) => value.floor() as i32,
        // `CollectionTag.size()`, `CompoundTag.size()` and `StringTag.length()`.
        NbtTag::List(list) => i32::try_from(list_len(list)).unwrap_or(i32::MAX),
        NbtTag::ByteArray(values) => i32::try_from(values.len()).unwrap_or(i32::MAX),
        NbtTag::IntArray(values) => i32::try_from(values.len()).unwrap_or(i32::MAX),
        NbtTag::LongArray(values) => i32::try_from(values.len()).unwrap_or(i32::MAX),
        NbtTag::Compound(compound) => i32::try_from(compound.len()).unwrap_or(i32::MAX),
        // Java `String.length()` counts UTF-16 code units.
        NbtTag::String(value) => {
            i32::try_from(value.to_string_lossy().encode_utf16().count()).unwrap_or(i32::MAX)
        }
    }
}

/// Vanilla `CollectionTag.size()` for a list tag.
const fn list_len(list: &NbtList) -> usize {
    match list {
        NbtList::Empty => 0,
        NbtList::Byte(values) => values.len(),
        NbtList::Short(values) => values.len(),
        NbtList::Int(values) => values.len(),
        NbtList::Long(values) => values.len(),
        NbtList::Float(values) => values.len(),
        NbtList::Double(values) => values.len(),
        NbtList::ByteArray(values) => values.len(),
        NbtList::String(values) => values.len(),
        NbtList::List(values) => values.len(),
        NbtList::Compound(values) => values.len(),
        NbtList::IntArray(values) => values.len(),
        NbtList::LongArray(values) => values.len(),
    }
}

/// Vanilla `NbtPathArgument.NbtPath.isTooDeep` for the root compound `/data merge` accepts.
///
/// `steel_utils` keeps its own copy private to the path mutators, so `/data`
/// repeats the depth rule for vanilla's `merge` guard.
fn compound_is_too_deep(compound: &NbtCompound, depth: usize) -> bool {
    if depth >= MAX_NBT_DEPTH {
        return true;
    }
    compound
        .values()
        .any(|child| tag_is_too_deep(child, depth + 1))
}

fn tag_is_too_deep(tag: &NbtTag, depth: usize) -> bool {
    if depth >= MAX_NBT_DEPTH {
        return true;
    }
    match tag {
        NbtTag::Compound(compound) => compound_is_too_deep(compound, depth),
        NbtTag::List(list) => list_is_too_deep(list, depth),
        _ => false,
    }
}

fn list_is_too_deep(list: &NbtList, depth: usize) -> bool {
    if depth >= MAX_NBT_DEPTH {
        return true;
    }
    match list {
        NbtList::Empty => false,
        NbtList::List(values) => values
            .iter()
            .any(|child| list_is_too_deep(child, depth + 1)),
        NbtList::Compound(values) => values
            .iter()
            .any(|child| compound_is_too_deep(child, depth + 1)),
        // Every other list holds leaf elements, which only report the limit.
        _ => depth + 1 >= MAX_NBT_DEPTH,
    }
}

/// Vanilla `DataCommands.ERROR_MERGE_UNCHANGED` (`commands.data.merge.failed`).
fn merge_unchanged() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_DATA_MERGE_FAILED,
    ))
}

/// Vanilla `NbtPathArgument.ERROR_DATA_TOO_DEEP` (`arguments.nbtpath.too_deep`).
fn data_too_deep() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::ARGUMENTS_NBTPATH_TOO_DEEP,
    ))
}

/// Vanilla `DataCommands.ERROR_GET_NON_EXISTENT` (`commands.data.get.unknown`).
pub(super) fn unknown_path(path: &NbtPath) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_DATA_GET_UNKNOWN
            .message([TextComponent::from(path.as_str().to_owned())])
            .component(),
    )
}

/// Vanilla `DataCommands.ERROR_GET_NOT_NUMBER` (`commands.data.get.invalid`).
fn invalid_number(path: &NbtPath) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_DATA_GET_INVALID
            .message([TextComponent::from(path.as_str().to_owned())])
            .component(),
    )
}

/// Vanilla `BlockDataAccessor.ERROR_NOT_A_BLOCK_ENTITY`.
fn block_not_a_block_entity() -> CommandSyntaxError {
    CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_DATA_BLOCK_INVALID,
    ))
}

/// Guard mirroring `/execute store`'s missing-domain-storage diagnostic.
fn missing_command_storage(domain: &str) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(format!("Domain '{domain}' has no command storage"))
}

#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;
    use steel_utils::nbt::parse_nbt_path;
    use text_components::content::Content;

    use super::super::create_dispatcher;
    use super::*;
    use crate::command::brigadier::{CommandDispatcher, NodeId};

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
            let names: Vec<String> = children
                .iter()
                .filter_map(|child| dispatcher.node(*child).map(|node| node.name().to_owned()))
                .collect();
            panic!("child {name} should exist; children = {names:?}");
        };
        child
    }

    fn argument_node(
        dispatcher: &Dispatcher,
        parent: NodeId,
        name: &str,
        expected: &SteelArgumentType,
    ) -> NodeId {
        let node = child(dispatcher, parent, name);
        let Some(actual) = dispatcher.node(node) else {
            panic!("{name} argument should exist");
        };
        assert_eq!(actual.argument_type(), Some(expected));
        node
    }

    fn assert_executable(dispatcher: &Dispatcher, node: NodeId) {
        let Some(node) = dispatcher.node(node) else {
            panic!("command node should exist");
        };
        assert!(node.is_executable());
    }

    /// Asserts `value`, `from` and `string` under one manipulator node, with
    /// vanilla's provider arguments and `sourcePath`/`start`/`end` chain.
    fn assert_source_branches(dispatcher: &Dispatcher, operation: NodeId) {
        let value = argument_node(
            dispatcher,
            operation,
            "value",
            &SteelArgumentType::nbt_tag(),
        );
        assert_executable(dispatcher, value);

        let integer = SteelArgumentType::from(ArgumentType::integer(i32::MIN, i32::MAX));
        let from = child(dispatcher, operation, "from");
        let string = child(dispatcher, operation, "string");
        for (provider, argument_type) in [
            ("entity", SteelArgumentType::entity()),
            ("block", SteelArgumentType::block_pos()),
            ("storage", SteelArgumentType::storage_key()),
        ] {
            let from_source = argument_node(
                dispatcher,
                child(dispatcher, from, provider),
                "source",
                &argument_type,
            );
            argument_node(
                dispatcher,
                from_source,
                "sourcePath",
                &SteelArgumentType::nbt_path(),
            );

            let string_source = argument_node(
                dispatcher,
                child(dispatcher, string, provider),
                "source",
                &argument_type,
            );
            let source_path = argument_node(
                dispatcher,
                string_source,
                "sourcePath",
                &SteelArgumentType::nbt_path(),
            );
            let start = argument_node(dispatcher, source_path, "start", &integer);
            argument_node(dispatcher, start, "end", &integer);
        }
    }

    #[test]
    fn tree_matches_vanilla_provider_shape() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let data = child(&dispatcher, dispatcher.root(), "data");

        for (provider, argument_type) in [
            ("entity", SteelArgumentType::entity()),
            ("block", SteelArgumentType::block_pos()),
            ("storage", SteelArgumentType::storage_key()),
        ] {
            let get = child(&dispatcher, child(&dispatcher, data, "get"), provider);
            let target = argument_node(&dispatcher, get, "target", &argument_type);
            // `get <target>` prints the whole compound.
            assert_executable(&dispatcher, target);

            let path = argument_node(&dispatcher, target, "path", &SteelArgumentType::nbt_path());
            let scale = argument_node(
                &dispatcher,
                path,
                "scale",
                &SteelArgumentType::from(ArgumentType::double(f64::MIN, f64::MAX)),
            );
            assert_executable(&dispatcher, path);
            assert_executable(&dispatcher, scale);

            let merge = child(&dispatcher, child(&dispatcher, data, "merge"), provider);
            let merge_target = argument_node(&dispatcher, merge, "target", &argument_type);
            let nbt = argument_node(
                &dispatcher,
                merge_target,
                "nbt",
                &SteelArgumentType::nbt_compound(),
            );
            assert_executable(&dispatcher, nbt);

            let remove = child(&dispatcher, child(&dispatcher, data, "remove"), provider);
            let remove_target = argument_node(&dispatcher, remove, "target", &argument_type);
            let remove_path = argument_node(
                &dispatcher,
                remove_target,
                "path",
                &SteelArgumentType::nbt_path(),
            );
            assert_executable(&dispatcher, remove_path);

            let provider_modify = child(&dispatcher, child(&dispatcher, data, "modify"), provider);
            let modify_target =
                argument_node(&dispatcher, provider_modify, "target", &argument_type);
            let target_path = argument_node(
                &dispatcher,
                modify_target,
                "targetPath",
                &SteelArgumentType::nbt_path(),
            );

            for name in ["prepend", "append", "set", "merge"] {
                let operation = child(&dispatcher, target_path, name);
                let value = argument_node(
                    &dispatcher,
                    operation,
                    "value",
                    &SteelArgumentType::nbt_tag(),
                );
                assert_executable(&dispatcher, value);
                assert_source_branches(&dispatcher, operation);
            }
            let insert = child(&dispatcher, target_path, "insert");
            let insert_index = argument_node(
                &dispatcher,
                insert,
                "index",
                &SteelArgumentType::from(ArgumentType::integer(i32::MIN, i32::MAX)),
            );
            // Vanilla's `insert` decorator adds the source branches directly
            // under the index: `insert <index> value <value>`.
            assert_source_branches(&dispatcher, insert_index);
        }
    }

    #[test]
    fn string_source_uses_vanilla_text_and_utf16_slices() {
        assert_eq!(
            get_as_text(&NbtTag::String("plain".into())).expect("string tag"),
            "plain"
        );
        assert_eq!(get_as_text(&NbtTag::Int(7)).expect("int tag"), "7");
        assert_eq!(get_as_text(&NbtTag::Float(1.0)).expect("float tag"), "1.0f");
        assert_eq!(
            get_as_text(&NbtTag::Double(2.5)).expect("double tag"),
            "2.5d"
        );

        let unsupported = get_as_text(&NbtTag::List(NbtList::Int(vec![1])))
            .expect_err("only string and numeric tags stringify");
        assert_eq!(
            message_key(&unsupported.message_component()),
            "commands.data.modify.expected_value"
        );

        assert_eq!(
            substring("abcdef", Some(1), Some(3)).expect("inside range"),
            "bc"
        );
        assert_eq!(
            substring("abcdef", Some(-2), None).expect("negative start"),
            "ef"
        );
        // Java substring indexes UTF-16 code units.
        assert_eq!(
            substring("a😀b", Some(0), Some(3)).expect("surrogate pair boundary"),
            "a😀"
        );

        let invalid = substring("abc", Some(1), Some(9)).expect_err("end is out of range");
        assert_eq!(
            message_key(&invalid.message_component()),
            "commands.data.modify.invalid_substring"
        );
    }

    #[test]
    fn tag_results_follow_vanilla_value_kinds() {
        assert_eq!(tag_result(&NbtTag::Byte(3)), 3);
        assert_eq!(tag_result(&NbtTag::Short(-2)), -2);
        assert_eq!(tag_result(&NbtTag::Int(7)), 7);
        assert_eq!(tag_result(&NbtTag::Long(i64::MAX)), i32::MAX);
        assert_eq!(tag_result(&NbtTag::Float(2.75)), 2);
        assert_eq!(tag_result(&NbtTag::Double(-1.5)), -2);

        let mut compound = NbtCompound::new();
        compound.insert("one", NbtTag::Int(1));
        compound.insert("two", NbtTag::Int(2));
        assert_eq!(tag_result(&NbtTag::Compound(compound)), 2);

        assert_eq!(tag_result(&NbtTag::List(NbtList::Int(vec![1, 2, 3]))), 3);
        assert_eq!(tag_result(&NbtTag::List(NbtList::Empty)), 0);
        assert_eq!(tag_result(&NbtTag::ByteArray(vec![1, 2])), 2);
        assert_eq!(tag_result(&NbtTag::IntArray(vec![1, 2, 3])), 3);
        assert_eq!(tag_result(&NbtTag::LongArray(vec![1])), 1);
        assert_eq!(tag_result(&NbtTag::String("abc".into())), 3);
        // Java's `String.length()` counts UTF-16 code units, not scalars.
        assert_eq!(tag_result(&NbtTag::String("a😀".into())), 3);
    }

    #[test]
    fn single_tag_distinguishes_missing_and_multiple_matches() {
        let path = parse_nbt_path("values[]").expect("path should parse");
        let mut compound = NbtCompound::new();
        compound.insert("values", NbtList::Int(vec![1, 2]));
        let data = NbtTag::Compound(compound);

        assert!(single_tag(&path, &data).is_err());

        let missing = parse_nbt_path("absent").expect("path should parse");
        assert_eq!(
            single_tag(&missing, &data).expect("missing path is not an error"),
            None
        );

        let single = parse_nbt_path("values[0]").expect("path should parse");
        assert_eq!(
            single_tag(&single, &data).expect("single match should resolve"),
            Some(NbtTag::Int(1))
        );
    }

    #[test]
    fn merge_and_remove_follow_vanilla_compound_semantics() {
        let mut data = NbtCompound::new();
        data.insert("keep", NbtTag::Int(1));
        let mut nested = NbtCompound::new();
        nested.insert("inner", NbtTag::Int(2));
        data.insert("nested", nested);

        let mut incoming = NbtCompound::new();
        incoming.insert("added", NbtTag::Int(3));
        let mut incoming_nested = NbtCompound::new();
        incoming_nested.insert("deep", NbtTag::Int(4));
        incoming.insert("nested", incoming_nested);

        let mut merged = data.clone();
        merge_nbt_compounds(&mut merged, &incoming);
        assert_eq!(merged.int("keep"), Some(1));
        assert_eq!(merged.int("added"), Some(3));
        let nested = merged.compound("nested").expect("nested stays compound");
        assert_eq!(nested.int("inner"), Some(2));
        assert_eq!(nested.int("deep"), Some(4));
        assert!(!nbt_compounds_equal(&data, &merged));

        let mut root = NbtTag::Compound(data);
        let path = parse_nbt_path("keep").expect("path should parse");
        assert_eq!(path.remove(&mut root), 1);
        assert_eq!(path.remove(&mut root), 0);
        let NbtTag::Compound(result) = root else {
            panic!("removing a child keeps the compound root");
        };
        assert_eq!(result.int("keep"), None);
    }

    #[test]
    fn depth_guard_matches_vanilla_limit() {
        fn nested_compound(depth: usize) -> NbtTag {
            let mut tag = NbtTag::Int(1);
            for _ in 0..depth {
                let mut compound = NbtCompound::new();
                compound.insert("child", tag);
                tag = NbtTag::Compound(compound);
            }
            tag
        }

        // `nested_compound(n)` puts the leaf at depth `n` below the root compound.
        assert!(!compound_is_too_deep(&NbtCompound::new(), 0));
        for (depth, expected) in [(1, false), (511, false), (512, true)] {
            let NbtTag::Compound(root) = nested_compound(depth) else {
                panic!("nested compound should stay a compound");
            };
            assert_eq!(
                compound_is_too_deep(&root, 0),
                expected,
                "depth {depth} should match vanilla's 512 limit"
            );
        }
    }

    /// Parses a path without panicking in every modify assertion below.
    fn modify_path(input: &str) -> NbtPath {
        parse_nbt_path(input).expect("modify path should parse")
    }

    /// Unwraps a mutated root back into a compound.
    fn root_compound(root: NbtTag) -> NbtCompound {
        let NbtTag::Compound(compound) = root else {
            panic!("modify keeps the compound root");
        };
        compound
    }

    /// Returns the vanilla translation key of a command error message.
    fn message_key(component: &TextComponent) -> &str {
        let Content::Translate(message) = &component.content else {
            panic!("modify errors stay translatable, got {component:?}");
        };
        &message.key
    }

    #[test]
    fn modify_set_follows_vanilla_list_compound_and_array_semantics() {
        // Lists: one index is replaced, and an unchanged value reports nothing.
        let mut root = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            compound.insert("values", NbtList::Int(vec![1, 2, 3]));
            compound
        });
        let path = modify_path("values[1]");
        assert_eq!(
            set_data_tag(&path, &mut root, &[NbtTag::Int(9)]).expect("set should apply"),
            1
        );
        assert_eq!(
            root_compound(root.clone()).list("values"),
            Some(&NbtList::Int(vec![1, 9, 3]))
        );
        assert_eq!(
            set_data_tag(&path, &mut root, &[NbtTag::Int(9)]).expect("set should apply"),
            0
        );

        // Compounds: missing parents are created, matching `getOrCreateParents`.
        let mut created = NbtTag::Compound(NbtCompound::new());
        let path = modify_path("a.b.c");
        assert_eq!(
            set_data_tag(&path, &mut created, &[NbtTag::String("leaf".into())]).expect("apply"),
            1
        );
        let compound = root_compound(created);
        let inner = compound
            .compound("a")
            .and_then(|a| a.compound("b"))
            .expect("set creates each missing compound parent");
        assert_eq!(
            inner.string("c").map(|value| value.to_str().into_owned()),
            Some("leaf".to_owned())
        );

        // Typed arrays keep their element type.
        let mut arrays = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            compound.insert("bytes", NbtTag::ByteArray(vec![1, 2]));
            compound
        });
        let path = modify_path("bytes[0]");
        assert_eq!(
            set_data_tag(&path, &mut arrays, &[NbtTag::Byte(7)]).expect("apply"),
            1
        );
        assert_eq!(
            root_compound(arrays).get("bytes"),
            Some(&NbtTag::ByteArray(vec![7, 2]))
        );
    }

    #[test]
    fn modify_insert_prepend_append_use_vanilla_indices() {
        fn list_root() -> NbtTag {
            NbtTag::Compound({
                let mut compound = NbtCompound::new();
                compound.insert("values", NbtList::Int(vec![1, 2, 3]));
                compound
            })
        }
        let path = modify_path("values");

        // `insert <index> value <value>`.
        let mut root = list_root();
        assert_eq!(
            insert_data_tags(&path, &mut root, 1, &[NbtTag::Int(9)]).expect("insert should apply"),
            1
        );
        assert_eq!(
            root_compound(root).list("values"),
            Some(&NbtList::Int(vec![1, 9, 2, 3]))
        );

        // `prepend` is `insert 0`.
        let mut root = list_root();
        assert_eq!(
            insert_data_tags(&path, &mut root, 0, &[NbtTag::Int(9)]).expect("prepend should apply"),
            1
        );
        assert_eq!(
            root_compound(root).list("values"),
            Some(&NbtList::Int(vec![9, 1, 2, 3]))
        );

        // `append` is `insert -1`, which vanilla resolves as `size + index + 1`.
        let mut root = list_root();
        assert_eq!(
            insert_data_tags(&path, &mut root, -1, &[NbtTag::Int(9)]).expect("append should apply"),
            1
        );
        assert_eq!(
            root_compound(root).list("values"),
            Some(&NbtList::Int(vec![1, 2, 3, 9]))
        );

        // Every inserted value advances the index, so order is preserved.
        let mut root = list_root();
        assert_eq!(
            insert_data_tags(&path, &mut root, 1, &[NbtTag::Int(8), NbtTag::Int(9)])
                .expect("apply"),
            1
        );
        assert_eq!(
            root_compound(root).list("values"),
            Some(&NbtList::Int(vec![1, 8, 9, 2, 3]))
        );

        // Typed arrays accept vanilla's insertion too.
        let mut arrays = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            compound.insert("ints", NbtTag::IntArray(vec![1, 2]));
            compound
        });
        let ints = modify_path("ints");
        assert!(
            insert_data_tags(&ints, &mut arrays, -1, &[NbtTag::Int(3)]).is_ok(),
            "append should accept an int array"
        );
        assert_eq!(
            root_compound(arrays).get("ints"),
            Some(&NbtTag::IntArray(vec![1, 2, 3]))
        );

        // A non-collection target and an out-of-range index are errors.
        let mut scalar = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            compound.insert("value", 1);
            compound
        });
        let scalar_path = modify_path("value");
        let error = insert_data_tags(&scalar_path, &mut scalar, 0, &[NbtTag::Int(9)])
            .expect_err("vanilla rejects a non-list insert target");
        assert_eq!(
            message_key(&error.message_component()),
            translations::COMMANDS_DATA_MODIFY_EXPECTED_LIST.0,
            "non-list insert should use `commands.data.modify.expected_list`"
        );

        let mut root = list_root();
        let error = insert_data_tags(&path, &mut root, 9, &[NbtTag::Int(9)])
            .expect_err("vanilla rejects an out-of-range index");
        assert_eq!(
            message_key(&error.message_component()),
            translations::COMMANDS_DATA_MODIFY_INVALID_INDEX.0,
            "bad insert index should use `commands.data.modify.invalid_index`"
        );
    }

    #[test]
    fn modify_merge_creates_parents_and_requires_compounds() {
        // Missing parents are created and hold the merged compound.
        let mut root = NbtTag::Compound(NbtCompound::new());
        let path = modify_path("a.b");
        let value = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            compound.insert("x", 1);
            compound
        });
        assert_eq!(
            merge_data_tags(&path, &mut root, &[value]).expect("merge should apply"),
            1
        );
        let compound = root_compound(root);
        let inner = compound
            .compound("a")
            .and_then(|a| a.compound("b"))
            .expect("merge creates each missing compound parent");
        assert_eq!(inner.int("x"), Some(1));

        // Existing compounds merge deeply instead of being replaced.
        let mut root = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            let mut nested = NbtCompound::new();
            nested.insert("keep", 1);
            compound.insert("nested", nested);
            compound
        });
        let path = modify_path("nested");
        let mut incoming = NbtCompound::new();
        incoming.insert("added", 2);
        assert_eq!(
            merge_data_tags(&path, &mut root, &[NbtTag::Compound(incoming)])
                .expect("merge should apply"),
            1
        );
        let nested = root_compound(root)
            .compound("nested")
            .cloned()
            .expect("nested stays a compound");
        assert_eq!(nested.int("keep"), Some(1));
        assert_eq!(nested.int("added"), Some(2));

        // Merging the same values again changes nothing.
        let mut same = NbtCompound::new();
        same.insert("added", 2);
        let mut root = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            compound.insert("nested", same);
            compound
        });
        let path = modify_path("nested");
        let mut incoming = NbtCompound::new();
        incoming.insert("added", 2);
        assert_eq!(
            merge_data_tags(&path, &mut root, &[NbtTag::Compound(incoming)])
                .expect("merge should apply"),
            0
        );

        // Vanilla only accepts compounds for `merge`'s values ...
        let mut root = NbtTag::Compound(NbtCompound::new());
        let path = modify_path("value");
        let error = merge_data_tags(&path, &mut root, &[NbtTag::Int(1)])
            .expect_err("merge rejects non-compound values");
        assert_eq!(
            message_key(&error.message_component()),
            translations::COMMANDS_DATA_MODIFY_EXPECTED_OBJECT.0,
            "non-compound merge value should use `commands.data.modify.expected_object`"
        );

        // ... and for every tag the path already selects.
        let mut root = NbtTag::Compound({
            let mut compound = NbtCompound::new();
            compound.insert("value", 1);
            compound
        });
        let error = merge_data_tags(&path, &mut root, &[NbtTag::Compound(NbtCompound::new())])
            .expect_err("merge rejects non-compound targets");
        assert_eq!(
            message_key(&error.message_component()),
            translations::COMMANDS_DATA_MODIFY_EXPECTED_OBJECT.0,
            "non-compound merge target should use `commands.data.modify.expected_object`"
        );
    }

    /// Vanilla `/data modify entity ... set Health` runs `Entity.load`, whose
    /// `LivingEntity.readAdditionalSaveData` link restores health. Steel keeps
    /// that link separate, so `load_with_owned_components` must apply it or the
    /// edit is silently ignored.
    #[test]
    fn entity_set_data_applies_the_living_layer_health() {
        use std::sync::Arc;

        use glam::DVec3;
        use steel_registry::vanilla_entities;

        use crate::bootstrap::init_globals;
        use crate::entity::entities::PigEntity;
        use crate::entity::{SharedEntity, next_entity_id};
        use crate::test_support::fresh_test_world;

        init_globals();
        let fixture = fresh_test_world("data_entity_health");
        let entity: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            next_entity_id(),
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(&fixture.world),
        ));
        let living = entity.as_living_entity().expect("a pig is a living entity");
        assert!(
            (living.get_health() - living.get_max_health()).abs() < f32::EPSILON,
            "a freshly created pig starts at full health"
        );

        let mut data = NbtCompound::new();
        data.insert("Health", 7.5_f32);
        DataAccessor::Entity(Arc::clone(&entity))
            .set_data(data)
            .expect("`/data modify entity ... set Health` should apply");

        assert!(
            (living.get_health() - 7.5).abs() < f32::EPSILON,
            "the living layer must apply the edited Health, got {}",
            living.get_health()
        );
    }
}
