//! Text display entity implementation.
//!
//! Display entities render a block, item, or text without collision.
//! They're commonly used for visual effects, holograms, and decorations.

use std::sync::Weak;

use bitflags::bitflags;
use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::blocks::behavior::PushReaction;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_entity_data::TextDisplayEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::TextComponent;
use uuid::Uuid;

use super::display_data;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::world::World;

/// NBT tag holding the display text.
const TAG_TEXT: &str = "text";
/// NBT tag holding the maximum line width.
const TAG_LINE_WIDTH: &str = "line_width";
/// NBT tag holding the text opacity.
const TAG_TEXT_OPACITY: &str = "text_opacity";
/// NBT tag holding the background color.
const TAG_BACKGROUND: &str = "background";
/// NBT tag holding the shadow flag.
const TAG_SHADOW: &str = "shadow";
/// NBT tag holding the see-through flag.
const TAG_SEE_THROUGH: &str = "see_through";
/// NBT tag holding the default-background flag.
const TAG_DEFAULT_BACKGROUND: &str = "default_background";
/// NBT tag holding the text alignment.
const TAG_ALIGNMENT: &str = "alignment";

bitflags! {
    /// Text display style flags, mirroring vanilla `Display.TextDisplay` flag masks.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct TextDisplayFlags: u8 {
        /// Renders the text with a drop shadow.
        const SHADOW = 1;
        /// Renders the text visible through walls.
        const SEE_THROUGH = 2;
        /// Renders the text with the default background.
        const USE_DEFAULT_BACKGROUND = 4;
        /// Left-aligns the text within the display.
        const ALIGN_LEFT = 8;
        /// Right-aligns the text within the display.
        const ALIGN_RIGHT = 16;
    }
}

impl Default for TextDisplayFlags {
    fn default() -> Self {
        Self::empty()
    }
}

/// Horizontal alignment of a text display, mirroring vanilla `Display.TextDisplay.Align`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextDisplayAlign {
    /// Text is centered, the vanilla default.
    #[default]
    Center,
    /// Text is left-aligned.
    Left,
    /// Text is right-aligned.
    Right,
}

impl TextDisplayAlign {
    /// Returns vanilla's serialized name for this alignment.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// Parses vanilla's serialized name, returning `None` for unknown names.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "center" => Some(Self::Center),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }

    /// Extracts the alignment from the synced style flags.
    #[must_use]
    pub fn from_flags(flags: TextDisplayFlags) -> Self {
        if flags.contains(TextDisplayFlags::ALIGN_LEFT) {
            Self::Left
        } else if flags.contains(TextDisplayFlags::ALIGN_RIGHT) {
            Self::Right
        } else {
            Self::Center
        }
    }

    /// Returns the flag bits encoding this alignment.
    #[must_use]
    pub const fn flag_bits(self) -> u8 {
        match self {
            Self::Center => 0,
            Self::Left => TextDisplayFlags::ALIGN_LEFT.bits(),
            Self::Right => TextDisplayFlags::ALIGN_RIGHT.bits(),
        }
    }
}

