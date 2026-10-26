//! Sign block entity implementation.
//!
//! Signs store text on both front and back sides, along with color and glow
//! information.

use std::array;
use std::sync::{Arc, Weak};

use simdnbt::borrow::{
    BaseNbtCompound as BorrowedNbtCompound, NbtCompound as BorrowedNbtCompoundView,
};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::data_components::vanilla_components::{
    SIGN_TEXT_BACK, SIGN_TEXT_FRONT, SignText as SignTextComponent, WAXED,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::{DyeColor, vanilla_block_entity_types};
use steel_utils::translations;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex, types::SignTextSlot,
};
use text_components::interactivity::ClickEvent;
use text_components::{Modifier, TextComponent, content::Content, format::Color};
use uuid::Uuid;

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::command::sender::CommandSender;
use crate::player::Player;
use crate::world::World;

/// Number of text lines on each side of a sign.
pub const SIGN_LINES: usize = 4;

/// Text and styling for one side of a sign.
#[derive(Debug, Clone)]
pub struct SignText {
    /// The 4 lines of text (raw, unfiltered).
    pub messages: [TextComponent; SIGN_LINES],
    /// The 4 lines of filtered text (vanilla `SignText.filteredMessages`).
    ///
    /// Vanilla only serializes this list when it differs from [`Self::messages`].
    pub filtered_messages: [TextComponent; SIGN_LINES],
    /// Text color (dye color applied to the sign).
    pub color: DyeColor,
    /// Whether the text has a glowing effect (from glow ink sac).
    pub has_glowing_text: bool,
}

impl Default for SignText {
    fn default() -> Self {
        Self::new()
    }
}

impl SignText {
    /// Creates a new empty sign text with default color (black) and no glow.
    #[must_use]
    pub fn new() -> Self {
        Self {
            messages: array::from_fn(|_| TextComponent::new()),
            filtered_messages: array::from_fn(|_| TextComponent::new()),
            color: DyeColor::Black,
            has_glowing_text: false,
        }
    }

    /// Gets a message line by index.
    #[must_use]
    pub fn get_message(&self, index: usize) -> Option<&TextComponent> {
        self.messages.get(index)
    }

    /// Sets a message line by index.
    ///
    /// Mirrors vanilla `SignText.Mutable.setLine(int, Component)`: the filtered
    /// line is set to the same text.
    pub fn set_message(&mut self, index: usize, message: TextComponent) {
        if index < SIGN_LINES {
            self.messages[index] = message.clone();
            self.filtered_messages[index] = message;
        }
    }

    /// Vanilla `SignBlockEntity.updateMessages`: replaces one line's text while
    /// keeping the line's existing style.
    ///
    /// The style carries the click and hover events that make sign commands
    /// work, so editing a line must not drop them.
    pub fn set_message_preserving_style(&mut self, index: usize, message: &str) {
        if index >= SIGN_LINES {
            return;
        }
        let mut line = TextComponent::plain(message.to_owned());
        line.format = self.messages[index].format.clone();
        line.interactions = self.messages[index].interactions.clone();
        self.messages[index] = line.clone();
        self.filtered_messages[index] = line;
    }

    /// Vanilla `SignText.hasAnyClickCommands`: whether any line carries a
    /// `run_command` click event.
    #[must_use]
    pub fn has_any_click_commands(&self) -> bool {
        self.messages.iter().any(|line| {
            matches!(
                line.interactions.click.as_ref(),
                Some(ClickEvent::RunCommand { .. })
            )
        })
    }

    /// Checks if any line has text content.
    #[must_use]
    pub fn has_message(&self) -> bool {
        self.messages.iter().any(|msg| {
            // Check if the text component has any actual content
            match &msg.content {
                Content::Text { text } => !text.is_empty(),
                _ => true, // Translations, etc. count as having a message
            }
        })
    }

    /// Builds sign text from an item's `sign_text_front`/`sign_text_back`
    /// component, including the filtered lines when the component carries them.
    fn from_component(component: Option<&SignTextComponent>) -> Self {
        component.map_or_else(Self::new, |component| Self {
            messages: component.messages.clone(),
            filtered_messages: component
                .filtered_messages
                .clone()
                .unwrap_or_else(|| component.messages.clone()),
            color: component.color,
            has_glowing_text: component.has_glowing_text,
        })
    }

