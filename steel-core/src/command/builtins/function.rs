//! Vanilla `/function` command.
//!
//! Mirrors `net.minecraft.server.commands.FunctionCommand`: `function <name>`
//! runs a function or every function in a `#tag`, `function <name> <arguments>`
//! supplies macro arguments, and `function <name> with <source> [<path>]` reads
//! them from a block entity, entity or command storage, exactly like vanilla's
//! shared `DataCommands.SOURCE_PROVIDERS`.
//!
//! **Deviation from vanilla:** vanilla compiles a function with
//! `function-permission-level` and executes it with the hard-coded GAMEMASTER set.
//! Steel parses each line when the function is instantiated, so both the parse and
//! the execution of a function use `function-permission-level`. Only vanilla
//! command levels are capped; Steel's rule-based grants still apply.

use std::sync::Arc;

use simdnbt::owned::NbtTag;
use steel_protocol::packets::game::{
    ArgumentType as ProtocolArgumentType, SuggestionType as ProtocolSuggestionType,
};
use steel_utils::{DowncastType, DowncastTypeKey, Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError, StringReader, SuggestionsBuilder},
    execution::{
        ChainModifiers, CommandArgumentSource, CommandResultCallback, CommandSource,
        CustomCommandExecutor, ExecutionCommandSource, ExecutionControl, SteelArgumentParser,
        SteelArgumentSuggestionContext, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, SteelContextChain, argument, literal,
    },
    registration::CommandRegistration,
};
use super::data::{self, DataTarget};
use super::scoreboard::format_list;
use crate::{
    command::incorrectly_typed_argument,
    datapack::{self, Function, FunctionArguments, InstantiationFailure},
    permission::PermissionLevel,
};

/// Vanilla `FunctionCommand.register`: `LEVEL_GAMEMASTERS`.
pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("function"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    let mut with = literal("with");
    // Vanilla's `SOURCE_PROVIDERS` order is entity, block, storage.
    for target in [DataTarget::Entity, DataTarget::Block, DataTarget::Storage] {
        with = with.then(
            literal(target.provider()).then(
                argument("source", target.argument_type())
                    .executes_custom(RunFunction {
                        arguments: Arguments::Source(target),
                    })
                    .then(
                        argument("path", SteelArgumentType::nbt_path()).executes_custom(
                            RunFunction {
                                arguments: Arguments::SourcePath(target),
                            },
                        ),
                    ),
            ),
        );
    }

    literal("function")
        .then(
            argument("name", function_argument())
                .executes_custom(RunFunction {
                    arguments: Arguments::None,
                })
                .then(
                    argument("arguments", SteelArgumentType::nbt_tag()).executes_custom(
                        RunFunction {
                            arguments: Arguments::Inline,
                        },
                    ),
                ),
        )
        .then(with)
}

/// Vanilla `FunctionCommand.FunctionCustomExecutor.arguments`: where macros come from.
#[derive(Clone, Copy)]
enum Arguments {
    /// `function <name>`: a plain function takes no arguments.
    None,
    /// `function <name> <arguments>`.
    Inline,
    /// `function <name> with <source>`.
    Source(DataTarget),
    /// `function <name> with <source> <path>`.
    SourcePath(DataTarget),
}

/// Vanilla `FunctionCommand.FunctionCustomExecutor`: runs the resolved functions.
struct RunFunction {
    arguments: Arguments,
}

impl CustomCommandExecutor<CommandSource> for RunFunction {
    fn run(
        &self,
        source: Arc<CommandSource>,
        chain: &SteelContextChain<CommandSource>,
        modifiers: ChainModifiers,
        control: &mut ExecutionControl<'_, CommandSource>,
    ) {
        if let Err(error) = self.queue(&source, chain, control) {
            // Vanilla `CustomCommandExecutor.WithErrorHandling.runGuarded`.
            source.handle_error(&error, modifiers.is_forked());
        }
    }
}

impl RunFunction {
    fn queue(
        &self,
        source: &Arc<CommandSource>,
        chain: &SteelContextChain<CommandSource>,
        control: &mut ExecutionControl<'_, CommandSource>,
    ) -> Result<(), CommandSyntaxError> {
        let context = chain.top_context();
        let target = function_value(context)?;
        let (id, functions) = resolve_functions(target)?;
        if functions.is_empty() {
            return Err(no_functions(&id));
        }
        let arguments = self.arguments(context)?;
        announce(source, &functions);

        // Vanilla `FunctionCommand.modifySenderForExecution`, capped at the
        // configured function permission level instead of the hard-coded GAMEMASTER.
        let function_source = Arc::new(
            source
                .as_ref()
                .with_maximum_permission(source.server().function_permission_set().level())
                .with_suppressed_output()
                .with_callback(CommandResultCallback::empty()),
        );

        for function in functions {
            let instantiated = {
                let dispatcher = source.server().command_dispatcher();
                function.instantiate(arguments.as_ref(), &dispatcher, &function_source)
            };
            let chains =
                instantiated.map_err(|failure| instantiation_failure(&function.id, &failure))?;
            // Vanilla `CallFunction` runs the body with a cleared callback and
            // reports the function's `/return` value through its own frame.
            control.queue_function_call(
                chains,
                Arc::clone(&function_source),
                result_collector(source, &function.id),
            );
        }
        Ok(())
    }

