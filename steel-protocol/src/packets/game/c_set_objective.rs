//! Clientbound set objective packet.

use std::io::{Result, Write};

use simdnbt::owned::NbtCompound;
use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_SET_OBJECTIVE;
use steel_utils::{
    codec::VarInt,
    serial::{PrefixedWrite, WriteTo},
};
use text_components::{
    TextComponent,
    format::{Color, Format},
};

/// The NBT compound tag id, which vanilla writes in front of a root tag's entries
/// (`NbtIo.writeAnyTag`).
const NBT_COMPOUND_TAG_ID: u8 = 0x0A;

/// Registry ids of vanilla's `minecraft:number_format_type` entries, in
/// `NumberFormatTypes.bootstrap` registration order.
const NUMBER_FORMAT_TYPE_BLANK: i32 = 0;
/// Registry id of the `minecraft:styled` number format type.
const NUMBER_FORMAT_TYPE_STYLED: i32 = 1;
/// Registry id of the `minecraft:fixed` number format type.
const NUMBER_FORMAT_TYPE_FIXED: i32 = 2;

/// How the scores of an objective are rendered (`ObjectiveCriteria.RenderType`).
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum ObjectiveRenderType {
    /// Render the raw score.
    Integer = 0,
    /// Render the score as a number of hearts.
    Hearts = 1,
}

/// Which change a [`CSetObjective`] packet applies.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = i8)]
pub enum ObjectiveMethod {
    /// Create the objective.
    Add = 0,
    /// Remove the objective.
    Remove = 1,
    /// Update the objective's display settings.
    Change = 2,
}

impl ObjectiveMethod {
    /// Whether this method carries the display name, render type and number format.
    #[must_use]
    pub const fn has_display_settings(self) -> bool {
        matches!(self, Self::Add | Self::Change)
    }
}

/// A scoreboard number format (`NumberFormatTypes.STREAM_CODEC`).
///
/// Vanilla dispatches on the `minecraft:number_format_type` registry, so the wire
/// format is the registry id followed by that type's own payload.
#[derive(Clone, Debug)]
pub enum NumberFormat {
    /// `minecraft:blank`: the score is rendered without a number.
    Blank,
    /// `minecraft:styled`: the score is rendered with the given text style.
    Styled {
        /// Vanilla `Style` NBT compound, e.g. `{"color": "red"}`.
        style: NbtCompound,
    },
    /// `minecraft:fixed`: the given component is rendered instead of the score.
    Fixed {
        /// Component rendered for every score.
        value: TextComponent,
    },
}

impl NumberFormat {
    /// Builds a `minecraft:styled` number format from a text style.
    ///
    /// Vanilla's `Style` codec also accepts `click_event`, `hover_event` and
    /// `insertion`, which live in `text_components`' `Interactivity` and have no
    /// public NBT conversion; add those entries to [`NumberFormat::Styled`]'s
    /// compound directly if they are ever needed.
    #[must_use]
    pub fn styled(format: &Format) -> Self {
        let mut style = NbtCompound::new();
        // Vanilla `Style.Serializer.MAP_CODEC` field order.
        if let Some(color) = &format.color {
            style.insert("color", serialize_color(color));
        }
        if let Some(shadow_color) = format.shadow_color {
            style.insert("shadow_color", shadow_color);
        }
        if let Some(bold) = format.bold {
            style.insert("bold", bold);
        }
        if let Some(italic) = format.italic {
            style.insert("italic", italic);
        }
        if let Some(underlined) = format.underlined {
            style.insert("underlined", underlined);
        }
        if let Some(strikethrough) = format.strikethrough {
            style.insert("strikethrough", strikethrough);
        }
        if let Some(obfuscated) = format.obfuscated {
            style.insert("obfuscated", obfuscated);
        }
        if let Some(font) = &format.font {
            style.insert("font", &**font);
        }
        Self::Styled { style }
    }
}

