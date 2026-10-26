//! `mcfunction` data packs, mirroring vanilla
//! `net.minecraft.commands.functions.{CommandFunction,StringTemplate,FunctionBuilder}`
//! and the server-side function library that reloads them.
//!
//! **Deviation from vanilla:** vanilla parses every function line into
//! `UnboundEntryAction`s during the *prepare* phase of a reload. Steel's command
//! dispatcher lives behind `Server::command_dispatcher()` and is only reachable on
//! the server thread, so Steel stores the validated line list and parses each line
//! when the function is *instantiated* (vanilla's `instantiate` step). The
//! observable behaviour — including syntax errors surfacing at run time rather
//! than load time — is otherwise the same, and the prepare phase stays off-thread.

use std::{any::Any, sync::Arc};

use rustc_hash::FxHashMap;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::{Identifier, locks::SyncRwLock, translations};
use text_components::TextComponent;

use crate::command::{
    CommandDispatcher,
    execution::{CommandExecutionContext, CommandSource, ExecutionStop, SteelContextChain},
};

use super::{
    pack::{PackError, PackType},
    resource::{MultiPackResourceManager, PreparableReloadListener},
};

/// Vanilla `CommandFunction.checkCommandLineLength` cap.
const MAX_COMMAND_LINE_LENGTH: usize = 2_000_000;

/// Vanilla `StringTemplate`: a macro line split into literal segments and variables.
///
/// `segments.len()` is `variables.len()` or `variables.len() + 1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StringTemplate {
    /// Literal text between `$(...)` substitutions.
    pub(crate) segments: Vec<String>,
    /// Macro variable names, in order of appearance.
    pub(crate) variables: Vec<String>,
}

impl StringTemplate {
    /// Vanilla `StringTemplate.fromString`.
    ///
    /// # Errors
    /// Returns a message when a `$(` is unterminated, a variable name is invalid,
    /// or the line contains no variables at all.
    pub(crate) fn from_string(input: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut variables = Vec::new();
        let bytes = input.as_bytes();
        let mut start = 0usize;
        let mut index = input.find('$');

        while let Some(found) = index {
            if found + 1 < bytes.len() && bytes[found + 1] == b'(' {
                segments.push(input[start..found].to_owned());
                let Some(close) = input[found + 1..].find(')') else {
                    return Err("Unterminated macro variable".to_owned());
                };
                let close = found + 1 + close;
                let variable = &input[found + 2..close];
                if !is_valid_variable_name(variable) {
                    return Err(format!("Invalid macro variable name '{variable}'"));
                }
                variables.push(variable.to_owned());
                start = close + 1;
                index = input[start..].find('$').map(|offset| start + offset);
            } else {
                index = input[found + 1..]
                    .find('$')
                    .map(|offset| found + 1 + offset);
            }
        }

        if start == 0 {
            return Err("No variables in macro".to_owned());
        }
        if start != input.len() {
            segments.push(input[start..].to_owned());
        }
        Ok(Self {
            segments,
            variables,
        })
    }

    /// Vanilla `StringTemplate.substitute`.
    ///
    /// # Errors
    /// Returns the offending variable name when `arguments` is missing an entry.
    pub(crate) fn substitute(&self, arguments: &[String]) -> Result<String, String> {
        if arguments.len() != self.variables.len() {
            return Err(format!(
                "Macro requires {} argument(s) but {} were supplied",
                self.variables.len(),
                arguments.len()
            ));
        }
        let mut builder = String::with_capacity(self.segments.iter().map(String::len).sum());
        for (index, variable) in self.variables.iter().enumerate() {
            builder.push_str(&self.segments[index]);
            builder.push_str(&arguments[index]);
            let _ = variable;
        }
        if self.segments.len() > self.variables.len() {
            builder.push_str(&self.segments[self.variables.len()]);
        }
        if builder.len() > MAX_COMMAND_LINE_LENGTH {
            return Err("Macro expansion exceeded the command line length limit".to_owned());
        }
        Ok(builder)
    }
}

