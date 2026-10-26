//! Vanilla `/waypoint` command.
//!
//! Mirrors `WaypointCommand`: `list` reports the level's waypoint transmitters and
//! `modify <waypoint> color|style ...` rewrites one transmitter's `Waypoint.Icon`.
//! Vanilla keeps the icon on the transmitting entity (`LivingEntity.locatorBarIcon`)
//! and applies it through `Waypoint.Icon.cloneAndAssignStyle`, which fills in the
//! transmitter's team color when it carries no explicit tint.
//!
//! Steel has no `waypoint_style_asset` registry: vanilla loads it client-side from
//! `assets/<namespace>/waypoint_style/*.json`, while the server only ever builds a
//! `ResourceKey` from the parsed identifier, so `style set` accepts the same
//! unvalidated identifier vanilla accepts.

use std::sync::Arc;

use steel_protocol::packets::game::WaypointIcon;
use steel_utils::{
    Identifier, RgbColor,
    translations::{
        ARGUMENT_WAYPOINT_INVALID, CHAT_COORDINATES_TOOLTIP, COMMANDS_WAYPOINT_LIST_EMPTY,
        COMMANDS_WAYPOINT_LIST_SUCCESS, COMMANDS_WAYPOINT_MODIFY_COLOR,
        COMMANDS_WAYPOINT_MODIFY_COLOR_RESET, COMMANDS_WAYPOINT_MODIFY_STYLE,
    },
};
use text_components::{
    Modifier, TextComponent,
    format::Color,
    interactivity::{ClickEvent, HoverEvent},
};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::{
    entity::Entity as _,
    permission::PermissionLevel,
    player::{Player, waypoint::level_transmitters},
};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("waypoint"), |_| command())
        .at_level(PermissionLevel::Gamemasters)
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("waypoint")
        .then(literal("list").executes(list_waypoints))
        .then(
            literal("modify").then(
                argument("waypoint", SteelArgumentType::entity())
                    .then(
                        literal("color")
                            .then(
                                argument("color", SteelArgumentType::team_color())
                                    .executes(set_color_team),
                            )
                            .then(
                                literal("hex").then(
                                    argument("color", SteelArgumentType::hex_color())
                                        .executes(set_color_hex),
                                ),
                            )
                            .then(literal("reset").executes(reset_color)),
                    )
                    .then(
                        literal("style")
                            .then(literal("reset").executes(set_style_reset))
                            .then(
                                literal("set").then(
                                    argument("style", SteelArgumentType::identifier_argument())
                                        .executes(set_style),
                                ),
                            ),
                    ),
            ),
        )
}

/// Vanilla `WaypointCommand.listWaypoints`.
fn list_waypoints(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let world = Arc::clone(context.source().world());
    let dimension = world.key.to_string();
    let transmitters = level_transmitters(&world);
    let Ok(count) = i32::try_from(transmitters.len()) else {
        return Err(CommandSyntaxError::dynamic(
            "Waypoint count exceeds the command result range",
        ));
    };

    if transmitters.is_empty() {
        let message = COMMANDS_WAYPOINT_LIST_EMPTY
            .message([dimension])
            .component();
        context.source().send_success(&message, false);
        return Ok(0);
    }

    let entries = transmitters
        .iter()
        .map(|transmitter| waypoint_entry(transmitter, &dimension))
        .collect();
    let message = COMMANDS_WAYPOINT_LIST_SUCCESS
        .message([
            TextComponent::from(count.to_string()),
            TextComponent::from(dimension),
            format_list(entries),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(count)
}

/// One `commands.waypoint.list.success` entry (vanilla `lambda$listWaypoints$1`):
/// the transmitter's display name tinted with its icon color, clicking through to
/// a teleport suggestion for its block position.
fn waypoint_entry(transmitter: &Player, dimension: &str) -> TextComponent {
    let position = transmitter.block_position();
    transmitter
        .display_name()
        .click_event(ClickEvent::suggest_command(format!(
            "/execute in {dimension} run tp @s {} {} {}",
            position.x(),
            position.y(),
            position.z()
        )))
        .hover_event(HoverEvent::show_text(TextComponent::from(
            &CHAT_COORDINATES_TOOLTIP,
        )))
        .color(icon_color(transmitter.waypoint_icon().color))
}

/// Vanilla `WaypointArgument.getWaypoint`: exactly one entity that transmits a
/// locator-bar waypoint. Only players implement `WaypointTransmitter` in Steel.
#[expect(dead_code, reason = "vanilla WaypointArgument.getWaypoint API surface")]
fn waypoint_target(context: &SteelCommandContext<CommandSource>) -> Result<(), CommandSyntaxError> {
    let target = context.entity("waypoint")?;
    if target.as_player().is_none() {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &ARGUMENT_WAYPOINT_INVALID,
        )));
    }
    Ok(())
}

/// Vanilla `WaypointCommand.setWaypointColor(TeamColor)`.
fn set_color_team(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let color = context.team_color("color")?;
    modify_icon(context, |icon| {
        icon.color = Some(RgbColor::new(color.rgb()));
    })?;

    let message = COMMANDS_WAYPOINT_MODIFY_COLOR
        .message([TextComponent::plain(color.as_str()).color(color.text_color())])
        .component();
    context.source().send_success(&message, false);
    Ok(0)
}

