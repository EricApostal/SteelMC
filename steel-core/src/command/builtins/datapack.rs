//! Vanilla `/datapack` command.
//!
//! Mirrors `DataPackCommand`: `list [available|enabled]`, `enable <name>` with
//! the `first`/`last`/`before`/`after` position forms, `disable <name>`, and
//! `create <id> <description>`.
//!
//! `list` re-discovers packs first, like vanilla's `PackRepository.reload()`, so a
//! directory added to the world's `datapacks/` folder appears without a restart.
//! Every mutation persists the selection file and immediately re-runs the resource
//! reload, exactly like vanilla's `ReloadCommand.reloadPacks` call.
//!
//! Deliberate difference: Steel has no per-world experimental feature flags, so a
//! pack that requires any `features.enabled` entry reports vanilla's
//! `commands.datapack.enable.failed.no_flags`.

use steel_utils::{
    Identifier,
    translations::{
        COMMANDS_DATAPACK_CREATE_ALREADY_EXISTS, COMMANDS_DATAPACK_CREATE_INVALID_FULL_NAME,
        COMMANDS_DATAPACK_CREATE_INVALID_NAME, COMMANDS_DATAPACK_CREATE_IO_FAILURE,
        COMMANDS_DATAPACK_CREATE_METADATA_ENCODE_FAILURE, COMMANDS_DATAPACK_CREATE_SUCCESS,
        COMMANDS_DATAPACK_DISABLE_FAILED, COMMANDS_DATAPACK_DISABLE_FAILED_FEATURE,
        COMMANDS_DATAPACK_ENABLE_FAILED, COMMANDS_DATAPACK_ENABLE_FAILED_NO_FLAGS,
        COMMANDS_DATAPACK_LIST_AVAILABLE_NONE, COMMANDS_DATAPACK_LIST_AVAILABLE_SUCCESS,
        COMMANDS_DATAPACK_LIST_ENABLED_NONE, COMMANDS_DATAPACK_LIST_ENABLED_SUCCESS,
        COMMANDS_DATAPACK_MODIFY_DISABLE, COMMANDS_DATAPACK_MODIFY_ENABLE,
        COMMANDS_DATAPACK_UNKNOWN, PACK_NAME_AND_SOURCE, PACK_SOURCE_BUILTIN, PACK_SOURCE_FEATURE,
        PACK_SOURCE_WORLD,
    },
};
use text_components::{
    Modifier, TextComponent, format::Color, interactivity::HoverEvent, translation::Translation,
};