/// Vanilla `StringTemplate.isValidVariableName`.
pub(crate) fn is_valid_variable_name(variable: &str) -> bool {
    !variable.is_empty()
        && variable
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// One validated line of an `mcfunction` file.
#[derive(Debug, Clone)]
pub(crate) enum FunctionEntry {
    /// A plain command line, already trimmed and validated.
    Plain(String),
    /// A `$` line whose `$(...)` substitutions happen at instantiation.
    Macro(StringTemplate),
}

/// Vanilla `CommandFunction`: one `data/<ns>/function/**.mcfunction` file.
#[derive(Debug)]
pub(crate) struct Function {
    /// Function id, e.g. `minecraft:always_pass`.
    pub(crate) id: Identifier,
    /// Validated entries in file order.
    pub(crate) entries: Vec<FunctionEntry>,
    /// Vanilla `MacroFunction.getArgumentNames`: distinct macro names in order.
    pub(crate) macro_names: Vec<String>,
}

impl Function {
    /// Vanilla `FunctionBuilder.build`: validates lines and collects macro names.
    ///
    /// # Errors
    /// Returns a message describing the first malformed line.
    pub(crate) fn from_lines(id: Identifier, lines: &[String]) -> Result<Self, String> {
        let mut entries = Vec::with_capacity(lines.len());
        let mut macro_names: Vec<String> = Vec::new();

        let mut index = 0usize;
        while index < lines.len() {
            let line_number = index + 1;
            let trimmed = lines[index].trim();
            let joined = if trimmed.ends_with('\\') {
                let mut builder = trimmed.to_owned();
                loop {
                    index += 1;
                    if index == lines.len() {
                        return Err("Line continuation at end of file".to_owned());
                    }
                    builder.pop();
                    builder.push_str(lines[index].trim());
                    if builder.len() > MAX_COMMAND_LINE_LENGTH {
                        return Err("Command line length limit exceeded".to_owned());
                    }
                    if !builder.ends_with('\\') {
                        break;
                    }
                }
                builder
            } else {
                trimmed.to_owned()
            };
            index += 1;

            if joined.is_empty() || joined.starts_with('#') {
                continue;
            }
            if let Some(rest) = joined.strip_prefix('/') {
                let name = rest.split_whitespace().next().unwrap_or_default();
                let hint = if rest.starts_with('/') {
                    "(if you intended to make a comment, use '#' not '//')".to_owned()
                } else {
                    format!("(did you mean '{name}'? Do not use a preceding forwards slash.)")
                };
                return Err(format!(
                    "Unknown or invalid command '{joined}' on line {line_number} {hint}"
                ));
            }
            if let Some(line) = joined.strip_prefix('$') {
                let template = StringTemplate::from_string(line).map_err(|error| {
                    format!("Can't parse function line {line_number}: '{line}': {error}")
                })?;
                for variable in &template.variables {
                    if !macro_names.contains(variable) {
                        macro_names.push(variable.clone());
                    }
                }
                entries.push(FunctionEntry::Macro(template));
            } else {
                if joined.len() > MAX_COMMAND_LINE_LENGTH {
                    return Err("Command line length limit exceeded".to_owned());
                }
                entries.push(FunctionEntry::Plain(joined));
            }
        }

        Ok(Self {
            id,
            entries,
            macro_names,
        })
    }

    /// Vanilla `CommandFunction.instantiate`: materializes the runnable lines.
    ///
    /// Steel's documented deviation is that a function's lines are parsed here
    /// rather than during the reload prepare phase, so a syntax error surfaces as
    /// an instantiation failure instead of a load-time skip.
    ///
    /// # Errors
    /// Returns the vanilla instantiation reason when a macro argument is missing or
    /// a line cannot be parsed.
    #[expect(
        clippy::result_large_err,
        reason = "the fallible callback signature mirrors vanilla FunctionInstantiationException"
    )]
    pub(crate) fn instantiate(
        &self,
        arguments: Option<&FunctionArguments>,
        dispatcher: &CommandDispatcher,
        source: &CommandSource,
    ) -> Result<Vec<SteelContextChain<CommandSource>>, InstantiationFailure> {
        let mut chains = Vec::with_capacity(self.entries.len());
        for command in self.expand_lines(arguments)? {
            let parse = dispatcher.parse(&command, source.clone());
            let chain = dispatcher.context_chain(parse).map_err(|error| {
                InstantiationFailure::new(
                    self.id.clone(),
                    translations::COMMANDS_FUNCTION_ERROR_PARSE
                        .message([
                            TextComponent::plain(self.id.to_string()),
                            TextComponent::plain(command.clone()),
                            error.message_component(),
                        ])
                        .component(),
                )
            })?;
            chains.push(chain);
        }
        Ok(chains)
    }

    /// Vanilla `MacroFunction.instantiate`'s textual half: the command lines.
    ///
    /// # Errors
    /// Returns the vanilla instantiation reason when a macro argument is missing.
    #[expect(
        clippy::result_large_err,
        reason = "the fallible callback signature mirrors vanilla FunctionInstantiationException"
    )]
    pub(crate) fn expand_lines(
        &self,
        arguments: Option<&FunctionArguments>,
    ) -> Result<Vec<String>, InstantiationFailure> {
        if !self.macro_names.is_empty() && arguments.is_none() {
            return Err(self.missing_arguments_failure());
        }

        let mut commands = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            match entry {
                FunctionEntry::Plain(line) => commands.push(line.clone()),
                FunctionEntry::Macro(template) => {
                    let arguments = arguments.expect("macro entries always require arguments");
                    let values = template
                        .variables
                        .iter()
                        .map(|name| {
                            arguments
                                .get(name)
                                .cloned()
                                .ok_or_else(|| self.missing_argument_failure(name))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    commands.push(
                        template
                            .substitute(&values)
                            .map_err(|reason| self.substitute_failure(&values, reason))?,
                    );
                }
            }
        }
        Ok(commands)
    }

    /// Vanilla `commands.function.error.missing_arguments`.
    fn missing_arguments_failure(&self) -> InstantiationFailure {
        InstantiationFailure::new(
            self.id.clone(),
            translations::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENTS
                .message([TextComponent::plain(self.id.to_string())])
                .component(),
        )
    }

    /// Vanilla `commands.function.error.missing_argument`.
    fn missing_argument_failure(&self, name: &str) -> InstantiationFailure {
        InstantiationFailure::new(
            self.id.clone(),
            translations::COMMANDS_FUNCTION_ERROR_MISSING_ARGUMENT
                .message([
                    TextComponent::plain(self.id.to_string()),
                    TextComponent::plain(name.to_owned()),
                ])
                .component(),
        )
    }

    /// A macro line whose substitution failed (vanilla reports it per line).
    fn substitute_failure(&self, values: &[String], reason: String) -> InstantiationFailure {
        InstantiationFailure::new(
            self.id.clone(),
            translations::COMMANDS_FUNCTION_ERROR_PARSE
                .message([
                    TextComponent::plain(self.id.to_string()),
                    TextComponent::plain(values.join(", ")),
                    TextComponent::plain(reason),
                ])
                .component(),
        )
    }
}

