//! Vanilla random number command (range sampling only).
//!
//! Implements `random value <range>` and `random roll <range>` against the full
//! vanilla parse tree and messages. The `<sequence>` and `reset` branches are
//! intentionally absent: vanilla draws those from the world's persistent
//! `RandomSequences` saved data (`/tmp/vanilla/src/world/RandomSequences.java`),
//! which Steel does not have yet (owner: SpawnerWorld).

use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
pub(super) fn registration() -> CommandRegistration<CommandSource> {
    // Vanilla's `value` and `roll` branches carry no permission requirement;
    // only the (not yet implemented) `reset` branch needs game masters.
    CommandRegistration::new(Identifier::vanilla_static("random"), |_| command()).default_access()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("random")
        .then(value_or_roll("value", false))
        .then(value_or_roll("roll", true))
}

fn value_or_roll(
    name: &'static str,
    announce: bool,
) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal(name).then(
        argument("range", SteelArgumentType::int_range())
            .executes(move |context| random_sample(context, announce)),
    )
}

/// Mirrors vanilla `RandomCommand.randomSample`.
fn random_sample(
    context: &SteelCommandContext<CommandSource>,
    announce: bool,
) -> Result<i32, CommandSyntaxError> {
    let range = context.int_range("range")?;
    let minimum = range.minimum().unwrap_or(i32::MIN);
    let maximum = range.maximum().unwrap_or(i32::MAX);
    let span = i64::from(maximum) - i64::from(minimum);
    if span == 0 {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_RANDOM_ERROR_RANGE_TOO_SMALL
                .msg()
                .component(),
        ));
    }
    if span >= i64::from(i32::MAX) {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_RANDOM_ERROR_RANGE_TOO_LARGE
                .msg()
                .component(),
        ));
    }
    // Divergence: vanilla draws from `ServerLevel.random` (or from the world's
    // persistent `RandomSequences` entry when `<sequence>` is given). Steel
    // exposes neither yet (owner: SpawnerWorld), so range-only sampling uses
    // the process RNG, matching Steel's other entropy draws.
    let value = rand::random_range(minimum..=maximum);
    if announce {
        let message = translations::COMMANDS_RANDOM_ROLL
            .message([
                source_display_name(context.source()),
                TextComponent::from(value.to_string()),
                TextComponent::from(minimum.to_string()),
                TextComponent::from(maximum.to_string()),
            ])
            .component();
        context
            .source()
            .server()
            .broadcast_system_chat(&message, None);
    } else {
        let message = translations::COMMANDS_RANDOM_SAMPLE_SUCCESS
            .message([TextComponent::from(value.to_string())])
            .component();
        context.source().send_success(&message, false);
    }
    Ok(value)
}

/// Mirrors vanilla `CommandSourceStack.getDisplayName`.
fn source_display_name(source: &CommandSource) -> TextComponent {
    match source.entity() {
        Some(entity) => entity.display_name(),
        None => TextComponent::plain(source.sender().to_string()),
    }
}