impl WriteTo for NumberFormat {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match self {
            Self::Blank => VarInt(NUMBER_FORMAT_TYPE_BLANK).write(writer)?,
            Self::Styled { style } => {
                VarInt(NUMBER_FORMAT_TYPE_STYLED).write(writer)?;
                // Vanilla serializes the style through the NBT tag codec, which encodes a
                // root compound tag as its id byte, followed by the entries and `END`.
                writer.write_all(&[NBT_COMPOUND_TAG_ID])?;
                WriteTo::write(style, writer)?;
            }
            Self::Fixed { value } => {
                VarInt(NUMBER_FORMAT_TYPE_FIXED).write(writer)?;
                WriteTo::write(value, writer)?;
            }
        }
        Ok(())
    }
}

/// Vanilla `TextColor#serialize`: the colour name, or `#RRGGBB` for custom colours.
fn serialize_color(color: &Color) -> String {
    match color {
        Color::Aqua => "aqua".to_owned(),
        Color::Black => "black".to_owned(),
        Color::Blue => "blue".to_owned(),
        Color::DarkAqua => "dark_aqua".to_owned(),
        Color::DarkBlue => "dark_blue".to_owned(),
        Color::DarkGray => "dark_gray".to_owned(),
        Color::DarkGreen => "dark_green".to_owned(),
        Color::DarkPurple => "dark_purple".to_owned(),
        Color::DarkRed => "dark_red".to_owned(),
        Color::Gold => "gold".to_owned(),
        Color::Gray => "gray".to_owned(),
        Color::Green => "green".to_owned(),
        Color::LightPurple => "light_purple".to_owned(),
        Color::Red => "red".to_owned(),
        Color::White => "white".to_owned(),
        Color::Yellow => "yellow".to_owned(),
        Color::Rgb(red, green, blue) => format!("#{red:02X}{green:02X}{blue:02X}"),
    }
}

/// Adds, changes or removes a scoreboard objective.
///
/// Matches vanilla's `ClientboundSetObjectivePacket`. The display name, render type
/// and number format are only written for [`ObjectiveMethod::Add`] and
/// [`ObjectiveMethod::Change`].
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_SET_OBJECTIVE)]
pub struct CSetObjective {
    /// Name of the objective.
    pub objective_name: String,
    /// Which change this packet applies.
    pub method: ObjectiveMethod,
    /// Display name shown for the objective.
    pub display_name: TextComponent,
    /// How the scores are rendered.
    pub render_type: ObjectiveRenderType,
    /// Number format used for the scores, if any.
    pub number_format: Option<NumberFormat>,
}

impl CSetObjective {
    /// Builds a packet that creates an objective.
    #[must_use]
    pub fn add(
        objective_name: impl Into<String>,
        display_name: TextComponent,
        render_type: ObjectiveRenderType,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            objective_name: objective_name.into(),
            method: ObjectiveMethod::Add,
            display_name,
            render_type,
            number_format,
        }
    }

    /// Builds a packet that changes an objective's display settings.
    #[must_use]
    pub fn change(
        objective_name: impl Into<String>,
        display_name: TextComponent,
        render_type: ObjectiveRenderType,
        number_format: Option<NumberFormat>,
    ) -> Self {
        Self {
            objective_name: objective_name.into(),
            method: ObjectiveMethod::Change,
            display_name,
            render_type,
            number_format,
        }
    }

    /// Builds a packet that removes an objective.
    #[must_use]
    pub fn remove(objective_name: impl Into<String>) -> Self {
        Self {
            objective_name: objective_name.into(),
            method: ObjectiveMethod::Remove,
            display_name: TextComponent::new(),
            render_type: ObjectiveRenderType::Integer,
            number_format: None,
        }
    }
}

impl WriteTo for CSetObjective {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.objective_name.write_prefixed::<VarInt>(writer)?;
        self.method.write(writer)?;
        if self.method.has_display_settings() {
            WriteTo::write(&self.display_name, writer)?;
            self.render_type.write(writer)?;
            self.number_format.write(writer)?;
        }
        Ok(())
    }
}