/// Vanilla `FunctionInstantiationException`: why a function could not be built.
#[derive(Debug, Clone)]
pub(crate) struct InstantiationFailure {
    /// The function being instantiated.
    pub(crate) id: Identifier,
    /// Vanilla's reason component, wrapped by `/function` in
    /// `commands.function.instantiationFailure`.
    pub(crate) reason: TextComponent,
}

impl InstantiationFailure {
    /// Builds one failure for `id`.
    #[must_use]
    pub(crate) const fn new(id: Identifier, reason: TextComponent) -> Self {
        Self { id, reason }
    }
}

/// Vanilla `MacroFunction` arguments: the `CompoundTag` supplied to `/function`.
#[derive(Debug, Default, Clone)]
pub(crate) struct FunctionArguments {
    values: FxHashMap<String, String>,
}

impl FunctionArguments {
    /// Builds arguments from a compound, mirroring vanilla's `CompoundTag` values.
    ///
    /// Vanilla accepts any NBT tag and stringifies it with `Tag.toString` for the
    /// textual substitution; Steel uses the SNBT form for the same effect.
    #[must_use]
    pub(crate) fn from_compound(compound: &NbtCompound) -> Self {
        let mut values = FxHashMap::default();
        for (name, tag) in compound.iter() {
            values.insert(name.to_string(), tag_to_substitution(tag));
        }
        Self { values }
    }

