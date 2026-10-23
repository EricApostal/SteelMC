//! Vanilla `/list` command.
//!
//! Mirrors `ListPlayersCommand`: the listing uses each player's display name and
//! `commands.list.nameAndId` for the `uuids` form, joined by vanilla
//! `ComponentUtils.formatList`'s gray `, ` separator. Steel's `PlayerMap` is a
//! hash index, so entries are ordered by ascending entity id — the order players
//! are appended to vanilla's `PlayerList` — instead of hash iteration order.

use std::sync::Arc;

use steel_utils::{
    Identifier,
    translations::{COMMANDS_LIST_NAME_AND_ID, COMMANDS_LIST_PLAYERS},
};
use text_components::{Modifier, TextComponent, format::Color};

use super::super::{
    brigadier::{CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, literal},
    registration::CommandRegistration,
};
use crate::{entity::Entity, player::Player};

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("list"), |_| command()).default_access()
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("list")
        .executes(|context| list_players(context, false))
        .then(literal("uuids").executes(|context| list_players(context, true)))
}

fn list_players(
    context: &SteelCommandContext<CommandSource>,
    show_uuids: bool,
) -> Result<i32, CommandSyntaxError> {
    let mut players = context.source().server().get_players();
    players.sort_by_key(|player| player.id());
    let Ok(player_count) = i32::try_from(players.len()) else {
        return Err(CommandSyntaxError::dynamic(
            "Online player count exceeds the command result range",
        ));
    };
    let max_players = context.source().server().config.max_players;
    let message = COMMANDS_LIST_PLAYERS
        .message([
            TextComponent::from(player_count.to_string()),
            TextComponent::from(max_players.to_string()),
            format_player_list(&players, show_uuids),
        ])
        .component();
    context.source().send_success(&message, false);
    Ok(player_count)
}

/// Vanilla `ComponentUtils.formatList` applied to the online players.
fn format_player_list(players: &[Arc<Player>], show_uuids: bool) -> TextComponent {
    join_entries(
        players
            .iter()
            .map(|player| player_entry(player, show_uuids))
            .collect(),
    )
}

/// One listing entry: the player's display name, or `commands.list.nameAndId`.
fn player_entry(player: &Arc<Player>, show_uuids: bool) -> TextComponent {
    if show_uuids {
        COMMANDS_LIST_NAME_AND_ID
            .message([
                player.display_name(),
                TextComponent::from(player.gameprofile.id().to_string()),
            ])
            .component()
    } else {
        player.display_name()
    }
}

/// Vanilla `ComponentUtils.formatList`: gray `, ` separators between entries.
fn join_entries(entries: Vec<TextComponent>) -> TextComponent {
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
    use text_components::{Modifier, TextComponent, format::Color};

    use super::join_entries;

    #[test]
    fn entries_are_joined_with_a_gray_separator() {
        let joined = join_entries(vec![
            TextComponent::plain("Alice"),
            TextComponent::plain("Bob"),
            TextComponent::plain("Carol"),
        ]);
        let separator = TextComponent::plain(", ").color(Color::Gray);
        assert_eq!(
            joined.children,
            [
                TextComponent::plain("Alice"),
                separator.clone(),
                TextComponent::plain("Bob"),
                separator.clone(),
                TextComponent::plain("Carol"),
            ]
        );
    }

    #[test]
    fn a_single_entry_is_not_separated_and_an_empty_list_stays_empty() {
        let single = join_entries(vec![TextComponent::plain("Alice")]);
        assert_eq!(single.children, [TextComponent::plain("Alice")]);

        let empty = join_entries(Vec::new());
        assert!(empty.children.is_empty());
    }
}
