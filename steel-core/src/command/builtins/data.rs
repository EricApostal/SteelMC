//! Vanilla `/data` command.
//!
//! Mirrors `DataCommands` for the `get`, `merge` and `remove` verbs across
//! vanilla's three `DataAccessor` providers: `entity`, `block` and `storage`.
//!
//! `modify` is not wired yet: it needs vanilla's `NbtTagArgument` values plus
//! the loot `ResourceOrIdArgument` number providers for `compute`, and Steel's
//! command runtime has neither.

use std::sync::Arc;

use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::{
    BlockPos, Identifier,
    nbt::{NbtNumeric as _, NbtPath, merge_nbt_compounds, nbt_compounds_equal},
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
            .then(remove_target(target));
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
                argument("nbt", SteelArgumentType::nbt_tag())
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

/// Resolves the provider's `DataAccessor` the way vanilla's `access(context)` does.
///
/// `/function ... with` reuses this, exactly like vanilla's shared `SOURCE_PROVIDERS`.
pub(super) fn resolve<'a>(
    context: &'a SteelCommandContext<CommandSource>,
    target: DataTarget,
) -> Result<DataAccessor<'a>, CommandSyntaxError> {
    match target {
        DataTarget::Entity => Ok(DataAccessor::Entity(context.entity("target")?)),
        DataTarget::Block => {
            let position = loaded_block_position(context, "target")?;
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
            let id = context.identifier("target")?.clone();
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
    let nbt = context.nbt_tag("nbt")?;
    // Vanilla's `CompoundTagArgument` parses only compounds, so anything else is
    // rejected with `argument.nbt.expected.compound`.
    let NbtTag::Compound(nbt) = nbt else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::ARGUMENT_NBT_EXPECTED_COMPOUND,
        )));
    };
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
fn list_len(list: &NbtList) -> usize {
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
            panic!("child {name} should exist");
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
                &SteelArgumentType::nbt_tag(),
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
        }
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
}