    /// Vanilla `CompoundTag.get`: one macro argument's textual substitution.
    #[must_use]
    pub(crate) fn get(&self, name: &str) -> Option<&String> {
        self.values.get(name)
    }

    /// Vanilla `MacroFunction.instantiate` argument check: every macro name must be present.
    #[must_use]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "vanilla MacroFunction argument API surface")
    )]
    pub(crate) fn has_all_of(&self, names: &[String]) -> bool {
        names.iter().all(|name| self.values.contains_key(name))
    }

    /// The supplied argument names.
    #[must_use]
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "vanilla MacroFunction argument API surface")
    )]
    pub(crate) fn names(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }
}

/// Renders one NBT value the way vanilla substitutes macros.
///
/// Vanilla stringifies every argument tag with `Tag.toString`: a bare string is
/// inserted as-is, while every other tag renders in SNBT form.
fn tag_to_substitution(tag: &NbtTag) -> String {
    if let NbtTag::String(value) = tag {
        return value.to_str().into_owned();
    }
    let mut out = String::new();
    write_snbt(tag, &mut out);
    out
}

/// Appends `tag` in SNBT form, mirroring vanilla's `StringTagVisitor`.
fn write_snbt(tag: &NbtTag, out: &mut String) {
    match tag {
        NbtTag::Byte(value) => out.push_str(&value.to_string()),
        NbtTag::Short(value) => out.push_str(&value.to_string()),
        NbtTag::Int(value) => out.push_str(&value.to_string()),
        NbtTag::Long(value) => {
            out.push_str(&value.to_string());
            out.push('L');
        }
        NbtTag::Float(value) => {
            out.push_str(&value.to_string());
            out.push('f');
        }
        NbtTag::Double(value) => {
            out.push_str(&value.to_string());
            out.push('d');
        }
        NbtTag::String(value) => write_quoted(value.to_str().as_ref(), out),
        NbtTag::ByteArray(values) => {
            out.push_str("[B;");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&value.to_string());
            }
            out.push(']');
        }
        NbtTag::IntArray(values) => {
            out.push_str("[I;");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&value.to_string());
            }
            out.push(']');
        }
        NbtTag::LongArray(values) => {
            out.push_str("[L;");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&value.to_string());
            }
            out.push(']');
        }
        NbtTag::List(list) => write_snbt_list(list, out),
        NbtTag::Compound(compound) => write_snbt_compound(compound, out),
    }
}

/// Appends one NBT compound in SNBT form.
fn write_snbt_compound(compound: &NbtCompound, out: &mut String) {
    out.push('{');
    for (index, (name, value)) in compound.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        write_quoted(name.to_str().as_ref(), out);
        out.push(':');
        write_snbt(value, out);
    }
    out.push('}');
}