    /// Vanilla `FunctionCustomExecutor.arguments`.
    fn arguments(
        &self,
        context: &SteelCommandContext<CommandSource>,
    ) -> Result<Option<FunctionArguments>, CommandSyntaxError> {
        match self.arguments {
            Arguments::None => Ok(None),
            Arguments::Inline => Ok(Some(compound_arguments(context.nbt_tag("arguments")?)?)),
            Arguments::Source(target) => {
                let accessor = data::resolve(context, target)?;
                Ok(Some(FunctionArguments::from_compound(&accessor.data())))
            }
            Arguments::SourcePath(target) => {
                let accessor = data::resolve(context, target)?;
                let path = context.nbt_path("path")?;
                let data = NbtTag::Compound(accessor.data());
                let Some(tag) = data::single_tag(path, &data)? else {
                    return Err(data::unknown_path(path));
                };
                Ok(Some(compound_arguments(&tag)?))
            }
        }
    }
}

/// Vanilla `FunctionCommand.getArgumentTag`: only a compound can carry macros.
fn compound_arguments(tag: &NbtTag) -> Result<FunctionArguments, CommandSyntaxError> {
    let NbtTag::Compound(compound) = tag else {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_FUNCTION_ERROR_ARGUMENT_NOT_COMPOUND
                .message([TextComponent::plain(nbt_type_name(tag))])
                .component(),
        ));
    };
    Ok(FunctionArguments::from_compound(compound))
}

/// Vanilla `TagType.getName`.
const fn nbt_type_name(tag: &NbtTag) -> &'static str {
    match tag {
        NbtTag::Byte(_) => "BYTE",
        NbtTag::Short(_) => "SHORT",
        NbtTag::Int(_) => "INT",
        NbtTag::Long(_) => "LONG",
        NbtTag::Float(_) => "FLOAT",
        NbtTag::Double(_) => "DOUBLE",
        NbtTag::ByteArray(_) => "BYTE[]",
        NbtTag::String(_) => "STRING",
        NbtTag::List(_) => "LIST",
        NbtTag::Compound(_) => "COMPOUND",
        NbtTag::IntArray(_) => "INT[]",
        NbtTag::LongArray(_) => "LONG[]",
    }
}

/// Vanilla `FunctionArgument.getFunctionCollection`.
fn resolve_functions(
    target: FunctionArgumentValue,
) -> Result<(Identifier, Vec<Arc<Function>>), CommandSyntaxError> {
    let Some(manager) = datapack::try_datapacks() else {
        return Ok((target.id, Vec::new()));
    };
    let registry = manager.functions().read();
    if target.tag {
        let functions = registry.tagged_shared(&target.id);
        Ok((target.id, functions))
    } else if let Some(function) = registry.get_shared(&target.id) {
        Ok((target.id, vec![function]))
    } else {
        Err(unknown_function(&target.id))
    }
}

/// Vanilla `FunctionCommand`'s two feedback messages before the calls are queued.
fn announce(source: &CommandSource, functions: &[Arc<Function>]) {
    if let [function] = functions {
        source.send_success(
            &translations::COMMANDS_FUNCTION_SCHEDULED_SINGLE
                .message([plain_id(&function.id)])
                .component(),
            true,
        );
        return;
    }
    let ids = format_list(functions.iter().map(|function| plain_id(&function.id)));
    source.send_success(
        &translations::COMMANDS_FUNCTION_SCHEDULED_MULTIPLE
            .message([ids])
            .component(),
        true,
    );
}

/// Vanilla `decorateOutputIfNeeded`: reports a function's `/return` value.
fn result_collector(source: &Arc<CommandSource>, id: &Identifier) -> CommandResultCallback {
    if source.is_silent() {
        return source.callback();
    }
    let original = source.callback();
    let source = Arc::clone(source);
    let id = id.clone();
    CommandResultCallback::new(move |success, result| {
        source.send_success(
            &translations::COMMANDS_FUNCTION_RESULT
                .message([plain_id(&id), TextComponent::plain(result.to_string())])
                .component(),
            true,
        );
        original.on_result(success, result);
    })
}