use super::super::{
    brigadier::{
        ArgumentType, CommandNodeBuilder, CommandRequirement, CommandSyntaxError,
        SuggestionsBuilder,
    },
    execution::{
        CommandPermissionSource, CommandSource, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, SteelSuggestionContext, argument, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use crate::{
    datapack::{
        self, PackInsert, PackSelectionError,
        pack::{DATA_PACK_FORMAT, PackSource},
    },
    permission::{PermissionExpr, PermissionLevel, PermissionState},
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("datapack"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("datapack")
        .then(
            literal("list")
                .executes(|context| list_all(context))
                .then(literal("available").executes(|context| {
                    rediscover_for_list(datapack::datapacks());
                    list_available(context)
                }))
                .then(literal("enabled").executes(|context| {
                    rediscover_for_list(datapack::datapacks());
                    list_enabled(context)
                })),
        )
        .then(
            literal("enable").then(
                argument("name", ArgumentType::string())
                    .suggests(suggest_unenabled)
                    .executes(|context| enable(context, PackInsert::Last))
                    .then(literal("last").executes(|context| enable(context, PackInsert::Last)))
                    .then(literal("first").executes(|context| enable(context, PackInsert::First)))
                    .then(
                        literal("before").then(
                            argument("existing", ArgumentType::string())
                                .suggests(suggest_enabled)
                                .executes(|context| {
                                    let existing = context.string("existing")?.to_owned();
                                    enable(context, PackInsert::Before(existing))
                                }),
                        ),
                    )
                    .then(
                        literal("after").then(
                            argument("existing", ArgumentType::string())
                                .suggests(suggest_enabled)
                                .executes(|context| {
                                    let existing = context.string("existing")?.to_owned();
                                    enable(context, PackInsert::After(existing))
                                }),
                        ),
                    ),
            ),
        )
        .then(
            literal("disable").then(
                argument("name", ArgumentType::string())
                    .suggests(suggest_enabled)
                    .executes(disable),
            ),
        )
        .then(literal("create").requires(owner_requirement()).then(
            argument("id", ArgumentType::string()).then(
                argument("description", SteelArgumentType::component()).executes(create_pack),
            ),
        ))
}

/// Vanilla `DataPackCommand.listPacks`: enabled packs then available packs.
fn list_all(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    rediscover_for_list(datapack::datapacks());
    let enabled = list_enabled(context)?;
    let available = list_available(context)?;
    Ok(enabled.saturating_add(available))
}

/// Vanilla `PackRepository.reload()` at the start of both list forms.
///
/// Vanilla's `list` only re-discovers packs: it never re-reads resources and never
/// selects a pack that was not already selected, so a pack dropped into
/// `datapacks/` still shows up under `list available` until `/reload` or
/// `/datapack enable` selects it.
///
/// Re-discovery can only fail on unreadable pack metadata; vanilla logs and keeps
/// the previous view in that case, so the listing still shows what is loaded.
fn rediscover_for_list(manager: &datapack::DatapackManager) {
    if let Err(error) = manager.rediscover() {
        log::warn!("Failed to re-discover data packs: {error}");
    }
}

/// Vanilla `DataPackCommand.listEnabledPacks`.
fn list_enabled(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let manager = datapack::datapacks();
    let repository = manager.repository().read();
    let ids = repository.enabled_ids();
    drop(repository);

    let message = if ids.is_empty() {
        TextComponent::from(&COMMANDS_DATAPACK_LIST_ENABLED_NONE)
    } else {
        let links = pack_links(&ids, true);
        COMMANDS_DATAPACK_LIST_ENABLED_SUCCESS
            .message([TextComponent::from(ids.len().to_string()), links])
            .component()
    };
    context.source().send_success(&message, false);
    count(ids.len())
}

/// Vanilla `DataPackCommand.listAvailablePacks`: discovered but not enabled.
fn list_available(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let manager = datapack::datapacks();
    let repository = manager.repository().read();
    let ids: Vec<String> = repository
        .available_ids()
        .into_iter()
        .filter(|id| !repository.is_enabled(id))
        .collect();
    drop(repository);

    let message = if ids.is_empty() {
        TextComponent::from(&COMMANDS_DATAPACK_LIST_AVAILABLE_NONE)
    } else {
        let links = pack_links(&ids, false);
        COMMANDS_DATAPACK_LIST_AVAILABLE_SUCCESS
            .message([TextComponent::from(ids.len().to_string()), links])
            .component()
    };
    context.source().send_success(&message, false);
    count(ids.len())
}

/// Vanilla `DataPackCommand.enablePack`.
fn enable(
    context: &SteelCommandContext<CommandSource>,
    position: PackInsert,
) -> Result<i32, CommandSyntaxError> {
    let id = context.string("name")?.to_owned();
    let manager = datapack::datapacks();

    {
        let repository = manager.repository().read();
        let Some(pack) = repository.pack(&id) else {
            return Err(unknown_pack(&id));
        };
        if repository.is_enabled(&id) {
            return Err(dynamic(&COMMANDS_DATAPACK_ENABLE_FAILED, &id));
        }
        if !pack.required_features.is_empty() {
            // Steel has no per-world experimental flags, so every requested flag
            // is unavailable; vanilla reports the same for a world without them.
            let flags = pack
                .required_features
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CommandSyntaxError::dynamic(
                COMMANDS_DATAPACK_ENABLE_FAILED_NO_FLAGS
                    .message([
                        TextComponent::plain(id.clone()),
                        TextComponent::plain(flags),
                    ])
                    .component(),
            ));
        }
    }

    apply_selection(&id, true, position)?;
    let link = pack_link(&id, true);
    context.source().send_success(
        &COMMANDS_DATAPACK_MODIFY_ENABLE.message([link]).component(),
        true,
    );
    reload_after_selection(context)?;
    enabled_count()
}

/// Vanilla `DataPackCommand.disablePack`.
fn disable(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.string("name")?.to_owned();
    let manager = datapack::datapacks();

    {
        let repository = manager.repository().read();
        let Some(pack) = repository.pack(&id) else {
            return Err(unknown_pack(&id));
        };
        if !repository.is_enabled(&id) {
            return Err(dynamic(&COMMANDS_DATAPACK_DISABLE_FAILED, &id));
        }
        if matches!(pack.location.source, PackSource::Feature) && !pack.required_features.is_empty()
        {
            return Err(dynamic(&COMMANDS_DATAPACK_DISABLE_FAILED_FEATURE, &id));
        }
    }

    apply_selection(&id, false, PackInsert::Last)?;
    let link = pack_link(&id, false);
    context.source().send_success(
        &COMMANDS_DATAPACK_MODIFY_DISABLE.message([link]).component(),
        true,
    );
    reload_after_selection(context)?;
    enabled_count()
}

/// Applies one selection change and persists it.
fn apply_selection(
    id: &str,
    enabled: bool,
    position: PackInsert,
) -> Result<(), CommandSyntaxError> {
    let manager = datapack::datapacks();
    let changed = {
        let mut repository = manager.repository().write();
        repository
            .set_selected(id, enabled, position)
            .map_err(|error| selection_error(id, enabled, error))?
    };
    if !changed {
        // `set_selected` reported no change; vanilla only reaches here for a
        // stale enable/disable, which the callers already reject.
        return Ok(());
    }
    manager
        .save_selection()
        .map_err(|error| CommandSyntaxError::dynamic(error.to_string()))?;
    Ok(())
}

/// Vanilla's `ReloadCommand.reloadPacks` after a selection change.
fn reload_after_selection(
    context: &SteelCommandContext<CommandSource>,
) -> Result<(), CommandSyntaxError> {
    match datapack::datapacks().reload() {
        Ok(summary) => {
            tracing::info!(
                packs = summary.packs_loaded,
                pack_ids = ?summary.pack_ids,
                new_packs = ?summary.new_packs,
                functions = summary.functions_loaded,
                elapsed = ?summary.elapsed,
                "Reloaded data packs after selection change"
            );
            datapack::resend_clients(context.source().server());
            Ok(())
        }
        Err(error) => {
            log::warn!("Failed to execute reload: {error}");
            context.source().send_failure(TextComponent::from(
                &steel_utils::translations::COMMANDS_RELOAD_FAILURE,
            ));
            Ok(())
        }
    }
}

fn enabled_count() -> Result<i32, CommandSyntaxError> {
    let manager = datapack::datapacks();
    let enabled = manager.repository().read().enabled_ids().len();
    count(enabled)
}

fn selection_error(id: &str, enabling: bool, error: PackSelectionError) -> CommandSyntaxError {
    match error {
        PackSelectionError::UnknownPack(_) => unknown_pack(id),
        PackSelectionError::AnchorNotEnabled(anchor) => CommandSyntaxError::dynamic(
            COMMANDS_DATAPACK_UNKNOWN
                .message([TextComponent::plain(anchor)])
                .component(),
        ),
        PackSelectionError::PackRequiredByOthers { .. } => {
            dynamic(&COMMANDS_DATAPACK_DISABLE_FAILED_FEATURE, id)
        }
        PackSelectionError::Unselectable(_) if enabling => {
            dynamic(&COMMANDS_DATAPACK_ENABLE_FAILED, id)
        }
        PackSelectionError::Unselectable(_) => dynamic(&COMMANDS_DATAPACK_DISABLE_FAILED, id),
    }
}

fn unknown_pack(id: &str) -> CommandSyntaxError {
    dynamic(&COMMANDS_DATAPACK_UNKNOWN, id)
}

fn dynamic(translation: &Translation<1>, id: &str) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translation
            .message([TextComponent::plain(id.to_owned())])
            .component(),
    )
}