/// Vanilla `WaypointCommand.setWaypointColor(Integer)` for `color hex`.
fn set_color_hex(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let argb = context.hex_color("color")?;
    let tint = RgbColor::new(argb);
    modify_icon(context, |icon| icon.color = Some(tint))?;

    let message = COMMANDS_WAYPOINT_MODIFY_COLOR
        .message([
            TextComponent::plain(format!("{:06X}", (argb & 0x00FF_FFFF) as u32))
                .color(icon_color(Some(tint))),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(0)
}

/// Vanilla `WaypointCommand.resetWaypointColor`.
fn reset_color(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    modify_icon(context, |icon| icon.color = None)?;
    context.source().send_success(
        &COMMANDS_WAYPOINT_MODIFY_COLOR_RESET.msg().component(),
        false,
    );
    Ok(0)
}

/// Vanilla `WaypointCommand.setWaypointStyle` with `WaypointStyleAssets.DEFAULT`.
fn set_style_reset(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    set_style_identifier(context, Identifier::vanilla_static("default"))
}

/// Vanilla `WaypointCommand.setWaypointStyle`.
fn set_style(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let style = context.identifier_argument("style")?.clone();
    set_style_identifier(context, style)
}

fn set_style_identifier(
    context: &SteelCommandContext<CommandSource>,
    style: Identifier,
) -> Result<i32, CommandSyntaxError> {
    modify_icon(context, |icon| icon.style = style.clone())?;
    context
        .source()
        .send_success(&COMMANDS_WAYPOINT_MODIFY_STYLE.msg().component(), false);
    Ok(0)
}

/// Vanilla `WaypointCommand.mutateIcon`: untrack the chosen transmitter from every
/// receiver, apply `mutate` to its icon, then re-track it.
fn modify_icon(
    context: &SteelCommandContext<CommandSource>,
    mutate: impl FnOnce(&mut WaypointIcon),
) -> Result<(), CommandSyntaxError> {
    let target = context.entity("waypoint")?;
    let Some(transmitter) = target.as_player() else {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &ARGUMENT_WAYPOINT_INVALID,
        )));
    };
    transmitter.mutate_waypoint_icon(mutate);
    Ok(())
}

/// Vanilla `Style.withColor(int)`: the tint's red, green and blue channels, with
/// vanilla's `-1` "no colour" default rendering as white.
fn icon_color(tint: Option<RgbColor>) -> Color {
    tint.map_or(Color::Rgb(0xFF, 0xFF, 0xFF), |tint| {
        Color::Rgb(tint.red(), tint.green(), tint.blue())
    })
}

/// Vanilla `ComponentUtils.formatList`: gray `, ` separators between entries.
fn format_list(entries: Vec<TextComponent>) -> TextComponent {
    let mut list = TextComponent::plain("");
    for (index, entry) in entries.into_iter().enumerate() {
        if index > 0 {
            list = list.add_child(TextComponent::plain(", ").color(Color::Gray));
        }
        list = list.add_child(entry);
    }
    list
}

#[cfg(test)]
mod tests {
    use super::{format_list, icon_color};
    use crate::command::brigadier::{CommandDispatcher, NodeId};
    use crate::command::builtins::create_dispatcher;
    use crate::command::execution::{CommandSource, SteelCommandRuntime};
    use steel_registry::init_vanilla_registry;
    use steel_utils::RgbColor;
    use text_components::{Modifier as _, TextComponent, format::Color};

    type Dispatcher = CommandDispatcher<CommandSource, SteelCommandRuntime>;

    fn child(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> NodeId {
        let Some(child) = dispatcher.children(parent).and_then(|children| {
            children.iter().copied().find(|child| {
                dispatcher
                    .node(*child)
                    .is_some_and(|node| node.name() == name)
            })
        }) else {
            panic!("missing command node '{name}'");
        };
        child
    }

    /// The command surface matches `WaypointCommand.register`'s graph.
    #[test]
    fn waypoint_exposes_list_and_modify_with_vanilla_leaves() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in dispatcher should build");
        };
        let waypoint = child(&dispatcher, dispatcher.root(), "waypoint");
        child(&dispatcher, waypoint, "list");
        let modify = child(&dispatcher, waypoint, "modify");
        let target = child(&dispatcher, modify, "waypoint");
        let color = child(&dispatcher, target, "color");
        child(&dispatcher, color, "color");
        child(&dispatcher, child(&dispatcher, color, "hex"), "color");
        child(&dispatcher, color, "reset");
        let style = child(&dispatcher, target, "style");
        child(&dispatcher, style, "reset");
        child(&dispatcher, child(&dispatcher, style, "set"), "style");
    }

    #[test]
    fn icon_colors_follow_vanilla_rgb_channel_order() {
        assert_eq!(icon_color(None), Color::Rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(
            icon_color(Some(RgbColor::new(0x30_3030))),
            Color::Rgb(0x30, 0x30, 0x30)
        );
        assert_eq!(
            icon_color(Some(RgbColor::new(0x00FF_5555))),
            Color::Rgb(0xFF, 0x55, 0x55)
        );
        // `color hex` carries an opaque ARGB value; only the RGB channels apply.
        assert_eq!(
            icon_color(Some(RgbColor::new(0xFF00_AA00u32 as i32))),
            Color::Rgb(0x00, 0xAA, 0x00)
        );
    }

    #[test]
    fn list_entries_are_joined_with_a_gray_separator() {
        let joined = format_list(vec![
            TextComponent::plain("Alice"),
            TextComponent::plain("Bob"),
        ]);
        assert_eq!(
            joined.children,
            [
                TextComponent::plain("Alice"),
                TextComponent::plain(", ").color(Color::Gray),
                TextComponent::plain("Bob"),
            ]
        );
        assert_eq!(format_list(Vec::new()).children, [] as [TextComponent; 0]);
    }
}
