//! Vanilla `/compute` command.
//!
//! Mirrors `net.minecraft.server.commands.ComputeCommand`: a context decorator
//! (`default`, `block <computePos>`, `entity <computeTarget>`) selects the loot
//! parameters, then an `integer`/`float` branch evaluates a
//! `ResourceOrIdArgument` number provider against that context and reports the
//! result.
//!
//! Context parity: every decorator inherits vanilla `LootContextSources`
//! `createParams`, which always installs `THIS_ENTITY` (the source entity, when
//! present) and `ORIGIN` (the source position); `default` adds nothing else and
//! `block <pos>` adds `BLOCK_STATE` plus the optional `BLOCK_ENTITY`. Steel's
//! `LootContext` always carries the level view, matching vanilla's
//! `LootContext.Builder(level)`.
//!
//! Known gaps (named, not stubbed):
//! - `BLOCK_ENTITY` carries the block entity type and, for a container block
//!   entity, the snapshotted `container` stacks; Steel's reduced
//!   `BlockEntityRef` has no live custom-name view, and no number provider reads
//!   the block entity at all.
//! - `minecraft:score`/`minecraft:storage`/`minecraft:environment_attribute`
//!   evaluate to their fallback because `/compute` installs no
//!   scoreboard/storage/attribute hooks (Steel has no `*Lookup` implementors
//!   yet).
//! - The invalid-value `%s` reason for `integer` providers is generic: Steel's
//!   `ContextIntProvider::get_int_unsafe` returns `Option<i32>` and drops Java's
//!   `ArithmeticException` message.

use glam::DVec3;
use rand::rngs::ThreadRng;
use steel_registry::loot_table::{BlockEntityRef, LootContext};
use steel_utils::{Identifier, translations};
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
    entity::loot::loot_level_view,
    entity::{entity_loot_ref, loot_equipment},
    loot::slot::SlotProvider,
    loot::{LootSlotSources, LootedContainer},
    permission::PermissionLevel,
    world::LevelReader as _,
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("compute"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

/// Vanilla `LootContextSources.addContextSources`' `ContextDecorator` choices
/// of this command; every one installs the shared `THIS_ENTITY`/`ORIGIN`.
#[derive(Clone, Copy)]
enum ComputeSource {
    /// `default`: only the source's `THIS_ENTITY`/`ORIGIN`.
    Default,
    /// `block <computePos>`: additionally `BLOCK_STATE` (+ `BLOCK_ENTITY`).
    Block,
    /// `entity <computeTarget>`: additionally `TARGET_ENTITY`.
    Entity,
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("compute")
        .then(literal("default").then(sources(ComputeSource::Default)))
        .then(
            literal("block").then(
                argument("computePos", SteelArgumentType::block_pos())
                    .then(sources(ComputeSource::Block)),
            ),
        )
        // `entity <computeTarget>`; Steel's `LootContext` now models vanilla's
        // `LootContextParams.TARGET_ENTITY`.
        .then(
            literal("entity").then(
                argument("computeTarget", SteelArgumentType::entity())
                    .then(sources(ComputeSource::Entity)),
            ),
        )
}

/// Vanilla `ComputeCommand.register`'s per-decorator `float`/`integer` branches.
fn sources(source: ComputeSource) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("float")
        .then(
            argument("provider", SteelArgumentType::float_number_provider())
                .executes(move |context| compute_as_float(context, source, 1.0))
                .then(
                    argument("scale", ArgumentType::float(f32::MIN, f32::MAX)).executes(
                        move |context| {
                            let scale = context.float("scale")?;
                            compute_as_float(context, source, scale)
                        },
                    ),
                ),
        )
        .then(
            literal("integer").then(
                argument("provider", SteelArgumentType::int_number_provider())
                    .executes(move |context| compute_as_integer(context, source)),
            ),
        )
}

/// Vanilla `ComputeCommand.computeAsInt`.
fn compute_as_integer(
    context: &SteelCommandContext<CommandSource>,
    source: ComputeSource,
) -> Result<i32, CommandSyntaxError> {
    let provider = context.int_number_provider("provider")?;

    match with_params(context, source, |params| provider.get_int_unsafe(params))? {
        Some(value) => {
            print_exact(context, provider.identifier(), value);
            Ok(value)
        }
        None => Err(invalid_value(provider.identifier(), INT_INVALID_REASON)),
    }
}

/// Vanilla `ComputeCommand.computeAsFloat`.
#[expect(clippy::float_cmp, reason = "vanilla compares exactly")]
fn compute_as_float(
    context: &SteelCommandContext<CommandSource>,
    source: ComputeSource,
    scale: f32,
) -> Result<i32, CommandSyntaxError> {
    let provider = context.float_number_provider("provider")?;

    let value = with_params(context, source, |params| provider.get_float_unsafe(params))?;
    if !value.is_finite() {
        return Err(invalid_value(
            provider.identifier(),
            &java_float_string(value),
        ));
    }

    let rounded = floor_to_int(value * scale);
    if (rounded as f32) == value {
        print_exact(context, provider.identifier(), rounded);
    } else {
        print_rounded(context, provider.identifier(), rounded, value);
    }
    Ok(rounded)
}

/// Vanilla `ComputeCommand.printExactOutput`.
fn print_exact(
    context: &SteelCommandContext<CommandSource>,
    name: Option<&Identifier>,
    value: i32,
) {
    let value = TextComponent::from(value.to_string());
    let message = match name {
        Some(name) => translations::COMMAND_COMPUTE_RESULT_NAMED_EXACT
            .message([plain_id(name), value])
            .component(),
        None => translations::COMMAND_COMPUTE_RESULT_UNNAMED_EXACT
            .message([value])
            .component(),
    };
    context.source().send_success(&message, false);
}