    /// Returns whether the sign editor can edit every line, i.e. all lines are
    /// plain text. Styling is ignored; translations, scores, etc. are not editable.
    #[must_use]
    pub fn has_editable_text(&self) -> bool {
        self.messages
            .iter()
            .all(|msg| matches!(msg.content, Content::Text { .. }))
    }

    /// Loads sign text from borrowed NBT, mirroring vanilla `SignText.CODEC`.
    ///
    /// A compound without a valid `messages` list decodes to the empty text,
    /// matching `input.read("front_text", SignText.CODEC).orElse(SignText.EMPTY)`.
    pub fn load(&mut self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        let Some(messages) = sign_lines_from_nbt(nbt, "messages") else {
            *self = Self::new();
            return;
        };

        // `SignText.load`: an absent `filtered_messages` list falls back to `messages`.
        let filtered = sign_lines_from_nbt(nbt, "filtered_messages");
        self.filtered_messages = filtered.unwrap_or_else(|| messages.clone());
        self.messages = messages;

        // `color` and `has_glowing_text` default to black and false.
        self.color = nbt
            .string("color")
            .and_then(|color_str| DyeColor::from_serialized_name(&color_str.to_str()))
            .unwrap_or(DyeColor::Black);
        self.has_glowing_text = nbt.byte("has_glowing_text").is_some_and(|glow| glow != 0);
    }

    /// Saves sign text to NBT with vanilla `SignText.CODEC` key names.
    pub fn save(&self, nbt: &mut NbtCompound) {
        nbt.insert("messages", sign_lines_to_nbt(&self.messages));

        // `lenientOptionalFieldOf`: `filtered_messages` is only written when it
        // differs from `messages` (`SignText.filteredMessagesForSerialization`).
        if self.filtered_messages != self.messages {
            nbt.insert(
                "filtered_messages",
                sign_lines_to_nbt(&self.filtered_messages),
            );
        }

        // Save color
        nbt.insert("color", self.color.serialized_name());

        // Save glow
        nbt.insert("has_glowing_text", i8::from(self.has_glowing_text));
    }
}

/// Decodes one 4-line component list, mirroring `SignText.messagesFromNbt`.
fn sign_lines_from_nbt(
    nbt: BorrowedNbtCompoundView<'_, '_>,
    key: &str,
) -> Option<[TextComponent; SIGN_LINES]> {
    let tags = nbt.list(key)?.to_owned().as_nbt_tags();
    let messages = tags
        .iter()
        .map(TextComponent::from_nbt)
        .collect::<Option<Vec<_>>>()?;
    <[TextComponent; SIGN_LINES]>::try_from(messages).ok()
}

/// Encodes one 4-line component list, mirroring `SignText.messagesToNbt`.
fn sign_lines_to_nbt(lines: &[TextComponent; SIGN_LINES]) -> NbtList {
    NbtList::from(
        lines
            .iter()
            .map(TextComponent::to_codec_nbt)
            .collect::<Vec<_>>(),
    )
}

/// Sign block entity.
///
/// Stores text on both front and back sides of the sign.
pub struct SignBlockEntity {
    base: BlockEntityBase,
    sign: SyncMutex<SignState>,
}

struct SignState {
    /// Text on the front side.
    front_text: SignText,
    /// Text on the back side.
    back_text: SignText,
    /// Whether the sign is waxed (prevents editing).
    is_waxed: bool,
    /// Vanilla `allow_op_features`: whether click commands on this sign run
    /// with op features. Only NBT-authored signs set it.
    allow_op_features: bool,
    /// UUID of the player currently allowed to edit this sign.
    /// Used to prevent multiple players from editing simultaneously.
    player_who_may_edit: Option<Uuid>,
}

// SAFETY: This key identifies Steel's shared sign implementation for both sign
// registry entries, rather than either registry entry itself.
unsafe impl DowncastType for SignBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sign");
}

