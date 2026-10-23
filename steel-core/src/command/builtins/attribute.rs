//! Vanilla entity attribute command.

use steel_registry::attribute::AttributeRef;
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
use crate::entity::{
    LivingEntity, SharedEntity,
    attribute::{AttributeModifier, AttributeModifierOperation},
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("attribute"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("attribute").then(
        argument("target", SteelArgumentType::entity()).then(
            argument("attribute", SteelArgumentType::attribute())
                .then(
                    literal("get").executes(get_value_unscaled).then(
                        argument("scale", ArgumentType::double(f64::MIN, f64::MAX))
                            .executes(get_value_scaled),
                    ),
                )
                .then(
                    literal("base")
                        .then(
                            literal("set").then(
                                argument("value", ArgumentType::double(f64::MIN, f64::MAX))
                                    .executes(set_base),
                            ),
                        )
                        .then(
                            literal("get").executes(get_base_unscaled).then(
                                argument("scale", ArgumentType::double(f64::MIN, f64::MAX))
                                    .executes(get_base_scaled),
                            ),
                        )
                        .then(literal("reset").executes(reset_base)),
                )
                .then(
                    literal("modifier")
                        .then(
                            literal("add").then(
                                argument("id", SteelArgumentType::identifier_argument()).then(
                                    argument("value", ArgumentType::double(f64::MIN, f64::MAX))
                                        .then(literal("add_value").executes(add_value_modifier))
                                        .then(
                                            literal("add_multiplied_base")
                                                .executes(add_multiplied_base_modifier),
                                        )
                                        .then(
                                            literal("add_multiplied_total")
                                                .executes(add_multiplied_total_modifier),
                                        ),
                                ),
                            ),
                        )
                        .then(
                            literal("remove").then(
                                argument("id", SteelArgumentType::identifier_argument())
                                    .executes(remove_modifier),
                            ),
                        )
                        .then(
                            literal("value").then(
                                literal("get").then(
                                    argument("id", SteelArgumentType::identifier_argument())
                                        .executes(get_modifier_value_unscaled)
                                        .then(
                                            argument(
                                                "scale",
                                                ArgumentType::double(f64::MIN, f64::MAX),
                                            )
                                            .executes(get_modifier_value_scaled),
                                        ),
                                ),
                            ),
                        ),
                ),
        ),
    )
}

/// Resolves the target as a living entity, matching vanilla's
/// `AttributeCommand.getLivingEntity`.
fn require_living(target: &SharedEntity) -> Result<&dyn LivingEntity, CommandSyntaxError> {
    target.as_living_entity().ok_or_else(|| {
        let message = translations::COMMANDS_ATTRIBUTE_FAILED_ENTITY
            .message([target.display_name()])
            .component();
        CommandSyntaxError::dynamic(message)
    })
}

/// Resolves the attribute argument and requires the target to have it.
fn required_attribute(
    context: &SteelCommandContext<CommandSource>,
    target: &SharedEntity,
    living: &dyn LivingEntity,
) -> Result<AttributeRef, CommandSyntaxError> {
    let attribute = context.attribute("attribute")?;
    if !living.attributes().lock().has_attribute(attribute) {
        return Err(no_such_attribute_error(target, attribute));
    }
    Ok(attribute)
}

fn attribute_description(attribute: AttributeRef) -> TextComponent {
    TextComponent::translated(TranslatedMessage::new(attribute.translation_key, None))
}

fn no_such_attribute_error(target: &SharedEntity, attribute: AttributeRef) -> CommandSyntaxError {
    let message = translations::COMMANDS_ATTRIBUTE_FAILED_NO_ATTRIBUTE
        .message([target.display_name(), attribute_description(attribute)])
        .component();
    CommandSyntaxError::dynamic(message)
}

fn no_such_modifier_error(
    target: &SharedEntity,
    attribute: AttributeRef,
    modifier_id: &Identifier,
) -> CommandSyntaxError {
    // Vanilla passes `(target, attribute, id)` into a lambda that emits
    // `(attribute, target, modifier)`; this keeps the vanilla argument order.
    let message = translations::COMMANDS_ATTRIBUTE_FAILED_NO_MODIFIER
        .message([
            attribute_description(attribute),
            target.display_name(),
            modifier_name(modifier_id),
        ])
        .component();
    CommandSyntaxError::dynamic(message)
}

/// Vanilla `Component.translationArg(Identifier)` renders the resource
/// location literally.
fn modifier_name(modifier_id: &Identifier) -> TextComponent {
    TextComponent::plain(modifier_id.to_string())
}

fn scale(context: &SteelCommandContext<CommandSource>) -> Result<f64, CommandSyntaxError> {
    context.double("scale")
}

/// Vanilla's `(int) (value * scale)` command result truncation.
fn scaled_result(value: f64, scale: f64) -> i32 {
    (value * scale) as i32
}

fn get_value_unscaled(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    get_value(context, 1.0)
}

fn get_value_scaled(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    get_value(context, scale(context)?)
}

fn get_value(
    context: &SteelCommandContext<CommandSource>,
    scale: f64,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let living = require_living(&target)?;
    let attribute = required_attribute(context, &target, living)?;
    let value = living
        .attributes()
        .lock()
        .get_value(attribute)
        .ok_or_else(|| no_such_attribute_error(&target, attribute))?;
    let message = translations::COMMANDS_ATTRIBUTE_VALUE_GET_SUCCESS
        .message([
            attribute_description(attribute),
            target.display_name(),
            TextComponent::plain(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(scaled_result(value, scale))
}

fn get_base_unscaled(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    get_base(context, 1.0)
}

fn get_base_scaled(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    get_base(context, scale(context)?)
}

fn get_base(
    context: &SteelCommandContext<CommandSource>,
    scale: f64,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let living = require_living(&target)?;
    let attribute = required_attribute(context, &target, living)?;
    let value = living
        .attributes()
        .lock()
        .get_base_value(attribute)
        .ok_or_else(|| no_such_attribute_error(&target, attribute))?;
    let message = translations::COMMANDS_ATTRIBUTE_BASE_VALUE_GET_SUCCESS
        .message([
            attribute_description(attribute),
            target.display_name(),
            TextComponent::plain(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(scaled_result(value, scale))
}

fn set_base(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let value = context.double("value")?;
    let target = context.entity("target")?;
    let living = require_living(&target)?;
    let attribute = required_attribute(context, &target, living)?;
    living.attributes().lock().set_base_value(attribute, value);
    let message = translations::COMMANDS_ATTRIBUTE_BASE_VALUE_SET_SUCCESS
        .message([
            attribute_description(attribute),
            target.display_name(),
            TextComponent::plain(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(1)
}

fn reset_base(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let living = require_living(&target)?;
    let attribute = required_attribute(context, &target, living)?;
    // Vanilla `AttributeMap.resetBaseValue` restores the registered default.
    living
        .attributes()
        .lock()
        .set_base_value(attribute, attribute.default_value);
    let value = living
        .attributes()
        .lock()
        .get_base_value(attribute)
        .ok_or_else(|| no_such_attribute_error(&target, attribute))?;
    let message = translations::COMMANDS_ATTRIBUTE_BASE_VALUE_RESET_SUCCESS
        .message([
            attribute_description(attribute),
            target.display_name(),
            TextComponent::plain(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(1)
}

fn add_value_modifier(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    add_modifier(context, AttributeModifierOperation::AddValue)
}

fn add_multiplied_base_modifier(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    add_modifier(context, AttributeModifierOperation::AddMultipliedBase)
}

fn add_multiplied_total_modifier(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    add_modifier(context, AttributeModifierOperation::AddMultipliedTotal)
}

fn add_modifier(
    context: &SteelCommandContext<CommandSource>,
    operation: AttributeModifierOperation,
) -> Result<i32, CommandSyntaxError> {
    let value = context.double("value")?;
    let target = context.entity("target")?;
    let living = require_living(&target)?;
    let attribute = required_attribute(context, &target, living)?;
    let modifier_id = context.identifier_argument("id")?.clone();
    let modifier = AttributeModifier {
        id: modifier_id.clone(),
        amount: value,
        operation,
    };
    if !living
        .attributes()
        .lock()
        .add_modifier(attribute, modifier, true)
    {
        let message = translations::COMMANDS_ATTRIBUTE_FAILED_MODIFIER_ALREADY_PRESENT
            .message([
                modifier_name(&modifier_id),
                attribute_description(attribute),
                target.display_name(),
            ])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }
    let message = translations::COMMANDS_ATTRIBUTE_MODIFIER_ADD_SUCCESS
        .message([
            modifier_name(&modifier_id),
            attribute_description(attribute),
            target.display_name(),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(1)
}

fn remove_modifier(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let living = require_living(&target)?;
    let attribute = required_attribute(context, &target, living)?;
    let modifier_id = context.identifier_argument("id")?.clone();
    if !living
        .attributes()
        .lock()
        .remove_modifier(attribute, &modifier_id)
    {
        return Err(no_such_modifier_error(&target, attribute, &modifier_id));
    }
    let message = translations::COMMANDS_ATTRIBUTE_MODIFIER_REMOVE_SUCCESS
        .message([
            modifier_name(&modifier_id),
            attribute_description(attribute),
            target.display_name(),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(1)
}

fn get_modifier_value_unscaled(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    get_modifier_value(context, 1.0)
}

fn get_modifier_value_scaled(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    get_modifier_value(context, scale(context)?)
}

fn get_modifier_value(
    context: &SteelCommandContext<CommandSource>,
    scale: f64,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let living = require_living(&target)?;
    let attribute = required_attribute(context, &target, living)?;
    let modifier_id = context.identifier_argument("id")?.clone();
    let value = living
        .attributes()
        .lock()
        .modifier_value(attribute, &modifier_id)
        .ok_or_else(|| no_such_modifier_error(&target, attribute, &modifier_id))?;
    let message = translations::COMMANDS_ATTRIBUTE_MODIFIER_VALUE_GET_SUCCESS
        .message([
            modifier_name(&modifier_id),
            attribute_description(attribute),
            target.display_name(),
            TextComponent::plain(value.to_string()),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(scaled_result(value, scale))
}
