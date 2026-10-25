//! Clientbound map item data packet - decoration and color-patch updates for filled maps.

use std::io::{Result, Write};

use steel_macros::ClientPacket;
use steel_registry::{
    RegistryEntry, map_decoration_type::MapDecorationTypeRef, packets::play::C_MAP_ITEM_DATA,
};
use steel_utils::{
    codec::VarInt,
    serial::{PrefixedWrite, WriteTo},
};
use text_components::TextComponent;

/// A single marker drawn on a filled map (vanilla `MapDecoration`).
///
/// Wire order: `VarInt` `minecraft:map_decoration_type` registry id, three bytes
/// (`x`, `y`, `rot`), then a presence byte with the optional hover name.
#[derive(Clone, Debug)]
pub struct MapDecoration {
    /// Decoration type, written as its `minecraft:map_decoration_type` registry id.
    pub decoration_type: MapDecorationTypeRef,
    /// Horizontal pixel position on the map (vanilla field `x`).
    pub x: i8,
    /// Vertical pixel position on the map (vanilla field `y`).
    pub y: i8,
    /// Rotation of the sprite; only the low 4 bits are sent.
    pub rot: i8,
    /// Optional hover name; absent is a single `false` byte.
    pub name: Option<TextComponent>,
}

impl MapDecoration {
    /// Creates a decoration, truncating `rot` to its low 4 bits like vanilla's constructor.
    #[must_use]
    pub const fn new(
        decoration_type: MapDecorationTypeRef,
        x: i8,
        y: i8,
        rot: i8,
        name: Option<TextComponent>,
    ) -> Self {
        Self {
            decoration_type,
            x,
            y,
            rot: rot & 15,
            name,
        }
    }
}

impl WriteTo for MapDecoration {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.decoration_type.id() as i32).write(writer)?;
        self.x.write(writer)?;
        self.y.write(writer)?;
        // Vanilla masks the rotation in the MapDecoration constructor.
        (self.rot & 15).write(writer)?;
        self.name.write(writer)
    }
}

/// A rectangular patch of map colors (vanilla `MapItemSavedData.MapPatch`).
///
/// Vanilla declares the four bounds as `int` but writes each as one unsigned
/// byte, in the order `width`, `height`, `startX`, `startY` - NOT the record
/// order - followed by a VarInt-prefixed color array of `width * height` bytes.
///
/// A patch whose `width` is 0 is indistinguishable from an absent patch on the
/// wire, which is how vanilla signals "no color patch".
#[derive(Clone, Debug)]
pub struct MapPatch {
    /// Map column of the patch's left edge (vanilla `startX`).
    pub start_x: u8,
    /// Map row of the patch's top edge (vanilla `startY`).
    pub start_y: u8,
    /// Patch width in map pixels.
    pub width: u8,
    /// Patch height in map pixels.
    pub height: u8,
    /// `width * height` map color bytes in row-major order.
    pub map_colors: Vec<u8>,
}

impl WriteTo for MapPatch {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.width.write(writer)?;
        self.height.write(writer)?;
        self.start_x.write(writer)?;
        self.start_y.write(writer)?;
        self.map_colors.write_prefixed::<VarInt>(writer)
    }
}

/// Updates the decorations and/or the color patch of a filled map.
///
/// Wire order: `VarInt` map id, `i8` scale, bool locked, optional decoration list
/// (presence byte + `VarInt` count + entries), then the color patch
/// (`width` byte of 0 means absent, otherwise the [`MapPatch`] payload).
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_MAP_ITEM_DATA)]
pub struct CMapItemData {
    /// Numeric map id; vanilla wraps this in `MapId` and writes only the `VarInt`.
    pub map_id: i32,
    /// Zoom scale of the map.
    pub scale: i8,
    /// Whether the map is locked and can no longer be edited.
    pub locked: bool,
    /// Decoration update, when present; absent writes a single `false` byte.
    pub decorations: Option<Vec<MapDecoration>>,
    /// Color patch update, when present; absent writes a single `0` width byte.
    pub color_patch: Option<MapPatch>,
}

impl CMapItemData {
    /// Creates a map item data packet.
    #[must_use]
    pub const fn new(
        map_id: i32,
        scale: i8,
        locked: bool,
        decorations: Option<Vec<MapDecoration>>,
        color_patch: Option<MapPatch>,
    ) -> Self {
        Self {
            map_id,
            scale,
            locked,
            decorations,
            color_patch,
        }
    }
}

impl WriteTo for CMapItemData {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.map_id).write(writer)?;
        self.scale.write(writer)?;
        self.locked.write(writer)?;
        // `ByteBufCodecs.list().apply(ByteBufCodecs::optional)`.
        match &self.decorations {
            Some(decorations) => {
                true.write(writer)?;
                decorations.write_prefixed::<VarInt>(writer)?;
            }
            None => false.write(writer)?,
        }
        // `MapItemSavedData.MapPatch.STREAM_CODEC` has its own optional framing.
        match &self.color_patch {
            Some(patch) => patch.write(writer)?,
            None => 0u8.write(writer)?,
        }
        Ok(())
    }
}