impl SignBlockEntity {
    /// Creates a new sign block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self::with_type(level, &vanilla_block_entity_types::SIGN, pos, state)
    }

    /// Creates a new hanging sign block entity.
    #[must_use]
    pub fn new_hanging(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self::with_type(level, &vanilla_block_entity_types::HANGING_SIGN, pos, state)
    }

    /// Creates a sign block entity with a specific type.
    #[must_use]
    pub fn with_type(
        level: Weak<World>,
        block_entity_type: BlockEntityTypeRef,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Self {
        Self {
            base: BlockEntityBase::new(block_entity_type, level, pos, state),
            sign: SyncMutex::new(SignState {
                front_text: SignText::new(),
                back_text: SignText::new(),
                is_waxed: false,
                allow_op_features: false,
                player_who_may_edit: None,
            }),
        }
    }

    /// Gets the UUID of the player currently allowed to edit this sign.
    #[must_use]
    pub fn get_player_who_may_edit(&self) -> Option<Uuid> {
        self.sign.lock().player_who_may_edit
    }

    /// Sets the player allowed to edit this sign.
    pub fn set_player_who_may_edit(&self, player: Option<Uuid>) {
        self.sign.lock().player_who_may_edit = player;
    }

    /// Checks if another player (not the given one) is currently editing this sign.
    #[must_use]
    pub fn is_other_player_editing(&self, player_uuid: Uuid) -> bool {
        self.sign
            .lock()
            .player_who_may_edit
            .is_some_and(|editor| editor != player_uuid)
    }

    /// Gets the text for a side.
    #[must_use]
    pub fn get_text(&self, slot: SignTextSlot) -> SignText {
        let sign = self.sign.lock();
        if slot.is_front() {
            sign.front_text.clone()
        } else {
            sign.back_text.clone()
        }
    }

    /// Returns whether this sign is waxed.
    #[must_use]
    pub fn is_waxed(&self) -> bool {
        self.sign.lock().is_waxed
    }

    /// Vanilla `SignBlockEntity.allowOpFeatures`.
    #[must_use]
    pub fn allows_op_features(&self) -> bool {
        self.sign.lock().allow_op_features
    }

    /// Vanilla `SignText.hasAnyClickCommands` for one side.
    #[must_use]
    pub fn has_any_click_commands(&self, slot: SignTextSlot) -> bool {
        self.get_text(slot).has_any_click_commands()
    }

    /// Vanilla `SignBlockEntity.canExecuteClickCommands`: a waxed sign whose
    /// side carries a `run_command` click event.
    #[must_use]
    pub fn can_execute_click_commands(&self, slot: SignTextSlot) -> bool {
        self.is_waxed() && self.has_any_click_commands(slot)
    }

    /// Vanilla `SignBlockEntity.executeClickCommandsIfPresent`: runs the
    /// `run_command` click events on one side and reports whether any were
    /// found.
    ///
    /// Vanilla builds a `GAMEMASTER`-level command source carrying the player
    /// as its entity. Steel's command queue only offers player-sourced
    /// execution, so the command runs with the clicking player's permissions;
    /// the `found` result and the `sign.click_actions_disabled` overlay match
    /// vanilla.
    pub fn execute_click_commands_if_present(
        &self,
        world: &Arc<World>,
        player: &Player,
        slot: SignTextSlot,
    ) -> bool {
        let text = self.get_text(slot);
        let allow_op_features = self.allows_op_features();
        let mut found = false;
        for line in &text.messages {
            let Some(ClickEvent::RunCommand { command }) = line.interactions.click.as_ref() else {
                continue;
            };
            if allow_op_features {
                let Some(source) = world.players.get_by_uuid(&player.gameprofile.id()) else {
                    continue;
                };
                let command = command.strip_prefix('/').unwrap_or(command);
                let _ = player
                    .server()
                    .submit_command(CommandSender::Player(source), command.to_owned());
            }
            found = true;
        }
        if found && !allow_op_features {
            player.send_overlay_message(
                &TextComponent::translated(translations::SIGN_CLICK_ACTIONS_DISABLED.msg())
                    .color(Color::Red),
            );
        }
        found
    }

    /// Replaces one side's lines while keeping each line's style, mirroring
    /// vanilla `SignBlockEntity.updateSignText` → `updateMessages`.
    pub fn set_lines_preserving_style(&self, slot: SignTextSlot, lines: &[String]) {
        let mut text = self.get_text(slot);
        for (index, line) in lines.iter().take(SIGN_LINES).enumerate() {
            text.set_message_preserving_style(index, line);
        }
        self.set_text(text, slot);
    }

    /// Vanilla `SignBlockEntity.setWaxed(true)`: returns whether the state changed
    /// and marks the sign updated so clients receive the new `is_waxed` value.
    pub fn wax(&self) -> bool {
        {
            let mut sign = self.sign.lock();
            if sign.is_waxed {
                return false;
            }
            sign.is_waxed = true;
        }
        self.mark_updated();
        true
    }
    /// Sets whether a side's text glows, returning whether its state changed or not :3.
    ///
    /// Mirrors vanilla `SignBlockEntity.updateText`: returns false when the side
    /// already has the requested glow state, so callers can skip consuming the item.
    pub fn set_glowing(&self, slot: SignTextSlot, glowing: bool) -> bool {
        {
            let mut sign = self.sign.lock();
            let text = if slot.is_front() {
                &mut sign.front_text
            } else {
                &mut sign.back_text
            };
            if text.has_glowing_text == glowing {
                return false;
            }
            text.has_glowing_text = glowing;
        }
        self.mark_updated();
        true
    }

    /// Vanilla `SignBlockEntity.setText`: replaces one side and calls `markUpdated`.
    pub fn set_text(&self, text: SignText, slot: SignTextSlot) {
        {
            let mut sign = self.sign.lock();
            if slot.is_front() {
                sign.front_text = text;
            } else {
                sign.back_text = text;
            }
        }
        self.mark_updated();
    }

    /// Vanilla `SignBlockEntity.markUpdated`: `setChanged` plus
    /// `level.sendBlockUpdated(pos, state, state, 3)`, whose block-entity half
    /// pushes the new text to tracking clients immediately.
    fn mark_updated(&self) {
        BlockEntity::set_changed(self);
        if let Some(world) = self.get_level() {
            world.broadcast_block_entity_if_needed(self.get_block_pos());
        }
    }
}