fn count(value: usize) -> Result<i32, CommandSyntaxError> {
    i32::try_from(value).map_err(|_| {
        CommandSyntaxError::dynamic("Data pack count exceeds the command result range")
    })
}

/// Vanilla `ComponentUtils.formatList` over each pack's chat link.
fn pack_links(ids: &[String], enabled: bool) -> TextComponent {
    let mut list = TextComponent::plain("");
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            list = list.add_child(TextComponent::plain(", ").color(Color::Gray));
        }
        list = list.add_child(pack_link(id, enabled));
    }
    list
}

/// Vanilla `PackLocationInfo.createChatLink`: `[name (source)]` colored by state.
fn pack_link(id: &str, enabled: bool) -> TextComponent {
    let (source, description) = {
        let manager = datapack::datapacks();
        let repository = manager.repository().read();
        let Some(pack) = repository.pack(id) else {
            return TextComponent::plain(format!("[{id}]"));
        };
        let source = match pack.location.source {
            PackSource::Builtin => TextComponent::from(&PACK_SOURCE_BUILTIN),
            PackSource::Feature => TextComponent::from(&PACK_SOURCE_FEATURE),
            PackSource::World => TextComponent::from(&PACK_SOURCE_WORLD),
        };
        let description = pack
            .metadata
            .as_ref()
            .map(|metadata| metadata.description.clone())
            .unwrap_or_default();
        (source, description)
    };

    let name = PACK_NAME_AND_SOURCE
        .message([TextComponent::plain(id.to_owned()), source])
        .component();
    let link = steel_utils::translations::CHAT_SQUARE_BRACKETS
        .message([name])
        .component()
        .color(if enabled { Color::Green } else { Color::Red })
        .insertion(id.to_owned());
    if description.is_empty() {
        link
    } else {
        link.hover_event(HoverEvent::show_text(description_text(&description)))
    }
}