/// Appends one NBT list in SNBT form.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per NbtList variant; splitting would scatter the SNBT grammar"
)]
fn write_snbt_list(list: &NbtList, out: &mut String) {
    out.push('[');
    let mut first = true;
    let separator = |out: &mut String, first: &mut bool| {
        if *first {
            *first = false;
        } else {
            out.push(',');
        }
    };
    match list {
        NbtList::Empty => {}
        NbtList::Byte(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str(&value.to_string());
            }
        }
        NbtList::Short(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str(&value.to_string());
            }
        }
        NbtList::Int(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str(&value.to_string());
            }
        }
        NbtList::Long(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str(&value.to_string());
                out.push('L');
            }
        }
        NbtList::Float(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str(&value.to_string());
                out.push('f');
            }
        }
        NbtList::Double(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str(&value.to_string());
                out.push('d');
            }
        }
        NbtList::ByteArray(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str("[B;");
                for (index, byte) in value.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&byte.to_string());
                }
                out.push(']');
            }
        }
        NbtList::String(values) => {
            for value in values {
                separator(out, &mut first);
                write_quoted(value.to_str().as_ref(), out);
            }
        }
        NbtList::List(values) => {
            for value in values {
                separator(out, &mut first);
                write_snbt_list(value, out);
            }
        }
        NbtList::Compound(values) => {
            for value in values {
                separator(out, &mut first);
                write_snbt_compound(value, out);
            }
        }
        NbtList::IntArray(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str("[I;");
                for (index, int) in value.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&int.to_string());
                }
                out.push(']');
            }
        }
        NbtList::LongArray(values) => {
            for value in values {
                separator(out, &mut first);
                out.push_str("[L;");
                for (index, long) in value.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&long.to_string());
                }
                out.push(']');
            }
        }
    }
    out.push(']');
}

/// Appends `value` as a quoted, escaped SNBT string.
fn write_quoted(value: &str, out: &mut String) {
    out.push('"');
    for char in value.chars() {
        match char {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            _ => out.push(char),
        }
    }
    out.push('"');
}

/// Vanilla server-side function library: id -> function, plus `#tag` lookups.
#[derive(Debug, Default)]
pub(crate) struct FunctionRegistry {
    functions: FxHashMap<Identifier, Arc<Function>>,
    tags: FxHashMap<Identifier, Vec<Identifier>>,
}

impl FunctionRegistry {
    /// Vanilla `ServerFunctionManager.get`: looks up a function by id.
    #[must_use]
    pub(crate) fn get(&self, id: &Identifier) -> Option<&Function> {
        self.functions.get(id).map(Arc::as_ref)
    }

    /// Vanilla `ServerFunctionManager.get` for callers that keep the function
    /// alive across other function runs without holding the registry lock.
    #[must_use]
    pub(crate) fn get_shared(&self, id: &Identifier) -> Option<Arc<Function>> {
        self.functions.get(id).cloned()
    }

    /// Vanilla `/function #tag`: every function the tag names, in tag order.
    #[must_use]
    pub(crate) fn tagged_shared(&self, tag: &Identifier) -> Vec<Arc<Function>> {
        self.tags.get(tag).map_or_default(|members| {
            members
                .iter()
                .filter_map(|id| self.functions.get(id).cloned())
                .collect()
        })
    }

    /// Vanilla `/function #tag`: every function the tag names, in tag order.
    #[must_use]
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "vanilla ServerFunctionManager tagged-function API surface"
        )
    )]
    pub(crate) fn tagged(&self, tag: &Identifier) -> Vec<&Function> {
        self.tags.get(tag).map_or_default(|members| {
            members
                .iter()
                .filter_map(|id| self.functions.get(id).map(Arc::as_ref))
                .collect()
        })
    }

    /// Every loaded function id, sorted for stable command suggestions.
    #[must_use]
    pub(crate) fn ids(&self) -> Vec<Identifier> {
        let mut ids: Vec<Identifier> = self.functions.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// The number of loaded functions.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.functions.len()
    }

    /// Whether no functions are loaded.
    #[must_use]
    #[expect(
        dead_code,
        reason = "vanilla ServerFunctionManager function-map API surface"
    )]
    pub(crate) fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Every loaded function tag id, sorted.
    #[must_use]
    pub(crate) fn tag_ids(&self) -> Vec<Identifier> {
        let mut ids: Vec<Identifier> = self.tags.keys().cloned().collect();
        ids.sort();
        ids
    }
}