/// Vanilla `ERROR_FUNCTION_INSTANTATION_FAILURE`.
fn instantiation_failure(id: &Identifier, failure: &InstantiationFailure) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_FUNCTION_INSTANTIATION_FAILURE
            .message([plain_id(id), failure.reason.clone()])
            .component(),
    )
}

fn no_functions(id: &Identifier) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_FUNCTION_SCHEDULED_NO_FUNCTIONS
            .message([plain_id(id)])
            .component(),
    )
}

fn unknown_function(id: &Identifier) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::ARGUMENTS_FUNCTION_UNKNOWN
            .message([plain_id(id)])
            .component(),
    )
}

/// Vanilla `Component.translationArg(Identifier)`: a plain literal of the id.
fn plain_id(id: &Identifier) -> TextComponent {
    TextComponent::plain(id.to_string())
}

/// Vanilla `FunctionArgument.Result`: one function id or one `#tag`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FunctionArgumentValue {
    /// Whether the value named a tag (`#id`).
    pub(super) tag: bool,
    /// The function or tag id, without the `#` prefix.
    pub(super) id: Identifier,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for FunctionArgumentValue {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/value/function");
}

pub(super) fn function_value(
    context: &SteelCommandContext<CommandSource>,
) -> Result<FunctionArgumentValue, CommandSyntaxError> {
    function_argument_value(context, "name")
}

/// Vanilla `FunctionArgument.getFunctionOrTag`: the parsed `function` argument.
pub(super) fn function_argument_value(
    context: &SteelCommandContext<CommandSource>,
    name: &str,
) -> Result<FunctionArgumentValue, CommandSyntaxError> {
    context
        .argument(name)?
        .downcast_ref::<FunctionArgumentValue>()
        .cloned()
        .ok_or_else(|| incorrectly_typed_argument(name))
}

/// Vanilla `FunctionArgument`: `foo`, `ns:foo` or `#tag`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FunctionArgumentParser;

// SAFETY: This Steel-owned key uniquely identifies the concrete type in the process.
unsafe impl DowncastType for FunctionArgumentParser {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:command/parser/function");
}

impl SteelArgumentParser for FunctionArgumentParser {
    type Value = FunctionArgumentValue;

    fn parse(
        &self,
        reader: &mut StringReader<'_>,
        _source: &dyn CommandArgumentSource,
    ) -> Result<Self::Value, CommandSyntaxError> {
        let tag = reader.peek() == Some('#');
        if tag {
            reader.skip();
        }
        let raw = reader.read_identifier().to_owned();
        let id: Identifier = raw
            .parse()
            .map_err(|_| CommandSyntaxError::dynamic(format!("Unknown function '{raw}'")))?;
        Ok(FunctionArgumentValue { tag, id })
    }

    fn list_suggestions(
        &self,
        _context: &dyn SteelArgumentSuggestionContext,
        builder: &mut SuggestionsBuilder<'_>,
    ) {
        suggest_functions(builder);
    }

    fn protocol_argument(&self) -> (ProtocolArgumentType, Option<ProtocolSuggestionType>) {
        (
            ProtocolArgumentType::Function,
            Some(ProtocolSuggestionType::AskServer),
        )
    }
}

/// The `minecraft:function` argument type shared with `/schedule`.
pub(super) fn function_argument() -> SteelArgumentType {
    SteelArgumentType::new(FunctionArgumentParser)
}

/// Vanilla `FunctionCommand.SUGGEST_FUNCTION`: tags first, then function names.
fn suggest_functions(builder: &mut SuggestionsBuilder<'_>) {
    let Some(manager) = datapack::try_datapacks() else {
        return;
    };
    let registry = manager.functions().read();
    let prefix = builder.remaining().to_owned();
    for tag in registry.tag_ids() {
        let value = format!("#{tag}");
        if value.starts_with(&prefix) {
            builder.suggest(value);
        }
    }
    for id in registry.ids() {
        let value = id.to_string();
        if value.starts_with(&prefix) {
            builder.suggest(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::nbt_type_name;
    use simdnbt::owned::{NbtCompound, NbtTag};

    #[test]
    fn nbt_type_names_match_vanilla_tag_names() {
        assert_eq!(nbt_type_name(&NbtTag::Int(1)), "INT");
        assert_eq!(nbt_type_name(&NbtTag::String("x".into())), "STRING");
        assert_eq!(
            nbt_type_name(&NbtTag::Compound(NbtCompound::new())),
            "COMPOUND"
        );
        assert_eq!(nbt_type_name(&NbtTag::LongArray(vec![1])), "LONG[]");
    }
}
