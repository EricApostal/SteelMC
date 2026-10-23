//! Vanilla swing command.

use std::slice;

use steel_registry::data_components::vanilla_components::SwingAnimation;
use steel_utils::{Identifier, translations, types::InteractionHand};
use text_components::TextComponent;

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::entity::SharedEntity;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("swing"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("swing").executes(swing_source_main_hand).then(
        argument("targets", SteelArgumentType::entities())
            .executes(swing_targets_main_hand)
            .then(hand_swing("mainhand", InteractionHand::MainHand))
            .then(hand_swing("offhand", InteractionHand::OffHand)),
    )
}

/// Builds one vanilla `handSwing` branch for a single interaction hand.
fn hand_swing(
    name: &str,
    hand: InteractionHand,
) -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal(name)
        .executes(move |context| swing_targets(context, hand, SwingAnimation::DEFAULT))
        .then(
            argument("animation", SteelArgumentType::swing_animation())
                .executes(move |context| {
                    let animation_type = context.swing_animation("animation")?;
                    swing_targets(
                        context,
                        hand,
                        SwingAnimation::new(animation_type, SwingAnimation::DEFAULT.duration),
                    )
                })
                .then(
                    argument("duration", SteelArgumentType::time(1)).executes(move |context| {
                        let animation_type = context.swing_animation("animation")?;
                        let duration = context.time("duration")?;
                        swing_targets(context, hand, SwingAnimation::new(animation_type, duration))
                    }),
                ),
        )
}

fn swing_source_main_hand(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let Some(entity) = context.source().entity() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::PERMISSIONS_REQUIRES_ENTITY,
        )));
    };
    swing_entities(
        context,
        slice::from_ref(entity),
        InteractionHand::MainHand,
        SwingAnimation::DEFAULT,
    )
}

fn swing_targets_main_hand(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    swing_entities(
        context,
        &targets,
        InteractionHand::MainHand,
        SwingAnimation::DEFAULT,
    )
}

fn swing_targets(
    context: &SteelCommandContext<CommandSource>,
    hand: InteractionHand,
    animation: SwingAnimation,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.entities("targets")?;
    swing_entities(context, &targets, hand, animation)
}

/// Swings every living target and reports vanilla's single/multiple feedback.
fn swing_entities(
    context: &SteelCommandContext<CommandSource>,
    targets: &[SharedEntity],
    hand: InteractionHand,
    animation: SwingAnimation,
) -> Result<i32, CommandSyntaxError> {
    let mut swinging = Vec::new();
    for target in targets {
        if let Some(living) = target.as_living_entity() {
            living.swing(hand, animation, true);
            swinging.push(target);
        }
    }

    if swinging.is_empty() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_SWING_FAILED_NOTLIVING
                .msg()
                .component(),
        ));
    }

    let message = if let [target] = swinging.as_slice() {
        translations::COMMANDS_SWING_SUCCESS_SINGLE
            .message([target.display_name()])
            .component()
    } else {
        translations::COMMANDS_SWING_SUCCESS_MULTIPLE
            .message([TextComponent::from(swinging.len().to_string())])
            .component()
    };
    context.source().send_success(&message, true);

    i32::try_from(swinging.len()).map_err(|_| {
        CommandSyntaxError::dynamic("Swung entity count exceeds the command result range")
    })
}