/// Vanilla `FunctionLibrary`: the reload listener that rebuilds [`FunctionRegistry`].
#[derive(Debug)]
pub(crate) struct FunctionLoader {
    registry: Arc<SyncRwLock<FunctionRegistry>>,
    report: Arc<SyncRwLock<Vec<String>>>,
}

impl FunctionLoader {
    /// Creates a loader installing into `registry`.
    pub(crate) fn new(registry: Arc<SyncRwLock<FunctionRegistry>>) -> Self {
        Self {
            registry,
            report: Arc::new(SyncRwLock::new(Vec::new())),
        }
    }

    /// Vanilla `FunctionLibrary.getTag`: the tag definitions from the last prepare.
    #[must_use]
    #[expect(dead_code, reason = "vanilla FunctionLibrary load-report API surface")]
    pub(crate) fn problems(&self) -> Vec<String> {
        self.report.read().clone()
    }
}

impl PreparableReloadListener for FunctionLoader {
    fn name(&self) -> &'static str {
        "functions"
    }

    fn prepare(
        &self,
        manager: &MultiPackResourceManager,
    ) -> Result<Box<dyn Any + Send>, PackError> {
        let mut functions: FxHashMap<Identifier, Arc<Function>> = FxHashMap::default();
        let mut problems = Vec::new();

        for (key, stack) in manager.list_resources(PackType::Server, "function") {
            let location: Identifier =
                key.parse()
                    .map_err(|error: &'static str| PackError::Decode {
                        path: key.clone(),
                        message: error.to_string(),
                    })?;
            let Some(resource) = stack.first() else {
                continue;
            };
            let text =
                String::from_utf8(resource.bytes.clone()).map_err(|error| PackError::Decode {
                    path: key.clone(),
                    message: format!("mcfunction is not valid UTF-8: {error}"),
                })?;
            let lines: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
            match Function::from_lines(location.clone(), &lines) {
                Ok(function) => {
                    functions.insert(location, Arc::new(function));
                }
                Err(message) => {
                    // Vanilla logs and skips a malformed function rather than
                    // failing the whole reload.
                    tracing::warn!(function = %location, %message, "skipping malformed mcfunction");
                    problems.push(format!("{location}: {message}"));
                }
            }
        }

        let tags = read_function_tags(manager, &mut problems)?;
        Ok(Box::new((functions, tags, problems)))
    }

    fn apply(&self, prepared: Box<dyn Any + Send>) {
        let Ok(prepared) = prepared.downcast::<(
            FxHashMap<Identifier, Arc<Function>>,
            FxHashMap<Identifier, Vec<Identifier>>,
            Vec<String>,
        )>() else {
            return;
        };
        let (functions, tags, problems) = *prepared;
        let mut registry = self.registry.write();
        registry.functions = functions;
        registry.tags = tags;
        drop(registry);
        *self.report.write() = problems;
    }
}

