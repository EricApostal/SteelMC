//! Vanilla rotate command.

use steel_utils::{Identifier, translations};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::entity::{EntityAnchor, SharedEntity};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("rotate"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("rotate").then(
        argument("target", SteelArgumentType::entity())
            .then(argument("rotation", SteelArgumentType::rotation()).executes(rotate_to_rotation))
            .then(
                literal("facing")
                    .then(
                        literal("entity").then(
                            argument("facingEntity", SteelArgumentType::entity())
                                .executes(rotate_facing_entity_feet)
                                .then(
                                    argument("facingAnchor", SteelArgumentType::entity_anchor())
                                        .executes(rotate_facing_entity_anchor),
                                ),
                        ),
                    )
                    .then(
                        argument("facingLocation", SteelArgumentType::vec3(true))
                            .executes(rotate_facing_location),
                    ),
            ),
    )
}

fn rotate_to_rotation(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let rotation = context.coordinates("rotation")?;

    // `Coordinates::rotation` already resolves each component against the source rotation,
    // which is the net result of vanilla's `RotationArgument` resolution followed by
    // `Entity.forceSetRotation`'s delta-then-re-add on the relative axes.
    let (yaw, pitch) = rotation.rotation(context.source());
    force_set_rotation(&target, yaw, pitch);
    send_rotate_success(context.source(), &target);
    Ok(1)
}

fn rotate_facing_entity_feet(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    rotate_facing_entity(context, EntityAnchor::Feet)
}

fn rotate_facing_entity_anchor(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let anchor = context.entity_anchor("facingAnchor")?;
    rotate_facing_entity(context, anchor)
}

fn rotate_facing_entity(
    context: &SteelCommandContext<CommandSource>,
    anchor: EntityAnchor,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let facing_entity = context.entity("facingEntity")?;

    // `Player::look_at_entity` overrides this and sends vanilla's extra look-at packet,
    // matching `ServerPlayer.lookAt` inside vanilla `LookAt.LookAtEntity`.
    target.look_at_entity(context.source().anchor(), facing_entity.as_ref(), anchor);
    send_rotate_success(context.source(), &target);
    Ok(1)
}

fn rotate_facing_location(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let location = context
        .coordinates("facingLocation")?
        .position(context.source());

    target.look_at(context.source().anchor(), location);
    send_rotate_success(context.source(), &target);
    Ok(1)
}

/// Mirrors vanilla `Entity.forceSetRotation` with absolute angles.
///
/// Vanilla sets the body rotation, copies the new yaw into the head yaw, and refreshes the
/// old-rotation snapshot used for interpolation.
fn force_set_rotation(target: &SharedEntity, yaw: f32, pitch: f32) {
    target.set_rotation((yaw, pitch));
    if let Some(living) = target.as_living_entity() {
        living.set_y_head_rot(yaw);
    }
    target.base().set_old_rotation_to_current();
}

fn send_rotate_success(source: &CommandSource, target: &SharedEntity) {
    source.send_success(
        &translations::COMMANDS_ROTATE_SUCCESS
            .message([target.display_name()])
            .component(),
        true,
    );
}
