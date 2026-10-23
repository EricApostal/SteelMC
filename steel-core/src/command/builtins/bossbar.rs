//! Vanilla `bossbar` command.
//!
//! Mirrors `BossBarCommands`: the `add`/`remove`/`list`/`set`/`get` tree over
//! [`CustomBossEvents`], including vanilla's unchanged-state failures, result
//! values and feedback keys. Feedback renders
//! [`CustomBossEvent::display_name`], vanilla's `CustomBossEvent.getDisplayName`.

use std::sync::Arc;

use parking_lot::MutexGuard;
use steel_utils::{Identifier, translations};
use text_components::{Modifier as _, TextComponent, format::Color};

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError, SuggestionsBuilder},
    execution::{
        CommandSource, CommandTextResolver, SteelArgumentType, SteelCommandContext,
        SteelCommandRuntime, SteelSuggestionContext, argument, literal, suggest_list,
    },
    registration::CommandRegistration,
};
use crate::entity::Entity as _;
use crate::entity::boss::{CustomBossEvent, CustomBossEvents};
use crate::player::Player;
use steel_protocol::packets::game::{BossBarColor, BossBarOverlay};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("bossbar"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("bossbar")
        .then(
            literal("add").then(
                argument("id", SteelArgumentType::identifier_argument())
                    .then(argument("name", SteelArgumentType::component()).executes(create_bar)),
            ),
        )
        .then(literal("remove").then(existing_bar_argument().executes(remove_bar)))
        .then(literal("list").executes(list_bars))
        .then(
            literal("set").then(
                existing_bar_argument()
                    .then(
                        literal("name").then(
                            argument("name", SteelArgumentType::component()).executes(set_name),
                        ),
                    )
                    .then(
                        literal("color")
                            .then(literal("pink").executes(|c| set_color(c, BossBarColor::Pink)))
                            .then(literal("blue").executes(|c| set_color(c, BossBarColor::Blue)))
                            .then(literal("red").executes(|c| set_color(c, BossBarColor::Red)))
                            .then(literal("green").executes(|c| set_color(c, BossBarColor::Green)))
                            .then(
                                literal("yellow").executes(|c| set_color(c, BossBarColor::Yellow)),
                            )
                            .then(
                                literal("purple").executes(|c| set_color(c, BossBarColor::Purple)),
                            )
                            .then(literal("white").executes(|c| set_color(c, BossBarColor::White))),
                    )
                    .then(
                        literal("style")
                            .then(
                                literal("progress")
                                    .executes(|c| set_style(c, BossBarOverlay::Progress)),
                            )
                            .then(
                                literal("notched_6")
                                    .executes(|c| set_style(c, BossBarOverlay::Notched6)),
                            )
                            .then(
                                literal("notched_10")
                                    .executes(|c| set_style(c, BossBarOverlay::Notched10)),
                            )
                            .then(
                                literal("notched_12")
                                    .executes(|c| set_style(c, BossBarOverlay::Notched12)),
                            )
                            .then(
                                literal("notched_20")
                                    .executes(|c| set_style(c, BossBarOverlay::Notched20)),
                            ),
                    )
                    .then(literal("value").then(
                        argument("value", ArgumentType::integer(0, i32::MAX)).executes(set_value),
                    ))
                    .then(literal("max").then(
                        argument("max", ArgumentType::integer(1, i32::MAX)).executes(set_max),
                    ))
                    .then(
                        literal("visible")
                            .then(argument("visible", ArgumentType::bool()).executes(set_visible)),
                    )
                    .then(
                        literal("players").executes(|c| set_players(c, &[])).then(
                            argument("targets", SteelArgumentType::players())
                                .executes(set_players_from_targets),
                        ),
                    ),
            ),
        )
        .then(
            literal("get").then(
                existing_bar_argument()
                    .then(literal("value").executes(get_value))
                    .then(literal("max").executes(get_max))
                    .then(literal("visible").executes(get_visible))
                    .then(literal("players").executes(get_players)),
            ),
        )
}