/// A text display entity that renders text at its position.
///
/// Text displays are purely visual entities with no collision.
/// They support transformation (translation, rotation, scale) and
/// interpolation for smooth animations.
#[entity_behavior(class = "TextDisplay")]
pub struct TextDisplayEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced entity data for network serialization.
    entity_data: SyncMutex<TextDisplayEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TextDisplayEntity`.
unsafe impl DowncastType for TextDisplayEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/text_display");
}

impl TextDisplayEntity {
    /// Creates a new text display entity.
    ///
    /// The `id` should be obtained from `next_entity_id()`.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(TextDisplayEntityData::new()),
        }
    }

    /// Creates a new text display entity with a specific UUID.
    ///
    /// The `id` should be obtained from `next_entity_id()`.
    #[must_use]
    pub fn with_uuid(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        uuid: Uuid,
        world: Weak<World>,
    ) -> Self {
        Self {
            base: EntityBase::with_uuid(id, uuid, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(TextDisplayEntityData::new()),
        }
    }

    /// Creates a text display entity from saved data.
    ///
    /// Display entities have no physical collision, but vanilla base state is
    /// still persisted and should round-trip through the shared base.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(TextDisplayEntityData::new()),
        }
    }

    /// Gets a reference to the entity data for reading/modifying synced state.
    pub const fn entity_data(&self) -> &SyncMutex<TextDisplayEntityData> {
        &self.entity_data
    }

    /// Returns the display text.
    #[must_use]
    pub fn text(&self) -> TextComponent {
        (**self.entity_data.lock().text.get()).clone()
    }

    /// Sets the display text.
    pub fn set_text(&self, text: TextComponent) {
        self.entity_data.lock().text.set(Box::new(text));
    }

    /// Returns the maximum line width used when wrapping text.
    #[must_use]
    pub fn line_width(&self) -> i32 {
        *self.entity_data.lock().line_width.get()
    }

    /// Sets the maximum line width used when wrapping text.
    pub fn set_line_width(&self, line_width: i32) {
        self.entity_data.lock().line_width.set(line_width);
    }

    /// Returns the text opacity byte.
    #[must_use]
    pub fn text_opacity(&self) -> i8 {
        *self.entity_data.lock().text_opacity.get()
    }

    /// Sets the text opacity byte.
    pub fn set_text_opacity(&self, text_opacity: i8) {
        self.entity_data.lock().text_opacity.set(text_opacity);
    }

    /// Returns the packed background color.
    #[must_use]
    pub fn background_color(&self) -> i32 {
        *self.entity_data.lock().background_color.get()
    }

    /// Sets the packed background color.
    pub fn set_background_color(&self, background_color: i32) {
        self.entity_data
            .lock()
            .background_color
            .set(background_color);
    }

    /// Returns the style flags byte.
    #[must_use]
    pub fn style_flags(&self) -> TextDisplayFlags {
        TextDisplayFlags::from_bits_retain(*self.entity_data.lock().style_flags.get() as u8)
    }

    /// Sets the style flags byte.
    pub fn set_style_flags(&self, flags: TextDisplayFlags) {
        self.entity_data.lock().style_flags.set(flags.bits() as i8);
    }

    /// Returns the text alignment derived from the style flags.
    #[must_use]
    pub fn align(&self) -> TextDisplayAlign {
        TextDisplayAlign::from_flags(self.style_flags())
    }

    /// Sets the text alignment, preserving the non-alignment style flags.
    pub fn set_align(&self, align: TextDisplayAlign) {
        let mut flags = self.style_flags();
        flags.remove(TextDisplayFlags::ALIGN_LEFT | TextDisplayFlags::ALIGN_RIGHT);
        flags.insert(TextDisplayFlags::from_bits_retain(align.flag_bits()));
        self.set_style_flags(flags);
    }
}

/// Reads a single style flag from its boolean NBT tag.
fn load_flag(
    flags: TextDisplayFlags,
    nbt: &BorrowedNbtCompoundView<'_, '_>,
    tag: &str,
    flag: TextDisplayFlags,
) -> TextDisplayFlags {
    let set = nbt
        .get(tag)
        .and_then(simdnbt::FromNbtTag::from_nbt_tag)
        .unwrap_or(false);
    if set { flags | flag } else { flags }
}

impl Entity for TextDisplayEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    // Vanilla `Display` disables physics and ignores piston pushes for every display type.
    fn is_ignoring_block_triggers(&self) -> bool {
        true
    }

    fn piston_push_reaction(&self) -> PushReaction {
        PushReaction::IgnoreEntity
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        false
    }

    fn no_physics(&self) -> bool {
        true
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let entity_data = self.entity_data.lock();
        display_data::save(&entity_data.display, nbt);
        nbt.insert(TAG_TEXT, entity_data.text.get().to_codec_nbt());
        nbt.insert(TAG_LINE_WIDTH, *entity_data.line_width.get());
        nbt.insert(TAG_BACKGROUND, *entity_data.background_color.get());
        nbt.insert(TAG_TEXT_OPACITY, *entity_data.text_opacity.get());

        let flags = TextDisplayFlags::from_bits_retain(*entity_data.style_flags.get() as u8);
        nbt.insert(TAG_SHADOW, flags.contains(TextDisplayFlags::SHADOW));
        nbt.insert(
            TAG_SEE_THROUGH,
            flags.contains(TextDisplayFlags::SEE_THROUGH),
        );
        nbt.insert(
            TAG_DEFAULT_BACKGROUND,
            flags.contains(TextDisplayFlags::USE_DEFAULT_BACKGROUND),
        );
        nbt.insert(TAG_ALIGNMENT, TextDisplayAlign::from_flags(flags).name());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        let defaults = TextDisplayEntityData::new();
        let mut entity_data = self.entity_data.lock();
        display_data::load(&mut entity_data.display, nbt);

        entity_data.line_width.set(
            nbt.get(TAG_LINE_WIDTH)
                .and_then(simdnbt::FromNbtTag::from_nbt_tag)
                .unwrap_or(*defaults.line_width.get()),
        );
        entity_data.text_opacity.set(
            nbt.get(TAG_TEXT_OPACITY)
                .and_then(simdnbt::FromNbtTag::from_nbt_tag)
                .unwrap_or(*defaults.text_opacity.get()),
        );
        entity_data.background_color.set(
            nbt.get(TAG_BACKGROUND)
                .and_then(simdnbt::FromNbtTag::from_nbt_tag)
                .unwrap_or(*defaults.background_color.get()),
        );

        let mut flags = TextDisplayFlags::empty();
        flags = load_flag(flags, &nbt, TAG_SHADOW, TextDisplayFlags::SHADOW);
        flags = load_flag(flags, &nbt, TAG_SEE_THROUGH, TextDisplayFlags::SEE_THROUGH);
        flags = load_flag(
            flags,
            &nbt,
            TAG_DEFAULT_BACKGROUND,
            TextDisplayFlags::USE_DEFAULT_BACKGROUND,
        );
        if let Some(align) = nbt
            .string(TAG_ALIGNMENT)
            .and_then(|name| TextDisplayAlign::from_name(&name.to_str()))
        {
            flags.insert(TextDisplayFlags::from_bits_retain(align.flag_bits()));
        }
        entity_data.style_flags.set(flags.bits() as i8);

        if let Some(text) = nbt
            .get(TAG_TEXT)
            .and_then(|tag| TextComponent::from_nbt(&tag.to_owned()))
        {
            // NOTE: vanilla resolves selectors/scores against the executing context and
            // drops the text on the client. Steel keeps the parsed codec-NBT component as-is;
            // contextual resolution will arrive with the chat/component resolution pass.
            entity_data.text.set(Box::new(text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simdnbt::borrow::read_compound;
    use std::io::Cursor;
    use steel_registry::vanilla_entities;

    const TEST_POSITION: DVec3 = DVec3::new(-8.0, 128.0, 4.0);

    fn new_display() -> TextDisplayEntity {
        TextDisplayEntity::new(
            &vanilla_entities::TEXT_DISPLAY,
            1,
            TEST_POSITION,
            Weak::new(),
        )
    }

    #[test]
    fn text_display_round_trips_text_and_flags() {
        let display = new_display();
        display.set_text(TextComponent::plain("hello"));
        display.set_line_width(80);
        display.set_text_opacity(120);
        display.set_background_color(0x11223344u32 as i32);
        display.set_align(TextDisplayAlign::Left);
        display.set_style_flags(display.style_flags() | TextDisplayFlags::SHADOW);

        let mut nbt = NbtCompound::new();
        display.save_additional(&mut nbt);

        assert_eq!(nbt.byte("text_opacity"), Some(120));
        assert_eq!(nbt.int("background"), Some(0x11223344u32 as i32));
        assert_eq!(nbt.int("line_width"), Some(80));
        assert_eq!(nbt.byte("shadow"), Some(1));
        assert_eq!(nbt.byte("see_through"), Some(0));
        assert_eq!(nbt.byte("default_background"), Some(0));
        assert_eq!(
            nbt.string("alignment")
                .map(|name| name.to_str().into_owned()),
            Some("left".to_owned())
        );

        let bytes = {
            let mut bytes = Vec::new();
            nbt.write(&mut bytes);
            bytes
        };
        let borrowed = read_compound(&mut Cursor::new(&bytes))
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));

        let restored = new_display();
        restored.load_additional((&borrowed).into());
        assert_eq!(restored.line_width(), 80);
        assert_eq!(restored.text_opacity(), 120);
        assert_eq!(restored.background_color(), 0x11223344u32 as i32);
        assert_eq!(restored.align(), TextDisplayAlign::Left);
        assert!(restored.style_flags().contains(TextDisplayFlags::SHADOW));
        assert_eq!(restored.text().to_string(), "hello");
    }

    #[test]
    fn text_display_missing_tags_restore_vanilla_defaults() {
        let nbt = NbtCompound::new();
        let bytes = {
            let mut bytes = Vec::new();
            nbt.write(&mut bytes);
            bytes
        };
        let borrowed = read_compound(&mut Cursor::new(&bytes))
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));

        let defaults = TextDisplayEntityData::new();
        let restored = new_display();
        restored.load_additional((&borrowed).into());
        assert_eq!(restored.line_width(), *defaults.line_width.get());
        assert_eq!(restored.text_opacity(), *defaults.text_opacity.get());
        assert_eq!(
            restored.background_color(),
            *defaults.background_color.get()
        );
        assert!(restored.style_flags().is_empty());
        assert_eq!(restored.text().to_string(), defaults.text.get().to_string());
    }

    #[test]
    fn text_display_align_names_match_vanilla() {
        for align in [
            TextDisplayAlign::Center,
            TextDisplayAlign::Left,
            TextDisplayAlign::Right,
        ] {
            assert_eq!(TextDisplayAlign::from_name(align.name()), Some(align));
            assert_eq!(
                TextDisplayAlign::from_flags(TextDisplayFlags::from_bits_retain(align.flag_bits())),
                align
            );
        }
        assert_eq!(TextDisplayAlign::from_name("unknown"), None);
    }
}
