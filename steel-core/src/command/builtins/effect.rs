//! Vanilla mob-effect grant/clear command.
//!
//! `effect clear` with no targets clears the command source's own entity,
//! matching `CommandSourceStack.getEntityOrException`.

use std::borrow::Cow;

use steel_registry::mob_effect::MobEffectRef;
use steel_utils::{Identifier, translations};
use text_components::{TextComponent, translation::TranslatedMessage};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::{
    behavior::MOB_EFFECT_BEHAVIORS,
    entity::{LivingEntity, MobEffectInstance, SharedEntity},
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("effect"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("effect")
        .then(
            literal("clear").executes(clear_source_effects).then(
                argument("targets", SteelArgumentType::entities())
                    .executes(clear_target_effects)
                    .then(
                        argument("effect", SteelArgumentType::mob_effect())
                            .executes(clear_specific_effect),
                    ),
            ),
        )
        .then(
            literal("give").then(
                argument("targets", SteelArgumentType::entities()).then(
                    argument("effect", SteelArgumentType::mob_effect())
                        .executes(give_default_effect)
                        .then(
                            argument("seconds", ArgumentType::integer(1, 1_000_000))
                                .executes(give_timed_effect)
                                .then(
                                    argument("amplifier", ArgumentType::integer(0, 255))
                                        .executes(give_timed_amplified_effect)
                                        .then(
                                            argument("hideParticles", ArgumentType::bool())
                                                .executes(give_timed_hidden_effect),
                                        ),
                                ),
                        )
                        .then(
                            literal("infinite").executes(give_infinite_effect).then(
                                argument("amplifier", ArgumentType::integer(0, 255))
                                    .executes(give_infinite_amplified_effect)
                                    .then(
                                        argument("hideParticles", ArgumentType::bool())
                                            .executes(give_infinite_hidden_effect),
                                    ),
                            ),
                        ),
                ),
            ),
        )
}

/// `effect clear` — clears every effect from the source's own entity.
fn clear_source_effects(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let Some(entity) = context.source().entity() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_ENTITY,
        )));
    };
    clear_all_effects(context, &[entity])
}

/// `effect clear <targets>`.
fn clear_target_effects(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    let targets: Vec<&SharedEntity> = targets.iter().collect();
    clear_all_effects(context, &targets)
}

/// `effect clear <targets> <effect>`.
fn clear_specific_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    let effect = context.mob_effect("effect")?;
    let mut cleared: Vec<&SharedEntity> = Vec::new();
    for target in &targets {
        let Some(living) = target.as_living_entity() else {
            continue;
        };
        if living.remove_mob_effect(effect) {
            cleared.push(target);
        }
    }
    if cleared.is_empty() {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_EFFECT_CLEAR_SPECIFIC_FAILED,
        )));
    }
    send_clear_specific_success(context, &cleared, effect);
    i32::try_from(cleared.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Effect clear count exceeds the command result range")
    })
}

fn clear_all_effects(
    context: &SteelCommandContext<CommandSource>,
    targets: &[&SharedEntity],
) -> Result<i32, CommandSyntaxError> {
    let mut cleared: Vec<&SharedEntity> = Vec::new();
    for &target in targets {
        let Some(living) = target.as_living_entity() else {
            continue;
        };
        if remove_all_effects(living) {
            cleared.push(target);
        }
    }
    if cleared.is_empty() {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_EFFECT_CLEAR_EVERYTHING_FAILED,
        )));
    }
    // Vanilla dispatches on the `NON_ZERO` element, so the single form fires
    // only when exactly one target actually had effects removed.
    let message = match cleared.as_slice() {
        [target] => translations::COMMANDS_EFFECT_CLEAR_EVERYTHING_SUCCESS_SINGLE
            .message([target.display_name()])
            .component(),
        _ => translations::COMMANDS_EFFECT_CLEAR_EVERYTHING_SUCCESS_MULTIPLE
            .message([cleared.len().to_string()])
            .component(),
    };
    context.source().send_success(&message, true);
    i32::try_from(cleared.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Effect clear count exceeds the command result range")
    })
}

/// Vanilla `LivingEntity.removeAllEffects`: removes every active status effect.
fn remove_all_effects(living: &dyn LivingEntity) -> bool {
    let mut removed = false;
    for active in living.active_mob_effects() {
        removed |= living.remove_mob_effect(active.effect());
    }
    removed
}

fn give_default_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    give_effect(context, None, 0, true)
}