/// Vanilla's `id` argument node: an unvalidated resource location that
/// suggests the ids of the currently defined custom boss bars.
fn existing_bar_argument() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    argument("id", SteelArgumentType::identifier_argument()).suggests(
        |context: &SteelSuggestionContext<'_, CommandSource>,
         builder: &mut SuggestionsBuilder<'_>| {
            let ids = lock_events(context.source()).ids();
            suggest_list(builder, ids.iter().map(ToString::to_string));
        },
    )
}

fn lock_events(source: &CommandSource) -> MutexGuard<'_, CustomBossEvents> {
    source.server().custom_boss_events.lock()
}

/// Vanilla `BossBarCommands.getBossBar`.
fn existing_bar<'a>(
    events: &'a CustomBossEvents,
    id: &Identifier,
) -> Result<&'a CustomBossEvent, CommandSyntaxError> {
    let Some(bar) = events.get(id) else {
        return Err(unknown_bar(id));
    };
    Ok(bar)
}

/// Vanilla `BossBarCommands.getBossBar`, for the mutating subcommands.
fn existing_bar_mut<'a>(
    events: &'a mut CustomBossEvents,
    id: &Identifier,
) -> Result<&'a mut CustomBossEvent, CommandSyntaxError> {
    if events.get(id).is_none() {
        return Err(unknown_bar(id));
    }
    let Some(bar) = events.get_mut(id) else {
        return Err(unknown_bar(id));
    };
    Ok(bar)
}

fn unknown_bar(id: &Identifier) -> CommandSyntaxError {
    CommandSyntaxError::dynamic(
        translations::COMMANDS_BOSSBAR_UNKNOWN
            .message([id.to_string()])
            .component(),
    )
}

/// Vanilla `ComponentUtils.formatList`: values joined by a gray `", "`.
fn format_list(values: impl IntoIterator<Item = TextComponent>) -> TextComponent {
    let mut list = TextComponent::plain("");
    for (index, value) in values.into_iter().enumerate() {
        if index > 0 {
            list = list.add_child(TextComponent::plain(", ").color(Color::Gray));
        }
        list = list.add_child(value);
    }
    list
}

fn event_count(events: &CustomBossEvents) -> Result<i32, CommandSyntaxError> {
    i32::try_from(events.events().count()).map_err(|_| {
        CommandSyntaxError::dynamic("Custom boss bar count exceeds the command result range")
    })
}

fn player_count(bar: &CustomBossEvent) -> Result<i32, CommandSyntaxError> {
    i32::try_from(bar.player_uuids().len()).map_err(|_| {
        CommandSyntaxError::dynamic("Custom boss bar player count exceeds the command result range")
    })
}

/// Vanilla `BossBarCommands.createBar`.
fn create_bar(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let name = context
        .text_component("name")?
        .try_resolve(&CommandTextResolver::with_source(context.source()))?;

    let mut events = lock_events(context.source());
    if events.get(id).is_some() {
        return Err(CommandSyntaxError::dynamic(
            translations::COMMANDS_BOSSBAR_CREATE_FAILED
                .message([id.to_string()])
                .component(),
        ));
    }

    let bar = events.create(id);
    bar.set_name(name);
    let message = translations::COMMANDS_BOSSBAR_CREATE_SUCCESS
        .message([bar.display_name()])
        .component();
    let result = event_count(&events)?;
    drop(events);

    context.source().send_success(&message, true);
    Ok(result)
}

/// Vanilla `BossBarCommands.removeBar`.
fn remove_bar(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    bar.clear_players();
    let message = translations::COMMANDS_BOSSBAR_REMOVE_SUCCESS
        .message([bar.display_name()])
        .component();
    drop(events.remove(id));
    let result = event_count(&events)?;
    drop(events);

    context.source().send_success(&message, true);
    Ok(result)
}