/// Renders a `pack.mcmeta` description for the hover tooltip.
///
/// The pack metadata keeps the raw JSON it read, which is either a plain string or
/// an encoded text component (`/datapack create` writes the latter, like vanilla).
fn description_text(raw: &str) -> TextComponent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return TextComponent::plain(raw.to_owned());
    };
    match &value {
        serde_json::Value::String(text) => TextComponent::plain(text.clone()),
        serde_json::Value::Object(_) => value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || TextComponent::plain(value.to_string()),
                |text| TextComponent::plain(text.to_owned()),
            ),
        _ => TextComponent::plain(value.to_string()),
    }
}

/// Vanilla `DataPackCommand`'s `create` subtree requirement (`LEVEL_OWNERS`).
///
/// Steel's registration API gates a whole root at one level, so the subcommand
/// carries the level requirement on its own node instead. The node is hidden from
/// sources below op level 4, exactly like vanilla's `requires(...)`.
fn owner_requirement() -> CommandRequirement<CommandSource> {
    CommandRequirement::authorization(|source: &CommandSource| {
        source.permission_state(&PermissionExpr::level(PermissionLevel::Owners))
            == Some(PermissionState::Allow)
    })
}

/// Vanilla `DataPackCommand.createPack`: writes a new world pack and re-discovers.
fn create_pack(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.string("id")?.to_owned();
    let description = context.text_component("description")?;

    if !is_valid_path_segment(&id) {
        return Err(dynamic(&COMMANDS_DATAPACK_CREATE_INVALID_NAME, &id));
    }
    if !is_path_part_portable(&id) {
        return Err(dynamic(&COMMANDS_DATAPACK_CREATE_INVALID_FULL_NAME, &id));
    }

    let manager = datapack::datapacks();
    let pack_dir = manager.world_datapack_dir().join(&id);
    if pack_dir.exists() {
        return Err(dynamic(&COMMANDS_DATAPACK_CREATE_ALREADY_EXISTS, &id));
    }

    let description = serde_json::to_value(description).map_err(|error| {
        CommandSyntaxError::dynamic(
            COMMANDS_DATAPACK_CREATE_METADATA_ENCODE_FAILURE
                .message([
                    TextComponent::plain(id.clone()),
                    TextComponent::plain(error.to_string()),
                ])
                .component(),
        )
    })?;
    let metadata = serde_json::json!({
        "pack": {
            "min_format": DATA_PACK_FORMAT,
            "max_format": DATA_PACK_FORMAT,
            "description": description,
        }
    });
    let bytes = serde_json::to_vec_pretty(&metadata).map_err(|error| {
        CommandSyntaxError::dynamic(
            COMMANDS_DATAPACK_CREATE_METADATA_ENCODE_FAILURE
                .message([
                    TextComponent::plain(id.clone()),
                    TextComponent::plain(error.to_string()),
                ])
                .component(),
        )
    })?;

    let write = std::fs::create_dir_all(pack_dir.join("data"))
        .and_then(|()| std::fs::write(pack_dir.join("pack.mcmeta"), bytes));
    if let Err(error) = write {
        log::warn!("Failed to create pack at {}: {error}", pack_dir.display());
        return Err(dynamic(&COMMANDS_DATAPACK_CREATE_IO_FAILURE, &id));
    }

    // Vanilla only writes the files; Steel re-discovers immediately so a
    // following `/datapack enable <id>` resolves the new pack without a `/reload`.
    if let Err(error) = manager.rediscover() {
        log::warn!("Failed to re-discover data packs after creating `{id}`: {error}");
    }

    context.source().send_success(
        &COMMANDS_DATAPACK_CREATE_SUCCESS
            .message([TextComponent::plain(id)])
            .component(),
        true,
    );
    Ok(1)
}