fn give_timed_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    give_effect(context, Some(context.integer("seconds")?), 0, true)
}

fn give_timed_amplified_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    give_effect(
        context,
        Some(context.integer("seconds")?),
        context.integer("amplifier")?,
        true,
    )
}

fn give_timed_hidden_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    give_effect(
        context,
        Some(context.integer("seconds")?),
        context.integer("amplifier")?,
        !context.boolean("hideParticles")?,
    )
}

fn give_infinite_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    give_effect(context, Some(-1), 0, true)
}

fn give_infinite_amplified_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    give_effect(context, Some(-1), context.integer("amplifier")?, true)
}

fn give_infinite_hidden_effect(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    give_effect(
        context,
        Some(-1),
        context.integer("amplifier")?,
        !context.boolean("hideParticles")?,
    )
}

fn give_effect(
    context: &SteelCommandContext<CommandSource>,
    seconds: Option<i32>,
    amplifier: i32,
    show_particles: bool,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    let effect = context.mob_effect("effect")?;
    let duration = duration_in_ticks(seconds, effect);
    let mut applied: Vec<&SharedEntity> = Vec::new();
    for target in &targets {
        let Some(living) = target.as_living_entity() else {
            continue;
        };
        // Vanilla's fifth constructor argument is `visible` and the sixth
        // (`showIcon`) defaults to it, so both follow the `hideParticles`
        // inversion here.
        let instance = MobEffectInstance::with_duration(effect, duration, amplifier)
            .with_visible(show_particles)
            .with_show_icon(show_particles);
        if living.add_mob_effect(instance) {
            applied.push(target);
        }
    }
    if applied.is_empty() {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_EFFECT_GIVE_FAILED,
        )));
    }
    // Vanilla passes the duration/20 as a third formatting argument, but
    // `commands.effect.give.success.*` only declares two placeholders, so the
    // rendered message is exactly the effect name and the target(s).
    let effect_name = effect_display_name(effect);
    // Vanilla dispatches on the `NON_ZERO` element.
    let message = match applied.as_slice() {
        [target] => translations::COMMANDS_EFFECT_GIVE_SUCCESS_SINGLE
            .message([effect_name, target.display_name()])
            .component(),
        _ => translations::COMMANDS_EFFECT_GIVE_SUCCESS_MULTIPLE
            .message([effect_name, TextComponent::plain(applied.len().to_string())])
            .component(),
    };
    context.source().send_success(&message, true);
    i32::try_from(applied.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Effect apply count exceeds the command result range")
    })
}

/// Vanilla `EffectCommands.computeDurationInTicks`: instantaneous effects take
/// seconds verbatim, `-1` means infinite, and the no-seconds default is 1 tick
/// for instantaneous effects or 600 ticks (30 seconds) otherwise.
fn duration_in_ticks(seconds: Option<i32>, effect: MobEffectRef) -> i32 {
    let instantaneous = MOB_EFFECT_BEHAVIORS
        .get_behavior(effect)
        .as_instantaneous()
        .is_some();
    match seconds {
        Some(seconds) if instantaneous => seconds,
        Some(-1) => -1,
        Some(seconds) => seconds.saturating_mul(20),
        None if instantaneous => 1,
        None => 600,
    }
}

/// Translatable display name for a mob effect,
/// `effect.<namespace>.<path>` like vanilla `MobEffect.getDisplayName`.
fn effect_display_name(effect: MobEffectRef) -> TextComponent {
    TextComponent::translated(TranslatedMessage {
        key: Cow::Owned(format!(
            "effect.{}.{}",
            effect.key.namespace, effect.key.path
        )),
        args: None,
        fallback: None,
    })
}

/// Sends the single/multiple clear feedback for the targets that actually lost
/// the effect, matching vanilla's `NON_ZERO` element dispatch.
fn send_clear_specific_success(
    context: &SteelCommandContext<CommandSource>,
    cleared: &[&SharedEntity],
    effect: MobEffectRef,
) {
    let effect_name = effect_display_name(effect);
    let message = match cleared {
        [target] => translations::COMMANDS_EFFECT_CLEAR_SPECIFIC_SUCCESS_SINGLE
            .message([effect_name, target.display_name()])
            .component(),
        _ => translations::COMMANDS_EFFECT_CLEAR_SPECIFIC_SUCCESS_MULTIPLE
            .message([effect_name, TextComponent::plain(cleared.len().to_string())])
            .component(),
    };
    context.source().send_success(&message, true);
}