/// Vanilla `BossBarCommands.listBars`.
fn list_bars(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let events = lock_events(context.source());
    let bars: Vec<TextComponent> = events.events().map(CustomBossEvent::display_name).collect();
    let message = if bars.is_empty() {
        TextComponent::from(&translations::COMMANDS_BOSSBAR_LIST_BARS_NONE)
    } else {
        translations::COMMANDS_BOSSBAR_LIST_BARS_SOME
            .message([
                TextComponent::from(bars.len().to_string()),
                format_list(bars),
            ])
            .component()
    };
    let result = event_count(&events)?;
    drop(events);

    context.source().send_success(&message, false);
    Ok(result)
}

/// Vanilla `BossBarCommands.getValue`.
fn get_value(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let events = lock_events(context.source());
    let bar = existing_bar(&events, id)?;
    let value = bar.value();
    let message = translations::COMMANDS_BOSSBAR_GET_VALUE
        .message([bar.display_name(), TextComponent::from(value.to_string())])
        .component();
    drop(events);

    context.source().send_success(&message, true);
    Ok(value)
}

/// Vanilla `BossBarCommands.getMax`.
fn get_max(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let events = lock_events(context.source());
    let bar = existing_bar(&events, id)?;
    let max = bar.max();
    let message = translations::COMMANDS_BOSSBAR_GET_MAX
        .message([bar.display_name(), TextComponent::from(max.to_string())])
        .component();
    drop(events);

    context.source().send_success(&message, true);
    Ok(max)
}

/// Vanilla `BossBarCommands.getVisible`.
fn get_visible(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let events = lock_events(context.source());
    let bar = existing_bar(&events, id)?;
    let visible = bar.is_visible();
    let message = if visible {
        translations::COMMANDS_BOSSBAR_GET_VISIBLE_VISIBLE
            .message([bar.display_name()])
            .component()
    } else {
        translations::COMMANDS_BOSSBAR_GET_VISIBLE_HIDDEN
            .message([bar.display_name()])
            .component()
    };
    drop(events);

    context.source().send_success(&message, true);
    Ok(i32::from(visible))
}

/// Vanilla `BossBarCommands.getPlayers`.
fn get_players(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let events = lock_events(context.source());
    let bar = existing_bar(&events, id)?;
    let message = if bar.player_uuids().is_empty() {
        translations::COMMANDS_BOSSBAR_GET_PLAYERS_NONE
            .message([bar.display_name()])
            .component()
    } else {
        translations::COMMANDS_BOSSBAR_GET_PLAYERS_SOME
            .message([
                bar.display_name(),
                TextComponent::from(bar.player_uuids().len().to_string()),
                format_list(bar.players().map(|player| player.display_name())),
            ])
            .component()
    };
    let result = player_count(bar)?;
    drop(events);

    context.source().send_success(&message, true);
    Ok(result)
}

/// Vanilla `BossBarCommands.setVisible`.
fn set_visible(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let visible = context.boolean("visible")?;
    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    if bar.is_visible() == visible {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            if visible {
                &translations::COMMANDS_BOSSBAR_SET_VISIBILITY_UNCHANGED_VISIBLE
            } else {
                &translations::COMMANDS_BOSSBAR_SET_VISIBILITY_UNCHANGED_HIDDEN
            },
        )));
    }

    bar.set_visible(visible);
    let message = if visible {
        translations::COMMANDS_BOSSBAR_SET_VISIBLE_SUCCESS_VISIBLE
            .message([bar.display_name()])
            .component()
    } else {
        translations::COMMANDS_BOSSBAR_SET_VISIBLE_SUCCESS_HIDDEN
            .message([bar.display_name()])
            .component()
    };
    drop(events);

    context.source().send_success(&message, true);
    Ok(0)
}

/// Vanilla `BossBarCommands.setValue`.
fn set_value(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let value = context.integer("value")?;
    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    if bar.value() == value {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BOSSBAR_SET_VALUE_UNCHANGED,
        )));
    }

    bar.set_value(value);
    let message = translations::COMMANDS_BOSSBAR_SET_VALUE_SUCCESS
        .message([bar.display_name(), TextComponent::from(value.to_string())])
        .component();
    drop(events);

    context.source().send_success(&message, true);
    Ok(value)
}

