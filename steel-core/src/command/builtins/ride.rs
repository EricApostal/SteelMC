//! Vanilla per-entity riding command.
//!
//! One deliberate difference from vanilla 26.3: this command checks same
//! dimension by comparing the source and target world's domain strings, which
//! is how Steel documents world identity. Vanilla compares level instances.

use std::sync::Arc;

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
use crate::entity::SharedEntity;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("ride"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("ride").then(
        argument("target", SteelArgumentType::entity())
            .then(
                literal("mount")
                    .then(argument("vehicle", SteelArgumentType::entity()).executes(mount)),
            )
            // `dismount` is a sibling of `mount`, both direct children of the
            // `target` argument.
            .then(literal("dismount").executes(dismount)),
    )
}

fn ensure_same_dimension(
    target: &SharedEntity,
    vehicle: &SharedEntity,
) -> Result<(), CommandSyntaxError> {
    let target_world = target.level();
    let vehicle_world = vehicle.level();
    let same_world = match (&target_world, &vehicle_world) {
        (Some(target_world), Some(vehicle_world)) => {
            Arc::ptr_eq(target_world, vehicle_world)
                || target_world.domain() == vehicle_world.domain()
        }
        (None, None) => true,
        _ => false,
    };
    if same_world {
        return Ok(());
    }
    Err(CommandSyntaxError::dynamic(TextComponent::from(
        &translations::COMMANDS_RIDE_MOUNT_FAILURE_WRONG_DIMENSION,
    )))
}

/// Starts `target` riding `vehicle`.
fn mount(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    if let Some(current) = target.vehicle() {
        let message = translations::COMMANDS_RIDE_ALREADY_RIDING
            .message([target.display_name(), current.display_name()])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }
    let vehicle = context.entity("vehicle")?;
    if vehicle.as_player().is_some() {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_RIDE_MOUNT_FAILURE_CANT_RIDE_PLAYERS,
        )));
    }
    if is_self_or_passenger(&target, &vehicle) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_RIDE_MOUNT_FAILURE_LOOP,
        )));
    }
    ensure_same_dimension(&target, &vehicle)?;
    // Vanilla `Entity.startRiding(vehicle, force=true, ...)`; the `Entity`
    // impl for players also syncs the client passenger list.
    if !target.start_riding(&vehicle) {
        let message = translations::COMMANDS_RIDE_MOUNT_FAILURE_GENERIC
            .message([target.display_name(), vehicle.display_name()])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }
    let message = translations::COMMANDS_RIDE_MOUNT_SUCCESS
        .message([target.display_name(), vehicle.display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Stops `target` riding its current vehicle.
fn dismount(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let target = context.entity("target")?;
    let Some(vehicle) = target.vehicle() else {
        let message = translations::COMMANDS_RIDE_NOT_RIDING
            .message([target.display_name()])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    };
    target.stop_riding();
    let message = translations::COMMANDS_RIDE_DISMOUNT_SUCCESS
        .message([target.display_name(), vehicle.display_name()])
        .component();
    context.source().send_success(&message, true);
    Ok(1)
}

/// Matches vanilla's `Entity.getSelfAndPassengers().anyMatch(vehicle)`: whether
/// `target` equals `vehicle` or carries it as any direct or indirect passenger.
fn is_self_or_passenger(target: &SharedEntity, vehicle: &SharedEntity) -> bool {
    fn contains(entity: &SharedEntity, wanted: &SharedEntity, depth: usize) -> bool {
        if entity.id() == wanted.id() {
            return true;
        }
        // Depth cap mirrors the entity-cycle walk in
        // `start_riding_entities`; depth beyond this cannot occur without a
        // corrupted relationship graph.
        if depth > 64 {
            return false;
        }
        entity
            .passengers()
            .iter()
            .any(|passenger| contains(passenger, wanted, depth + 1))
    }
    contains(target, vehicle, 0)
}

#[cfg(test)]
mod tests {
    use super::super::create_dispatcher;
    use crate::command::{
        brigadier::{CommandDispatcher, CommandNode, NodeId},
        execution::{CommandSource, SteelArgumentType, SteelCommandRuntime},
    };
    use steel_registry::init_vanilla_registry;

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

    fn has_child(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> bool {
        dispatcher.children(parent).is_some_and(|children| {
            children.iter().any(|child| {
                dispatcher
                    .node(*child)
                    .is_some_and(|node| node.name() == name)
            })
        })
    }

    #[test]
    fn ride_dismount_is_a_sibling_of_mount() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };
        let root = child(&dispatcher, dispatcher.root(), "ride");
        let target = child(&dispatcher, root, "target");
        assert_eq!(
            dispatcher
                .node(target)
                .and_then(|node| node.argument_type()),
            Some(&SteelArgumentType::entity())
        );

        let mount = child(&dispatcher, target, "mount");
        let dismount = child(&dispatcher, target, "dismount");
        assert!(!has_child(&dispatcher, mount, "dismount"));
        assert!(
            dispatcher
                .node(dismount)
                .is_some_and(CommandNode::is_executable)
        );
        assert!(
            dispatcher
                .node(child(&dispatcher, mount, "vehicle"))
                .is_some_and(CommandNode::is_executable)
        );
    }
}
