//! Map markers stored by [`MapItemSavedData`](super::MapItemSavedData).
//!
//! Mirrors vanilla `MapDecoration`, `MapBanner` and `MapFrame`.

use serde::{Deserialize, Serialize, de::Error as DeError};
use steel_registry::blocks::Block;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::map_decoration_type::MapDecorationTypeRef;
use steel_registry::{DyeColor, REGISTRY, RegistryExt as _, vanilla_map_decoration_types};
use steel_utils::{BlockPos, Downcast as _, Identifier};
use text_components::TextComponent;

use crate::block_entity::entities::BannerBlockEntity;
use crate::world::level_reader::LevelReader;

/// Vanilla `MapDecoration`: one marker drawn on a map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapDecoration {
    /// Marker icon and tracking behaviour.
    pub decoration_type: MapDecorationTypeRef,
    /// Horizontal map pixel, `-128..=127`.
    pub x: i8,
    /// Vertical map pixel, `-128..=127`.
    pub y: i8,
    /// Sprite rotation, masked to four bits.
    pub rot: i8,
    /// Optional hover label.
    pub name: Option<TextComponent>,
}

impl MapDecoration {
    /// Creates a decoration, masking the rotation like vanilla's constructor.
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

/// Vanilla `MapBanner`: a banner that appears on every map of its dimension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapBanner {
    pos: BlockPos,
    color: DyeColor,
    name: Option<TextComponent>,
}

impl MapBanner {
    /// Creates a banner marker.
    #[must_use]
    pub const fn new(pos: BlockPos, color: DyeColor, name: Option<TextComponent>) -> Self {
        Self { pos, color, name }
    }

    /// Vanilla `MapBanner.fromWorld`: reads the banner block entity at `pos`.
    ///
    /// Vanilla requires the `BannerBlockEntity` itself, because the base color
    /// and the custom name both live on it. Steel derives the color from the
    /// banner block's registry key (one block per color) and reads the custom
    /// name from [`BannerBlockEntity`]; a banner without its block entity is
    /// skipped, matching vanilla's `instanceof` check.
    #[must_use]
    pub fn from_world(level: &dyn LevelReader, pos: BlockPos) -> Option<Self> {
        let color = banner_color(level.get_block_state(pos).get_block())?;
        let banner = level.get_block_entity(pos)?;
        let name = banner.downcast_ref::<BannerBlockEntity>()?.custom_name();
        Some(Self::new(pos, color, name))
    }

    /// Returns the banner position.
    #[must_use]
    pub const fn pos(&self) -> BlockPos {
        self.pos
    }

    /// Returns the banner's base color.
    #[must_use]
    pub const fn color(&self) -> DyeColor {
        self.color
    }

    /// Returns the banner's custom name, when one was recorded.
    #[must_use]
    pub const fn name(&self) -> Option<&TextComponent> {
        self.name.as_ref()
    }

    /// Vanilla `MapBanner.getDecoration`: the per-color banner decoration type.
    #[must_use]
    pub const fn decoration_type(&self) -> MapDecorationTypeRef {
        banner_decoration_type(self.color)
    }

    /// Vanilla `MapBanner.getId`.
    #[must_use]
    pub fn id(&self) -> String {
        format!("banner-{},{},{}", self.pos.x(), self.pos.y(), self.pos.z())
    }
}

/// Vanilla `MapFrame`: an item frame holding a map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapFrame {
    pos: BlockPos,
    rotation: i32,
    entity_id: i32,
}

impl MapFrame {
    /// Creates a frame marker.
    #[must_use]
    pub const fn new(pos: BlockPos, rotation: i32, entity_id: i32) -> Self {
        Self {
            pos,
            rotation,
            entity_id,
        }
    }

    /// Returns the frame position.
    #[must_use]
    pub const fn pos(self) -> BlockPos {
        self.pos
    }

    /// Returns the frame rotation in degrees.
    #[must_use]
    pub const fn rotation(self) -> i32 {
        self.rotation
    }

    /// Returns the frame entity id.
    #[must_use]
    pub const fn entity_id(self) -> i32 {
        self.entity_id
    }

    /// Vanilla `MapFrame.getId`: keyed by position.
    #[must_use]
    pub fn id(self) -> String {
        frame_id(self.pos)
    }

    /// Vanilla `MapFrame.getFrameKey`: keyed by entity id.
    #[must_use]
    pub fn frame_key(self) -> String {
        format!("frame-{}", self.entity_id)
    }
}

/// Vanilla `MapFrame.frameId(pos)`.
#[must_use]
pub fn frame_id(pos: BlockPos) -> String {
    format!("frame-{},{},{}", pos.x(), pos.y(), pos.z())
}

/// Vanilla `MapBanner.getDecoration` color mapping.
#[must_use]
pub const fn banner_decoration_type(color: DyeColor) -> MapDecorationTypeRef {
    match color {
        DyeColor::White => &vanilla_map_decoration_types::BANNER_WHITE,
        DyeColor::Orange => &vanilla_map_decoration_types::BANNER_ORANGE,
        DyeColor::Magenta => &vanilla_map_decoration_types::BANNER_MAGENTA,
        DyeColor::LightBlue => &vanilla_map_decoration_types::BANNER_LIGHT_BLUE,
        DyeColor::Yellow => &vanilla_map_decoration_types::BANNER_YELLOW,
        DyeColor::Lime => &vanilla_map_decoration_types::BANNER_LIME,
        DyeColor::Pink => &vanilla_map_decoration_types::BANNER_PINK,
        DyeColor::Gray => &vanilla_map_decoration_types::BANNER_GRAY,
        DyeColor::LightGray => &vanilla_map_decoration_types::BANNER_LIGHT_GRAY,
        DyeColor::Cyan => &vanilla_map_decoration_types::BANNER_CYAN,
        DyeColor::Purple => &vanilla_map_decoration_types::BANNER_PURPLE,
        DyeColor::Blue => &vanilla_map_decoration_types::BANNER_BLUE,
        DyeColor::Brown => &vanilla_map_decoration_types::BANNER_BROWN,
        DyeColor::Green => &vanilla_map_decoration_types::BANNER_GREEN,
        DyeColor::Red => &vanilla_map_decoration_types::BANNER_RED,
        DyeColor::Black => &vanilla_map_decoration_types::BANNER_BLACK,
    }
}