/// Vanilla `BossBarCommands.setMax`.
fn set_max(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let max = context.integer("max")?;
    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    if bar.max() == max {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BOSSBAR_SET_MAX_UNCHANGED,
        )));
    }

    bar.set_max(max);
    let message = translations::COMMANDS_BOSSBAR_SET_MAX_SUCCESS
        .message([bar.display_name(), TextComponent::from(max.to_string())])
        .component();
    drop(events);

    context.source().send_success(&message, true);
    Ok(max)
}

/// Vanilla `BossBarCommands.setColor`.
fn set_color(
    context: &SteelCommandContext<CommandSource>,
    color: BossBarColor,
) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    if bar.color() == color {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BOSSBAR_SET_COLOR_UNCHANGED,
        )));
    }

    bar.set_color(color);
    let message = translations::COMMANDS_BOSSBAR_SET_COLOR_SUCCESS
        .message([bar.display_name()])
        .component();
    drop(events);

    context.source().send_success(&message, true);
    Ok(0)
}

/// Vanilla `BossBarCommands.setStyle`.
fn set_style(
    context: &SteelCommandContext<CommandSource>,
    style: BossBarOverlay,
) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    if bar.overlay() == style {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BOSSBAR_SET_STYLE_UNCHANGED,
        )));
    }

    bar.set_overlay(style);
    let message = translations::COMMANDS_BOSSBAR_SET_STYLE_SUCCESS
        .message([bar.display_name()])
        .component();
    drop(events);

    context.source().send_success(&message, true);
    Ok(0)
}

/// Vanilla `BossBarCommands.setName`.
fn set_name(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let name = context
        .text_component("name")?
        .try_resolve(&CommandTextResolver::with_source(context.source()))?;

    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    if bar.name() == &name {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BOSSBAR_SET_NAME_UNCHANGED,
        )));
    }

    bar.set_name(name);
    let message = translations::COMMANDS_BOSSBAR_SET_NAME_SUCCESS
        .message([bar.display_name()])
        .component();
    drop(events);

    context.source().send_success(&message, true);
    Ok(0)
}

/// Vanilla `BossBarCommands.setPlayers` reached from `players <targets>`.
fn set_players_from_targets(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let targets = context.players("targets")?;
    set_players(context, &targets)
}

/// Vanilla `BossBarCommands.setPlayers`.
fn set_players(
    context: &SteelCommandContext<CommandSource>,
    targets: &[Arc<Player>],
) -> Result<i32, CommandSyntaxError> {
    let id = context.identifier("id")?;
    let mut events = lock_events(context.source());
    let bar = existing_bar_mut(&mut events, id)?;
    if !bar.set_players(targets) {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_BOSSBAR_SET_PLAYERS_UNCHANGED,
        )));
    }

    let message = if bar.player_uuids().is_empty() {
        translations::COMMANDS_BOSSBAR_SET_PLAYERS_SUCCESS_NONE
            .message([bar.display_name()])
            .component()
    } else {
        translations::COMMANDS_BOSSBAR_SET_PLAYERS_SUCCESS_SOME
            .message([
                bar.display_name(),
                TextComponent::from(targets.len().to_string()),
                format_list(targets.iter().map(|player| player.display_name())),
            ])
            .component()
    };
    let result = player_count(bar)?;
    drop(events);

    context.source().send_success(&message, true);
    Ok(result)
}
#[cfg(test)]
mod tests {
    use steel_registry::init_vanilla_registry;

    use super::super::create_dispatcher;
    use super::*;
    use crate::command::brigadier::{CommandDispatcher, NodeId};

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