/// Vanilla `ComputeCommand.printRoundedOutput`.
fn print_rounded(
    context: &SteelCommandContext<CommandSource>,
    name: Option<&Identifier>,
    rounded: i32,
    value: f32,
) {
    let rounded = TextComponent::from(rounded.to_string());
    let value = TextComponent::from(value.to_string());
    let message = match name {
        Some(name) => translations::COMMAND_COMPUTE_RESULT_NAMED_ROUNDED
            .message([plain_id(name), value, rounded])
            .component(),
        None => translations::COMMAND_COMPUTE_RESULT_UNNAMED_ROUNDED
            .message([value, rounded])
            .component(),
    };
    context.source().send_success(&message, false);
}

/// Vanilla `ComputeCommand.throwInvalidValue`.
fn invalid_value(name: Option<&Identifier>, reason: &str) -> CommandSyntaxError {
    let reason = TextComponent::plain(reason.to_owned());
    let message = match name {
        Some(name) => translations::COMMAND_COMPUTE_RESULT_NAMED_INVALID
            .message([plain_id(name), reason])
            .component(),
        None => translations::COMMAND_COMPUTE_RESULT_UNNAMED_INVALID
            .message([reason])
            .component(),
    };
    CommandSyntaxError::dynamic(message)
}

/// Vanilla `Component.translationArg(Identifier)` renders the id literally.
fn plain_id(id: &Identifier) -> TextComponent {
    TextComponent::plain(id.to_string())
}

/// Java `Mth.floor(float)`: floor, then the saturating `(int)` cast.
const fn floor_to_int(value: f32) -> i32 {
    value.floor() as i32
}

/// Java `Float.toString` for the non-finite values `/compute` can observe.
fn java_float_string(value: f32) -> String {
    if value.is_nan() {
        "NaN".to_owned()
    } else if value == f32::INFINITY {
        "Infinity".to_owned()
    } else if value == f32::NEG_INFINITY {
        "-Infinity".to_owned()
    } else {
        value.to_string()
    }
}

/// Reason reported for an `integer` provider that returned no value.
///
/// Steel's `ContextIntProvider::get_int_unsafe` collapses Java's
/// `ArithmeticException` (overflow, division by zero, non-representable float)
/// into `None`, so the specific message vanilla forwards is unavailable.
const INT_INVALID_REASON: &str = "arithmetic error";

/// Builds the decorated context and evaluates `provider` against it.
///
/// Lives behind a small visitor so the borrows of the RNG and the source
/// entity's equipment outlive the provider call.
///
/// `default` inherits only `THIS_ENTITY`/`ORIGIN` from vanilla `createParams`;
/// `block` additionally installs `BLOCK_STATE` and, when present,
/// `BLOCK_ENTITY`. All sources carry the level view that
/// `minecraft:location_check` reads, matching vanilla's
/// `LootContext.Builder(level)`.
fn with_params<T>(
    context: &SteelCommandContext<CommandSource>,
    source: ComputeSource,
    evaluate: impl FnOnce(&mut LootContext<'_, ThreadRng>) -> T,
) -> Result<T, CommandSyntaxError> {
    let command_source = context.source();
    let origin: DVec3 = command_source.position();
    let equipment = command_source
        .entity()
        .and_then(|entity| loot_equipment(entity.as_ref()));

    // The borrowed views below point into these owned locals, so they must outlive the
    // `LootContext` rather than being scoped to the branch that builds them.
    let block_source = if matches!(source, ComputeSource::Block) {
        let pos = loaded_block_position(context, "computePos")?;
        Some((pos, command_source.world().get_block_entity(pos)))
    } else {
        None
    };
    let target_source = if matches!(source, ComputeSource::Entity) {
        Some(context.entity("computeTarget")?)
    } else {
        None
    };
    let target_equipment = target_source
        .as_ref()
        .and_then(|entity| loot_equipment(entity.as_ref()));

    let mut rng = rand::rng();
    // The looted block entity's container supplies the `minecraft:dynamic`
    // `minecraft:contents` drop and the `container`/`block_entity` slot
    // sources, snapshotted so no container lock crosses the loot evaluation.
    let looted_block_entity = block_source
        .as_ref()
        .and_then(|(_, entity)| entity.as_deref());
    let container = LootedContainer::snapshot(looted_block_entity);
    let slot_sources = LootSlotSources::new(
        container
            .as_ref()
            .map(|container| container as &dyn SlotProvider),
        None,
        container
            .as_ref()
            .map(|container| container as &dyn SlotProvider),
    );
    let mut params = LootContext::new(&mut rng)
        .with_origin(origin.x, origin.y, origin.z)
        .with_slot_sources(&slot_sources);
    if let Some(entity) = command_source.entity() {
        params = params.with_this_entity(entity_loot_ref(entity.as_ref(), equipment.as_ref()));
    }
    if let Some((pos, block_entity)) = &block_source {
        params = params.with_block_state(command_source.world().get_block_state(*pos));
        if let Some(block_entity) = block_entity {
            params = params.with_block_entity(BlockEntityRef {
                block_entity_type: Some(&block_entity.get_type().key),
                custom_name: None,
                inventory: container.as_ref().map(LootedContainer::items),
            });
        }
    }
    if let Some(entity) = &target_source {
        params =
            params.with_target_entity(entity_loot_ref(entity.as_ref(), target_equipment.as_ref()));
    }
    let level = loot_level_view(command_source.world(), (origin.x, origin.y, origin.z));
    params = params.with_location(&level);

    Ok(evaluate(&mut params))
}