/// Vanilla `FileUtil.isValidPathSegment`: `[-._a-z0-9]+`, excluding `.` and `..`.
fn is_valid_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '.' | '_')
        })
}

/// Vanilla `FileUtil.isPathPartPortable`: rejects Windows-reserved device names
/// and names ending in a dot.
fn is_path_part_portable(name: &str) -> bool {
    // Vanilla `RESERVED_WINDOWS_FILENAMES`: bare `COM`, the classic device names,
    // and `COM1-9`/`LPT1-9`. Bare `LPT` is not reserved, only its numbered forms.
    const RESERVED: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CLOCK$", "COM"];

    if name.ends_with('.') {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return false;
    }
    // COM1-9 / LPT1-9, matching vanilla's `COM[1-9]`/`LPT[1-9]` alternatives.
    let reserved_prefix = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|rest| rest.len() == 1 && matches!(rest.as_bytes()[0], b'1'..=b'9'))
    };
    !(reserved_prefix("COM") || reserved_prefix("LPT"))
}

fn suggest_enabled(
    _context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let manager = datapack::datapacks();
    let ids = manager.repository().read().enabled_ids();
    drop(manager);
    suggest_list(builder, ids);
}

fn suggest_unenabled(
    _context: &SteelSuggestionContext<'_, CommandSource>,
    builder: &mut SuggestionsBuilder<'_>,
) {
    let manager = datapack::datapacks();
    let repository = manager.repository().read();
    let ids: Vec<String> = repository
        .available_ids()
        .into_iter()
        .filter(|id| !repository.is_enabled(id))
        .collect();
    drop(repository);
    suggest_list(builder, ids);
}