/// Returns the banner color of a banner block, or `None` for other blocks.
///
/// Vanilla stores this on the per-instance `BannerBlock.color` field; Steel's
/// banner blocks are one registry entry per color, so the color is read from
/// the registry key (`white_banner`, `white_wall_banner`, ...).
#[must_use]
pub fn banner_color(block: &Block) -> Option<DyeColor> {
    let key = block.key.path.as_ref();
    let color_name = key
        .strip_suffix("_wall_banner")
        .or_else(|| key.strip_suffix("_banner"))?;
    DyeColor::from_serialized_name(color_name)
}

/// Resolves a persisted decoration-type key back to its registry entry.
#[must_use]
pub fn decoration_type_by_key(key: &Identifier) -> Option<MapDecorationTypeRef> {
    REGISTRY.map_decoration_types.by_key(key)
}

impl Serialize for MapBanner {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        BannerWire {
            pos: [self.pos.x(), self.pos.y(), self.pos.z()],
            color: DyeColorName(self.color),
            name: self.name.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MapBanner {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = BannerWire::deserialize(deserializer)?;
        Ok(Self::new(
            BlockPos::new(wire.pos[0], wire.pos[1], wire.pos[2]),
            wire.color.0,
            wire.name,
        ))
    }
}

/// Vanilla `MapBanner.CODEC`: `{pos:[x,y,z], color, name}`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BannerWire {
    pos: [i32; 3],
    #[serde(default = "default_banner_color")]
    color: DyeColorName,
    #[serde(default)]
    name: Option<TextComponent>,
}

/// Serde adapter for [`DyeColor`] using vanilla's serialized color names.
///
/// Vanilla writes a banner color as its name (`white`, `light_blue`, ...); the
/// registry enum has no `serde` derive of its own, so the wire field wraps it.
struct DyeColorName(DyeColor);

impl Serialize for DyeColorName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialized_name().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DyeColorName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        DyeColor::from_serialized_name(&name)
            .map(DyeColorName)
            .ok_or_else(|| DeError::custom(format!("unknown dye color: {name}")))
    }
}

const fn default_banner_color() -> DyeColorName {
    DyeColorName(DyeColor::White)
}

impl Serialize for MapFrame {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        FrameWire {
            pos: [self.pos.x(), self.pos.y(), self.pos.z()],
            rotation: self.rotation,
            entity_id: self.entity_id,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MapFrame {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = FrameWire::deserialize(deserializer)?;
        Ok(Self::new(
            BlockPos::new(wire.pos[0], wire.pos[1], wire.pos[2]),
            wire.rotation,
            wire.entity_id,
        ))
    }
}

/// Vanilla `MapFrame.CODEC`: `{pos:[x,y,z], rotation, entity_id}`.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameWire {
    pos: [i32; 3],
    rotation: i32,
    entity_id: i32,
}

#[cfg(test)]
mod tests {
    use super::{
        MapBanner, MapFrame, banner_color, banner_decoration_type, decoration_type_by_key,
    };
    use steel_registry::{DyeColor, vanilla_blocks};
    use steel_utils::BlockPos;

    #[test]
    fn banner_colors_are_read_from_registry_keys() {
        assert_eq!(
            banner_color(&vanilla_blocks::WHITE_BANNER),
            Some(DyeColor::White)
        );
        assert_eq!(
            banner_color(&vanilla_blocks::LIGHT_BLUE_WALL_BANNER),
            Some(DyeColor::LightBlue)
        );
        assert_eq!(banner_color(&vanilla_blocks::STONE), None);
    }

    #[test]
    fn banner_decoration_types_cover_every_color() {
        for color in DyeColor::VALUES {
            let decoration = banner_decoration_type(color);
            assert_eq!(
                decoration.key.path.as_ref(),
                format!("banner_{}", color.serialized_name())
            );
            assert_eq!(
                decoration_type_by_key(&decoration.key).map(|found| found.key.clone()),
                Some(decoration.key.clone())
            );
        }
    }

    #[test]
    fn banner_and_frame_ids_use_position_keys() {
        let pos = BlockPos::new(1, 2, 3);
        let banner = MapBanner::new(pos, DyeColor::Red, None);
        assert_eq!(banner.id(), "banner-1,2,3");
        let frame = MapFrame::new(pos, 90, 42);
        assert_eq!(frame.id(), "frame-1,2,3");
        assert_eq!(frame.frame_key(), "frame-42");
    }

    #[test]
    fn banner_round_trips_through_serde() {
        let banner = MapBanner::new(BlockPos::new(1, 2, 3), DyeColor::Red, None);
        let json = serde_json::to_string(&banner).expect("banner should serialize");
        let restored: MapBanner = serde_json::from_str(&json).expect("banner should deserialize");
        assert_eq!(restored, banner);
    }
}