/// Reads `data/<ns>/tags/function/**` into resolved tag members.
fn read_function_tags(
    manager: &MultiPackResourceManager,
    problems: &mut Vec<String>,
) -> Result<FxHashMap<Identifier, Vec<Identifier>>, PackError> {
    let mut raw: FxHashMap<Identifier, Vec<String>> = FxHashMap::default();
    for (key, stack) in manager.list_resources(PackType::Server, "tags/function") {
        let Some(resource) = stack.first() else {
            continue;
        };
        let json: serde_json::Value =
            serde_json::from_slice(&resource.bytes).map_err(|error| PackError::Decode {
                path: key.clone(),
                message: format!("invalid function tag: {error}"),
            })?;
        let Some(values) = json.get("values").and_then(serde_json::Value::as_array) else {
            problems.push(format!("{key}: function tag has no `values` array"));
            continue;
        };
        let tag: Identifier = key
            .parse()
            .map_err(|error: &'static str| PackError::Decode {
                path: key.clone(),
                message: error.to_string(),
            })?;
        raw.insert(
            tag,
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect(),
        );
    }

    // Vanilla `TagLoader` resolves `#tag` references recursively; function tags
    // may nest other function tags.
    let mut resolved: FxHashMap<Identifier, Vec<Identifier>> = FxHashMap::default();
    let keys: Vec<Identifier> = raw.keys().cloned().collect();
    for tag in keys {
        let mut members = Vec::new();
        let mut visiting = Vec::new();
        resolve_function_tag(&tag, &raw, &mut members, &mut visiting, problems);
        resolved.insert(tag, members);
    }
    Ok(resolved)
}

/// Depth-first `#tag` resolution for function tags, de-duplicating members.
fn resolve_function_tag(
    tag: &Identifier,
    raw: &FxHashMap<Identifier, Vec<String>>,
    members: &mut Vec<Identifier>,
    visiting: &mut Vec<Identifier>,
    problems: &mut Vec<String>,
) {
    if visiting.contains(tag) {
        problems.push(format!("{tag}: function tag reference cycle"));
        return;
    }
    let Some(entries) = raw.get(tag) else {
        return;
    };
    visiting.push(tag.clone());
    for entry in entries {
        if let Some(reference) = entry.strip_prefix('#') {
            let Ok(reference) = reference.parse::<Identifier>() else {
                problems.push(format!("{tag}: invalid nested tag '{reference}'"));
                continue;
            };
            resolve_function_tag(&reference, raw, members, visiting, problems);
        } else {
            let Ok(location) = entry.parse::<Identifier>() else {
                problems.push(format!("{tag}: invalid function id '{entry}'"));
                continue;
            };
            if !members.contains(&location) {
                members.push(location);
            }
        }
    }
    visiting.pop();
}

/// Vanilla `ServerFunctionManager.execute`: runs one function to completion.
///
/// Used by the `#load`/`#tick` tags and `/schedule`, which have no enclosing
/// command frame. `/function` instead queues its calls into the running
/// [`CommandExecutionContext`] so nested functions share one command quota.
///
/// A suspension would need the server tick to resume, which vanilla also cannot
/// do for a function run started outside a command; it is logged and abandoned.
///
/// # Errors
/// Returns the instantiation reason when a line cannot be built.
#[expect(
    clippy::result_large_err,
    reason = "the fallible callback signature mirrors vanilla FunctionInstantiationException"
)]
pub(crate) fn run_function(
    source: &Arc<CommandSource>,
    function: &Function,
    arguments: Option<&FunctionArguments>,
) -> Result<(), InstantiationFailure> {
    let chains = {
        let dispatcher = source.server().command_dispatcher();
        function.instantiate(arguments, &dispatcher, source)?
    };

    let mut execution = CommandExecutionContext::for_source(source);
    execution.queue_initial_function_call(chains, Arc::clone(source));
    match execution.run() {
        ExecutionStop::Completed => Ok(()),
        stop => {
            tracing::warn!(
                function = %function.id,
                ?stop,
                "function run did not complete in one tick"
            );
            Ok(())
        }
    }
}

/// Renders a function's macro signature for command errors and suggestions.
#[must_use]
#[expect(dead_code, reason = "vanilla MacroFunction signature API surface")]
pub(crate) fn macro_signature(function: &Function) -> Option<TextComponent> {
    if function.macro_names.is_empty() {
        return None;
    }
    Some(TextComponent::plain(format!(
        "{{{}}}",
        function
            .macro_names
            .iter()
            .map(|name| format!("{name}: …"))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "these tests unwrap their own fixture data so failures point at the offending assertion"
)]
#[expect(
    clippy::assert_is_empty,
    reason = "`assert!(xs.is_empty())` reads better than equality against an empty slice"
)]
mod tests {
    use std::sync::Arc;