impl BlockEntity for SignBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        // Convert to NbtCompound view for accessing methods
        let nbt_view: BorrowedNbtCompoundView<'_, '_> = nbt.into();
        let mut sign = self.sign.lock();

        // Vanilla reads each side with `SignText.CODEC` and falls back to
        // `SignText.EMPTY` when the key is absent or fails to decode.
        sign.front_text = match nbt_view.compound("front_text") {
            Some(front_nbt) => {
                let mut text = SignText::new();
                text.load(front_nbt);
                text
            }
            None => SignText::new(),
        };

        sign.back_text = match nbt_view.compound("back_text") {
            Some(back_nbt) => {
                let mut text = SignText::new();
                text.load(back_nbt);
                text
            }
            None => SignText::new(),
        };

        // Vanilla `input.getBooleanOr("is_waxed", false)`.
        sign.is_waxed = nbt_view.byte("is_waxed").is_some_and(|waxed| waxed != 0);
        // Vanilla `input.getBooleanOr("allow_op_features", false)`.
        sign.allow_op_features = nbt_view
            .byte("allow_op_features")
            .is_some_and(|allow| allow != 0);
    }

    fn apply_components_from_item(&self, item: &ItemStack) {
        let mut sign = self.sign.lock();
        sign.front_text = SignText::from_component(item.get(SIGN_TEXT_FRONT));
        sign.back_text = SignText::from_component(item.get(SIGN_TEXT_BACK));
        sign.is_waxed = item.has(WAXED);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let sign = self.sign.lock();
        // Vanilla only writes `allow_op_features` when it is set.
        if sign.allow_op_features {
            nbt.insert("allow_op_features", 1_i8);
        }
        // Save front text
        let mut front_nbt = NbtCompound::new();
        sign.front_text.save(&mut front_nbt);
        nbt.insert("front_text", front_nbt);

        // Save back text
        let mut back_nbt = NbtCompound::new();
        sign.back_text.save(&mut back_nbt);
        nbt.insert("back_text", back_nbt);

        // Save waxed state
        nbt.insert("is_waxed", i8::from(sign.is_waxed));
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Send full sign data to client
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }

    fn tick(&self, world: &Arc<World>) {
        // Clear the edit lock if the editing player is too far away or gone
        let editor_uuid = self.sign.lock().player_who_may_edit;
        let Some(editor_uuid) = editor_uuid else {
            return;
        };
        let should_clear = world
            .players
            .get_by_uuid(&editor_uuid)
            .is_none_or(|player| {
                // Vanilla `SignBlockEntity.playerIsTooFarAwayToEdit`:
                // `!editingPlayer.isWithinBlockInteractionRange(pos, 4.0)`.
                !player.is_within_block_interaction_range_with_buffer(self.get_block_pos(), 4.0)
            });

        if should_clear {
            let mut sign = self.sign.lock();
            if sign.player_who_may_edit == Some(editor_uuid) {
                sign.player_who_may_edit = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{array, io::Cursor, sync::Arc};

    use simdnbt::borrow::{read_compound, read_tag};
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_items};
    use steel_utils::{BlockPos, translations, types::SignTextSlot};
    use text_components::{
        Modifier as _, TextComponent, content::Content, interactivity::ClickEvent,
    };
    use uuid::Uuid;

    use super::{
        DyeColor, ItemStack, SIGN_TEXT_FRONT, SignBlockEntity, SignText, SignTextComponent, WAXED,
    };
    use crate::block_entity::BlockEntity as _;
    use crate::test_support::fresh_test_world;

    #[test]
    fn plain_sign_lines_save_as_a_string_list() {
        let mut text = SignText::new();
        text.messages = array::from_fn(|index| TextComponent::plain(index.to_string()));

        let mut nbt = NbtCompound::new();
        text.save(&mut nbt);

        assert_eq!(
            nbt.get("messages"),
            Some(&NbtTag::List(NbtList::String(vec![
                "0".into(),
                "1".into(),
                "2".into(),
                "3".into(),
            ])))
        );
    }

    #[test]
    fn filtered_sign_lines_are_written_only_when_they_differ() {
        let mut text = SignText::new();
        text.set_message(0, TextComponent::plain("raw"));
        text.filtered_messages[0] = TextComponent::plain("filtered");

        let mut nbt = NbtCompound::new();
        text.save(&mut nbt);
        assert_eq!(
            nbt.get("filtered_messages"),
            Some(&NbtTag::List(NbtList::String(vec![
                "filtered".into(),
                "".into(),
                "".into(),
                "".into(),
            ])))
        );

        // `SignText.load` defaults the filtered lines to the raw ones.
        text.filtered_messages = text.messages.clone();
        let mut nbt = NbtCompound::new();
        text.save(&mut nbt);
        assert!(nbt.get("filtered_messages").is_none());

        let mut bytes = Vec::new();
        NbtTag::Compound(nbt).write(&mut bytes);
        let borrowed = read_tag(&mut Cursor::new(bytes.as_slice()))
            .expect("saved sign text should be valid NBT");
        let mut decoded = SignText::new();
        decoded.load(
            borrowed
                .as_tag()
                .compound()
                .expect("saved sign text should be a compound"),
        );
        assert_eq!(decoded.filtered_messages, decoded.messages);
    }

    #[test]
    fn mixed_sign_lines_round_trip_through_the_component_codec() {
        let mut expected = SignText::new();
        expected.messages[0] = TextComponent::plain("plain");
        expected.messages[1] = TextComponent::plain("styled").bold(true);

        let mut nbt = NbtCompound::new();
        expected.save(&mut nbt);
        assert!(matches!(
            nbt.get("messages"),
            Some(NbtTag::List(NbtList::Compound(_)))
        ));

        let mut bytes = Vec::new();
        NbtTag::Compound(nbt).write(&mut bytes);
        let borrowed = read_tag(&mut Cursor::new(bytes.as_slice()))
            .expect("saved sign text should be valid NBT");
        let borrowed_tag = borrowed.as_tag();
        let compound = borrowed_tag
            .compound()
            .expect("saved sign text should be a compound");

        let mut decoded = SignText::new();
        decoded.load(compound);

        assert_eq!(decoded.messages, expected.messages);
        assert_eq!(decoded.color, expected.color);
        assert_eq!(decoded.has_glowing_text, expected.has_glowing_text);
    }

    #[test]
    fn styled_plain_text_is_editable_but_translations_are_not() {
        let mut text = SignText::new();
        text.messages[0] = TextComponent::plain("styled").bold(true);
        assert!(text.has_editable_text());

        text.messages[1] = TextComponent::translated(translations::BLOCK_MINECRAFT_STONE.msg());
        assert!(!text.has_editable_text());
    }

    #[test]
    fn placed_sign_takes_text_and_wax_from_item_components() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("sign_item_components");
        let world = &world_fixture.world;
        let pos = BlockPos::new(0, 64, 0);
        let sign = SignBlockEntity::new(
            Arc::downgrade(world),
            pos,
            vanilla_blocks::OAK_SIGN.default_state(),
        );

        let mut messages = array::from_fn(|_| TextComponent::new());
        messages[0] = TextComponent::plain("front");
        let mut item = ItemStack::new(&vanilla_items::OAK_SIGN);
        item.set(
            SIGN_TEXT_FRONT,
            SignTextComponent::new(messages.clone(), None, DyeColor::Red, true),
        );
        item.set(WAXED, ());

        sign.apply_components_from_item(&item);

        let front = sign.get_text(SignTextSlot::Front);
        assert_eq!(front.messages, messages);
        assert_eq!(front.color, DyeColor::Red);
        assert!(front.has_glowing_text);
        assert!(!sign.get_text(SignTextSlot::Back).has_message());
        assert!(sign.is_waxed());
    }

    #[test]
    fn sign_tick_releases_state_before_player_lookup_and_editor_clear() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("sign_editor_clear");
        let world = &world_fixture.world;
        let sign = SignBlockEntity::new(
            Arc::downgrade(world),
            BlockPos::new(8, 64, 8),
            vanilla_blocks::OAK_SIGN.default_state(),
        );
        sign.set_player_who_may_edit(Some(Uuid::from_u128(1)));

        sign.tick(world);
        assert_eq!(sign.get_player_who_may_edit(), None);
    }

    /// Vanilla `SignBlockEntity.updateMessages` copies the old line's style onto
    /// the new literal, so a `run_command` click event survives editing.
    #[test]
    fn edited_sign_lines_keep_their_click_events() {
        let mut text = SignText::new();
        text.messages[0] =
            TextComponent::plain("old").click_event(ClickEvent::run_command("/say hi"));
        text.set_message_preserving_style(0, "new");

        assert!(text.has_any_click_commands());
        let line = &text.messages[0];
        assert!(matches!(
            &line.content,
            Content::Text { text } if text.as_ref() == "new"
        ));
        assert!(matches!(
            line.interactions.click.as_ref(),
            Some(ClickEvent::RunCommand { .. })
        ));
        assert_eq!(text.filtered_messages[0], text.messages[0]);
    }

    /// Vanilla only writes `allow_op_features` when it is set and reads it back
    /// from NBT; the flag gates sign click-command execution.
    #[test]
    fn allow_op_features_round_trips_through_nbt() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("sign_allow_op_features");
        let world = &world_fixture.world;
        let sign = SignBlockEntity::new(
            Arc::downgrade(world),
            BlockPos::new(4, 64, 4),
            vanilla_blocks::OAK_SIGN.default_state(),
        );
        assert!(!sign.allows_op_features());

        let mut nbt = NbtCompound::new();
        nbt.insert("allow_op_features", 1_i8);
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(bytes.as_slice()))
            .expect("allow_op_features NBT should reborrow");
        sign.load_additional(&borrowed);
        assert!(sign.allows_op_features());

        let mut saved = NbtCompound::new();
        sign.save_additional(&mut saved);
        assert_eq!(saved.get("allow_op_features"), Some(&NbtTag::Byte(1)));
    }
}