    fn assert_argument(
        dispatcher: &Dispatcher,
        parent: NodeId,
        name: &str,
        argument_type: &SteelArgumentType,
        executable: bool,
    ) -> NodeId {
        let node = child(dispatcher, parent, name);
        assert_eq!(
            dispatcher.node(node).and_then(|node| node.argument_type()),
            Some(argument_type)
        );
        assert_eq!(
            dispatcher
                .node(node)
                .is_some_and(|node| node.is_executable()),
            executable
        );
        node
    }

    fn assert_executable_literal(dispatcher: &Dispatcher, parent: NodeId, name: &str) -> NodeId {
        let node = child(dispatcher, parent, name);
        assert!(matches!(
            dispatcher.node(node),
            Some(node) if node.is_executable()
        ));
        node
    }

    #[test]
    fn bossbar_graph_matches_vanilla_tree() {
        init_vanilla_registry();
        let Ok(dispatcher) = create_dispatcher() else {
            panic!("built-in commands should register");
        };

        let bossbar = child(&dispatcher, dispatcher.root(), "bossbar");
        assert!(matches!(
            dispatcher.node(bossbar),
            Some(node) if node.is_restricted() && !node.is_executable()
        ));

        let add = child(&dispatcher, bossbar, "add");
        let add_id = assert_argument(
            &dispatcher,
            add,
            "id",
            &SteelArgumentType::identifier_argument(),
            false,
        );
        assert_argument(
            &dispatcher,
            add_id,
            "name",
            &SteelArgumentType::component(),
            true,
        );

        // Vanilla `BossBarCommands.remove`: the literal is only a branch, the id
        // argument carries the executor.
        let remove = child(&dispatcher, bossbar, "remove");
        assert!(
            dispatcher
                .node(remove)
                .is_some_and(|node| !node.is_executable()),
            "`bossbar remove` requires an id"
        );
        assert_argument(
            &dispatcher,
            remove,
            "id",
            &SteelArgumentType::identifier_argument(),
            true,
        );

        assert_executable_literal(&dispatcher, bossbar, "list");

        let set = child(&dispatcher, bossbar, "set");
        let set_id = assert_argument(
            &dispatcher,
            set,
            "id",
            &SteelArgumentType::identifier_argument(),
            false,
        );

        let name = child(&dispatcher, set_id, "name");
        assert_argument(
            &dispatcher,
            name,
            "name",
            &SteelArgumentType::component(),
            true,
        );

        let color = child(&dispatcher, set_id, "color");
        for value in ["pink", "blue", "red", "green", "yellow", "purple", "white"] {
            assert_executable_literal(&dispatcher, color, value);
        }

        let style = child(&dispatcher, set_id, "style");
        for value in [
            "progress",
            "notched_6",
            "notched_10",
            "notched_12",
            "notched_20",
        ] {
            assert_executable_literal(&dispatcher, style, value);
        }

        let value = child(&dispatcher, set_id, "value");
        assert_argument(
            &dispatcher,
            value,
            "value",
            &SteelArgumentType::from(ArgumentType::integer(0, i32::MAX)),
            true,
        );

        let max = child(&dispatcher, set_id, "max");
        assert_argument(
            &dispatcher,
            max,
            "max",
            &SteelArgumentType::from(ArgumentType::integer(1, i32::MAX)),
            true,
        );

        let visible = child(&dispatcher, set_id, "visible");
        assert_argument(
            &dispatcher,
            visible,
            "visible",
            &SteelArgumentType::from(ArgumentType::bool()),
            true,
        );

        let players = assert_executable_literal(&dispatcher, set_id, "players");
        assert_argument(
            &dispatcher,
            players,
            "targets",
            &SteelArgumentType::players(),
            true,
        );

        let get = child(&dispatcher, bossbar, "get");
        let get_id = assert_argument(
            &dispatcher,
            get,
            "id",
            &SteelArgumentType::identifier_argument(),
            false,
        );
        for value in ["value", "max", "visible", "players"] {
            assert_executable_literal(&dispatcher, get_id, value);
        }
    }
}