    use crate::command::execution::CommandSource;

    use super::{Function, FunctionArguments, StringTemplate, run_function};
    use simdnbt::owned::NbtCompound;
    use steel_utils::Identifier;

    #[test]
    fn template_round_trips_simple_substitution() {
        let template = StringTemplate::from_string("say $(name) hello $(other)").unwrap();
        assert_eq!(template.variables, vec!["name", "other"]);
        let expanded = template
            .substitute(&["world".to_owned(), "again".to_owned()])
            .unwrap();
        assert_eq!(expanded, "say world hello again");
    }

    #[test]
    fn template_rejects_line_without_variables() {
        assert!(StringTemplate::from_string("say hello").is_err());
    }

    #[test]
    fn template_rejects_unterminated_variable() {
        assert!(StringTemplate::from_string("say $(name hello").is_err());
    }

    #[test]
    fn template_rejects_illegal_variable_name() {
        assert!(StringTemplate::from_string("say $(na-me)").is_err());
    }

    #[test]
    fn function_skips_comments_and_blank_lines() {
        let lines = vec!["# a comment".to_owned(), String::new(), "say hi".to_owned()];
        let function = Function::from_lines(Identifier::vanilla_static("test"), &lines).unwrap();
        assert_eq!(function.entries.len(), 1);
        assert!(function.macro_names.is_empty());
    }

    #[test]
    fn function_rejects_leading_slash() {
        let lines = vec!["/say hi".to_owned()];
        let error = Function::from_lines(Identifier::vanilla_static("test"), &lines).unwrap_err();
        assert!(error.contains("Do not use a preceding forwards slash"));
    }

    #[test]
    fn function_joins_continued_lines() {
        let lines = vec!["say hi \\".to_owned(), "there".to_owned()];
        let function = Function::from_lines(Identifier::vanilla_static("test"), &lines).unwrap();
        match &function.entries[0] {
            super::FunctionEntry::Plain(line) => assert_eq!(line, "say hi there"),
            other @ super::FunctionEntry::Macro(_) => {
                panic!("expected a plain entry, got {other:?}")
            }
        }
    }

    #[test]
    fn macro_names_are_collected_in_first_seen_order() {
        let lines = vec!["$say $(b) $(a) $(b)".to_owned()];
        let function = Function::from_lines(Identifier::vanilla_static("test"), &lines).unwrap();
        assert_eq!(function.macro_names, vec!["b", "a"]);
    }

    #[test]
    fn instantiate_reports_missing_macro_argument() {
        let lines = vec!["$say $(name)".to_owned()];
        let function = Function::from_lines(Identifier::vanilla_static("test"), &lines).unwrap();
        // Vanilla `MacroFunction.instantiate` rejects a call without arguments.
        assert!(function.expand_lines(None).is_err());

        let mut compound = NbtCompound::new();
        compound.insert("other", "value");
        let arguments = FunctionArguments::from_compound(&compound);
        assert!(function.expand_lines(Some(&arguments)).is_err());

        let mut compound = NbtCompound::new();
        compound.insert("name", "world");
        let arguments = FunctionArguments::from_compound(&compound);
        assert_eq!(
            function.expand_lines(Some(&arguments)).unwrap(),
            vec!["say world".to_owned()]
        );
    }

    #[test]
    fn arguments_come_from_compound_strings() {
        let mut compound = NbtCompound::new();
        compound.insert("name", "world");
        let arguments = FunctionArguments::from_compound(&compound);
        assert_eq!(arguments.names(), vec!["name"]);
        assert!(arguments.has_all_of(&["name".to_owned()]));
    }

    #[test]
    fn run_function_is_reachable() {
        // Type-level check that the public entry point keeps its signature.
        let _ = run_function as fn(&Arc<CommandSource>, &Function, Option<&FunctionArguments>) -> _;
    }
}
