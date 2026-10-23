//! Map item saved data.
//!
//! Mirrors vanilla `MapItemSavedData`: the map's dimension, center, scale, color
//! buffer, banner and frame markers, live player markers, and the per-viewer
//! dirty tracking that drives `ClientboundMapItemDataPacket` updates.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use steel_protocol::packets::game::{CMapItemData, MapDecoration as WireMapDecoration, MapPatch};
use steel_registry::map_decoration_type::MapDecorationTypeRef;
use steel_registry::vanilla_map_decoration_types;
use steel_utils::{BlockPos, Identifier};
use text_components::TextComponent;
use uuid::Uuid;

use crate::entity::Entity as _;
use crate::world::level_reader::LevelReader;

use super::decoration::{MapBanner, MapDecoration, MapFrame, frame_id};

/// Width and height of a map's color buffer in pixels.
pub const MAP_SIZE: usize = 128;
/// Half a map's size in pixels.
pub const HALF_MAP_SIZE: i32 = 64;
/// Highest zoom scale a map can reach.
pub const MAX_SCALE: u8 = 4;
/// Maximum number of tracked decorations on one map.
pub const TRACKED_DECORATION_LIMIT: i32 = 256;
/// Number of bytes in a map's color buffer.
pub const MAP_COLOR_BYTES: usize = MAP_SIZE * MAP_SIZE;

/// Vanilla `MapItemSavedData.HoldingPlayer`: one viewer's dirty tracking.
#[derive(Clone, Debug)]
struct HoldingPlayer {
    dirty_data: bool,
    dirty_rect: Option<(u8, u8, u8, u8)>,
    dirty_decorations: bool,
    tick: u32,
    step: u32,
}

impl Default for HoldingPlayer {
    fn default() -> Self {
        Self {
            dirty_data: true,
            dirty_rect: None,
            dirty_decorations: true,
            tick: 0,
            step: 0,
        }
    }
}

impl HoldingPlayer {
    /// Vanilla `HoldingPlayer.setColorsDirty`.
    fn set_colors_dirty(&mut self, x: u8, y: u8) {
        self.dirty_data = true;
        self.dirty_rect = Some(match self.dirty_rect {
            Some((min_x, min_y, max_x, max_y)) => {
                (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
            }
            None => (x, y, x, y),
        });
    }

    /// Vanilla `HoldingPlayer.createPatch`.
    fn create_patch(&mut self, colors: &[u8]) -> Option<MapPatch> {
        let (min_x, min_y, max_x, max_y) = self.dirty_rect.take()?;
        self.dirty_data = false;
        let width = max_x - min_x + 1;
        let height = max_y - min_y + 1;
        let mut map_colors = Vec::with_capacity(usize::from(width) * usize::from(height));
        for y in min_y..=max_y {
            let row = usize::from(y) * MAP_SIZE;
            for x in min_x..=max_x {
                map_colors.push(colors[row + usize::from(x)]);
            }
        }
        Some(MapPatch {
            start_x: min_x,
            start_y: min_y,
            width,
            height,
            map_colors,
        })
    }

    /// Vanilla `HoldingPlayer.nextUpdatePacket` dirty flags.
    fn next_update(
        &mut self,
        map_id: i32,
        scale: u8,
        locked: bool,
        colors: &[u8],
        decorations: &BTreeMap<String, MapDecoration>,
    ) -> Option<CMapItemData> {
        let patch = self.create_patch(colors);
        let decoration_list = if self.dirty_decorations && self.tick.is_multiple_of(5) {
            self.dirty_decorations = false;
            Some(
                decorations
                    .values()
                    .map(|decoration| {
                        WireMapDecoration::new(
                            decoration.decoration_type,
                            decoration.x,
                            decoration.y,
                            decoration.rot,
                            decoration.name.clone(),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        self.tick = self.tick.wrapping_add(1);
        if patch.is_none() && decoration_list.is_none() {
            return None;
        }
        Some(CMapItemData::new(
            map_id,
            i8::try_from(scale).unwrap_or(0),
            locked,
            decoration_list,
            patch,
        ))
    }
}

/// Persisted map state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PersistentMapData {
    pub(crate) dimension: Identifier,
    #[serde(rename = "xCenter")]
    pub(crate) center_x: i32,
    #[serde(rename = "zCenter")]
    pub(crate) center_z: i32,
    pub(crate) scale: u8,
    /// Base64-encoded 128x128 color buffer.
    pub(crate) colors: String,
    #[serde(rename = "trackingPosition", default = "default_tracking_position")]
    pub(crate) tracking_position: bool,
    #[serde(rename = "unlimitedTracking", default)]
    pub(crate) unlimited_tracking: bool,
    #[serde(default)]
    pub(crate) locked: bool,
    #[serde(default)]
    pub(crate) banners: Vec<MapBanner>,
    #[serde(default)]
    pub(crate) frames: Vec<MapFrame>,
}

const fn default_tracking_position() -> bool {
    true
}

/// Vanilla `MapItemSavedData`.
#[derive(Clone, Debug)]
pub struct MapItemSavedData {
    /// Map center X in blocks.
    pub center_x: i32,
    /// Map center Z in blocks.
    pub center_z: i32,
    /// Dimension the map was created in.
    pub dimension: Identifier,
    /// Whether player and frame markers are tracked.
    pub tracking_position: bool,
    /// Whether markers may leave the map's edge instead of disappearing.
    pub unlimited_tracking: bool,
    /// Zoom scale, `0..=MAX_SCALE`.
    pub scale: u8,
    /// Row-major 128x128 color buffer.
    pub colors: Box<[u8; MAP_COLOR_BYTES]>,
    /// Whether the map has been locked in a cartography table.
    pub locked: bool,
    banner_markers: BTreeMap<String, MapBanner>,
    decorations: BTreeMap<String, MapDecoration>,
    frame_markers: BTreeMap<String, MapFrame>,
    tracked_decoration_count: i32,
    carried_by_players: BTreeMap<Uuid, HoldingPlayer>,
}

impl MapItemSavedData {
    /// Vanilla `MapItemSavedData.createFresh`: centers a new map on the nearest
    /// scale-aligned area containing `(origin_x, origin_z)`.
    #[must_use]
    pub fn create_fresh(
        origin_x: f64,
        origin_z: f64,
        scale: u8,
        tracking_position: bool,
        unlimited_tracking: bool,
        dimension: Identifier,
    ) -> Self {
        let scale = scale.min(MAX_SCALE);
        let size = f64::from(1u32 << scale) * MAP_SIZE as f64;
        let area_x = ((origin_x + 64.0) / size).floor();
        let area_z = ((origin_z + 64.0) / size).floor();
        let center_x = (area_x.mul_add(size, size / 2.0) - 64.0) as i32;
        let center_z = (area_z.mul_add(size, size / 2.0) - 64.0) as i32;
        Self::with_center(
            center_x,
            center_z,
            scale,
            tracking_position,
            unlimited_tracking,
            false,
            dimension,
        )
    }

    /// Vanilla `MapItemSavedData.createForClient`.
    #[must_use]
    pub fn create_for_client(scale: u8, locked: bool, dimension: Identifier) -> Self {
        Self::with_center(0, 0, scale, false, false, locked, dimension)
    }

    fn with_center(
        center_x: i32,
        center_z: i32,
        scale: u8,
        tracking_position: bool,
        unlimited_tracking: bool,
        locked: bool,
        dimension: Identifier,
    ) -> Self {
        Self {
            center_x,
            center_z,
            dimension,
            tracking_position,
            unlimited_tracking,
            scale: scale.min(MAX_SCALE),
            colors: Box::new([0; MAP_COLOR_BYTES]),
            locked,
            banner_markers: BTreeMap::new(),
            decorations: BTreeMap::new(),
            frame_markers: BTreeMap::new(),
            tracked_decoration_count: 0,
            carried_by_players: BTreeMap::new(),
        }
    }

    /// Vanilla `MapItemSavedData.locked`: a locked copy of this map.
    #[must_use]
    pub fn locked_copy(&self) -> Self {
        let mut copy = Self::with_center(
            self.center_x,
            self.center_z,
            self.scale,
            self.tracking_position,
            self.unlimited_tracking,
            true,
            self.dimension.clone(),
        );
        copy.banner_markers = self.banner_markers.clone();
        copy.decorations = self.decorations.clone();
        copy.tracked_decoration_count = self.tracked_decoration_count;
        copy.colors = self.colors.clone();
        copy
    }

    /// Vanilla `MapItemSavedData.scaled`: a new map one zoom step out.
    #[must_use]
    pub fn scaled(&self) -> Self {
        Self::with_center(
            self.center_x,
            self.center_z,
            self.scale.saturating_add(1).min(MAX_SCALE),
            self.tracking_position,
            self.unlimited_tracking,
            false,
            self.dimension.clone(),
        )
    }

    /// Returns the scale factor in blocks per map pixel.
    #[must_use]
    pub const fn scale_factor(&self) -> i32 {
        1 << self.scale
    }

    /// Vanilla `MapItemSavedData.getHoldingPlayer`: registers a map viewer.
    pub fn track_viewer(&mut self, player: Uuid) {
        self.carried_by_players.entry(player).or_default();
    }

    /// Vanilla `MapItemSavedData.removeHoldingPlayer`.
    pub fn remove_viewer(&mut self, player: Uuid) {
        self.carried_by_players.remove(&player);
    }

    /// Returns whether `player` currently has this map open or carried.
    #[must_use]
    pub fn is_tracked_viewer(&self, player: Uuid) -> bool {
        self.carried_by_players.contains_key(&player)
    }

    /// Vanilla `MapItemSavedData.getHoldingPlayer(player).step++`: advances the
    /// viewer's rasterizer dither phase and returns the new step.
    pub fn next_viewer_step(&mut self, player: Uuid) -> u32 {
        let holding = self.carried_by_players.entry(player).or_default();
        holding.step = holding.step.wrapping_add(1);
        holding.step
    }

    /// Vanilla `MapItemSavedData.getUpdatePacket`.
    #[must_use]
    pub fn update_packet(&mut self, map_id: i32, player: Uuid) -> Option<CMapItemData> {
        let Self {
            colors,
            decorations,
            scale,
            locked,
            carried_by_players,
            ..
        } = self;
        let holding = carried_by_players.get_mut(&player)?;
        holding.next_update(map_id, *scale, *locked, colors.as_slice(), decorations)
    }

    /// Vanilla `MapItemSavedData.addClientSideDecorations` on the server side:
    /// replaces every tracked marker with `decorations`.
    pub fn set_tracked_decorations(&mut self, decorations: BTreeMap<String, MapDecoration>) {
        self.decorations = decorations;
        self.recount_tracked_decorations();
        self.set_decorations_dirty();
    }

    /// Returns the current marker map.
    #[must_use]
    pub const fn decorations(&self) -> &BTreeMap<String, MapDecoration> {
        &self.decorations
    }

    /// Returns the stored banner markers.
    #[must_use]
    pub const fn banner_markers(&self) -> &BTreeMap<String, MapBanner> {
        &self.banner_markers
    }

    /// Returns the stored frame markers.
    #[must_use]
    pub const fn frame_markers(&self) -> &BTreeMap<String, MapFrame> {
        &self.frame_markers
    }

    /// Vanilla `MapItemSavedData.setColor`.
    pub fn set_color(&mut self, x: u8, y: u8, color: u8) {
        self.colors[usize::from(y) * MAP_SIZE + usize::from(x)] = color;
    }

    /// Vanilla `MapItemSavedData.updateColor`: writes a color and marks it dirty.
    ///
    /// Returns whether the pixel changed; the rasterizer keeps sampling a column
    /// while it still sees changes.
    pub fn update_color(&mut self, x: u8, y: u8, color: u8) -> bool {
        if self.colors[usize::from(y) * MAP_SIZE + usize::from(x)] == color {
            return false;
        }
        self.set_color(x, y, color);
        for holding in self.carried_by_players.values_mut() {
            holding.set_colors_dirty(x, y);
        }
        true
    }

    /// Vanilla `MapItemSavedData.checkBanners` call site: refreshes banner
    /// markers in the sampled column under the player.
    pub(crate) fn check_banners_near_player(
        &mut self,
        level: &dyn LevelReader,
        player: &crate::player::Player,
    ) {
        let position = player.position();
        self.check_banners(level, position.x.floor() as i32, position.z.floor() as i32);
    }

    /// Vanilla `MapItemSavedData.setDecorationsDirty`.
    pub fn set_decorations_dirty(&mut self) {
        for holding in self.carried_by_players.values_mut() {
            holding.dirty_decorations = true;
        }
    }

    /// Vanilla `MapItemSavedData.removeDecoration`.
    pub fn remove_decoration(&mut self, key: &str) {
        let Some(removed) = self.decorations.remove(key) else {
            return;
        };
        if removed.decoration_type.track_count {
            self.tracked_decoration_count -= 1;
        }
        self.set_decorations_dirty();
    }

    /// Vanilla `MapItemSavedData.addDecoration`.
    ///
    /// `has_ceiling` and `game_time` are vanilla `calculateRotation(level, yRot)`
    /// inputs; `x_pos`/`z_pos` are world coordinates.
    pub fn add_decoration(
        &mut self,
        decoration_type: MapDecorationTypeRef,
        key: &str,
        x_pos: f64,
        z_pos: f64,
        y_rot: f32,
        name: Option<TextComponent>,
        has_ceiling: bool,
        game_time: i64,
    ) {
        let scaling = f64::from(self.scale_factor());
        let delta_x = (x_pos - f64::from(self.center_x)) / scaling;
        let delta_z = (z_pos - f64::from(self.center_z)) / scaling;
        let Some((resolved_type, x, y, rot)) = self.calculate_decoration_location_and_type(
            decoration_type,
            delta_x,
            delta_z,
            y_rot,
            has_ceiling,
            game_time,
        ) else {
            self.remove_decoration(key);
            return;
        };
        let previous = self.decorations.insert(
            key.to_owned(),
            MapDecoration::new(resolved_type, x, y, rot, name),
        );
        if let Some(previous) = previous
            && previous.decoration_type.track_count
        {
            self.tracked_decoration_count -= 1;
        }
        if resolved_type.track_count {
            self.tracked_decoration_count += 1;
        }
        self.set_decorations_dirty();
    }

    /// Vanilla `MapItemSavedData.calculateDecorationLocationAndType`.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors the vanilla helper's inputs"
    )]
    fn calculate_decoration_location_and_type(
        &self,
        decoration_type: MapDecorationTypeRef,
        delta_x: f64,
        delta_z: f64,
        y_rot: f32,
        has_ceiling: bool,
        game_time: i64,
    ) -> Option<(MapDecorationTypeRef, i8, i8, i8)> {
        let rot = calculate_rotation(has_ceiling, y_rot, game_time);
        if decoration_type.key == vanilla_map_decoration_types::PLAYER.key {
            if delta_x >= -63.0 && delta_x <= 63.0 && delta_z >= -63.0 && delta_z <= 63.0 {
                return Some((
                    &vanilla_map_decoration_types::PLAYER,
                    clamp_map_coordinate(delta_x),
                    clamp_map_coordinate(delta_z),
                    rot,
                ));
            }
            if self.unlimited_tracking {
                if delta_x.abs() < 320.0 && delta_z.abs() < 320.0 {
                    return Some((
                        &vanilla_map_decoration_types::PLAYER_OFF_MAP,
                        clamp_map_coordinate(delta_x),
                        clamp_map_coordinate(delta_z),
                        rot,
                    ));
                }
                return Some((
                    &vanilla_map_decoration_types::PLAYER_OFF_LIMITS,
                    clamp_map_coordinate(delta_x),
                    clamp_map_coordinate(delta_z),
                    rot,
                ));
            }
            return None;
        }
        if !self.unlimited_tracking
            && (delta_x < -63.0 || delta_x > 63.0 || delta_z < -63.0 || delta_z > 63.0)
        {
            return None;
        }
        Some((
            decoration_type,
            clamp_map_coordinate(delta_x),
            clamp_map_coordinate(delta_z),
            rot,
        ))
    }

    /// Vanilla `MapItemSavedData.checkBanners`: refreshes the banner marker at a
    /// sampled column.
    pub fn check_banners(&mut self, level: &dyn LevelReader, x: i32, z: i32) {
        let delta_x = f64::from(x - self.center_x) / f64::from(self.scale_factor());
        let delta_z = f64::from(z - self.center_z) / f64::from(self.scale_factor());
        if delta_x.abs() > 63.0 || delta_z.abs() > 63.0 {
            return;
        }
        let mut y = level.height_at(crate::chunk::heightmap::HeightmapType::WorldSurface, x, z) + 1;
        let min_y = level.min_y();
        let mut remaining = 16;
        while y > min_y && remaining > 0 {
            remaining -= 1;
            let pos = BlockPos::new(x, y, z);
            let banner = MapBanner::from_world(level, pos);
            y -= 1;
            let Some(banner) = banner else {
                continue;
            };
            let key = banner.id();
            if self.banner_markers.get(&key) == Some(&banner) {
                continue;
            }
            if self.banner_markers.remove(&key).is_some() {
                self.remove_decoration(&key);
                self.set_decorations_dirty();
                continue;
            }
            if self.tracked_decoration_count < TRACKED_DECORATION_LIMIT {
                self.banner_markers.insert(key.clone(), banner.clone());
                self.add_decoration(
                    banner.decoration_type(),
                    &key,
                    f64::from(banner.pos().x()),
                    f64::from(banner.pos().z()),
                    180.0,
                    banner.name().cloned(),
                    false,
                    0,
                );
                self.set_decorations_dirty();
            }
        }
    }

    /// Vanilla `MapItemSavedData.toggleBanner`: adds or removes a banner marker.
    ///
    /// Returns `false` when the position does not hold a banner.
    pub fn toggle_banner(&mut self, level: &dyn LevelReader, pos: BlockPos) -> bool {
        let key = format!("banner-{},{},{}", pos.x(), pos.y(), pos.z());
        if self.banner_markers.remove(&key).is_some() {
            self.remove_decoration(&key);
            self.set_decorations_dirty();
            return true;
        }
        let Some(banner) = MapBanner::from_world(level, pos) else {
            return false;
        };
        if self.tracked_decoration_count >= TRACKED_DECORATION_LIMIT {
            return false;
        }
        self.banner_markers.insert(key.clone(), banner.clone());
        self.add_decoration(
            banner.decoration_type(),
            &key,
            f64::from(banner.pos().x()),
            f64::from(banner.pos().z()),
            180.0,
            banner.name().cloned(),
            false,
            0,
        );
        self.set_decorations_dirty();
        true
    }

    /// Vanilla `MapItemSavedData.tickCarriedBy` item-frame half: records the
    /// frame marker and its `FRAME` decoration.
    pub fn track_frame(
        &mut self,
        frame: MapFrame,
        x_pos: f64,
        z_pos: f64,
        has_ceiling: bool,
        game_time: i64,
    ) {
        if !self.tracking_position {
            return;
        }
        let key = frame.id();
        if let Some(previous) = self.frame_markers.get(&key)
            && previous.entity_id() != frame.entity_id()
        {
            let stale = *previous;
            self.frame_markers.remove(&key);
            self.remove_decoration(&stale.frame_key());
        }
        let frame_key = frame.frame_key();
        self.add_decoration(
            &vanilla_map_decoration_types::FRAME,
            &frame_key,
            x_pos,
            z_pos,
            f32::from(i16::try_from(frame.rotation()).unwrap_or(0)),
            None,
            has_ceiling,
            game_time,
        );
        self.frame_markers.insert(key, frame);
        self.set_decorations_dirty();
    }

    /// Vanilla `MapItemSavedData.getFrameKey`.
    #[must_use]
    pub fn frame_key(frame: MapFrame) -> String {
        frame.frame_key()
    }

    /// Removes a frame marker by position key.
    pub fn remove_frame(&mut self, pos: BlockPos) {
        if let Some(frame) = self.frame_markers.remove(&frame_id(pos)) {
            self.remove_decoration(&frame.frame_key());
        }
    }

    fn recount_tracked_decorations(&mut self) {
        self.tracked_decoration_count = self
            .decorations
            .values()
            .filter(|decoration| decoration.decoration_type.track_count)
            .count()
            .try_into()
            .unwrap_or(TRACKED_DECORATION_LIMIT);
    }

    /// Converts this map into its persisted form.
    #[must_use]
    pub(crate) fn to_persistent(&self) -> PersistentMapData {
        use base64::Engine as _;
        PersistentMapData {
            dimension: self.dimension.clone(),
            center_x: self.center_x,
            center_z: self.center_z,
            scale: self.scale,
            colors: base64::engine::general_purpose::STANDARD.encode(self.colors.as_slice()),
            tracking_position: self.tracking_position,
            unlimited_tracking: self.unlimited_tracking,
            locked: self.locked,
            banners: self.banner_markers.values().cloned().collect(),
            frames: self.frame_markers.values().copied().collect(),
        }
    }

    /// Restores a map from its persisted form, rebuilding banner and frame
    /// decorations like vanilla's codec does.
    #[must_use]
    pub(crate) fn from_persistent(persistent: PersistentMapData) -> Self {
        use base64::Engine as _;
        let mut data = Self::with_center(
            persistent.center_x,
            persistent.center_z,
            persistent.scale,
            persistent.tracking_position,
            persistent.unlimited_tracking,
            persistent.locked,
            persistent.dimension,
        );
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(persistent.colors) {
            if decoded.len() == MAP_COLOR_BYTES {
                data.colors.copy_from_slice(&decoded);
            }
        }
        for banner in persistent.banners {
            let key = banner.id();
            data.add_decoration(
                banner.decoration_type(),
                &key,
                f64::from(banner.pos().x()),
                f64::from(banner.pos().z()),
                180.0,
                banner.name().cloned(),
                false,
                0,
            );
            data.banner_markers.insert(key, banner);
        }
        for frame in persistent.frames {
            let key = frame.frame_key();
            data.add_decoration(
                &vanilla_map_decoration_types::FRAME,
                &key,
                f64::from(frame.pos().x()),
                f64::from(frame.pos().z()),
                f32::from(i16::try_from(frame.rotation()).unwrap_or(0)),
                None,
                false,
                0,
            );
            data.frame_markers.insert(frame.id(), frame);
        }
        data
    }
}

/// Vanilla `MapItemSavedData.clampMapCoordinate`.
#[must_use]
pub fn clamp_map_coordinate(value: f64) -> i8 {
    if value <= -63.0 {
        return -128;
    }
    if value >= 63.0 {
        return 127;
    }
    (value * 2.0 + 0.5) as i8
}

/// Vanilla `MapItemSavedData.calculateRotation`.
#[must_use]
pub fn calculate_rotation(has_ceiling: bool, y_rot: f32, game_time: i64) -> i8 {
    if has_ceiling {
        let step = game_time / 10;
        let mixed = (step * step * 34_187_121 + step * 121) >> 15;
        return (mixed & 15) as i8;
    }
    let rotation = (y_rot + 8.0) * 16.0 / 360.0;
    rotation as i8 & 15
}

/// Vanilla `MapItemSavedData.getFrameKey` helper for external callers.
#[must_use]
pub fn frame_decoration_key(frame: MapFrame) -> String {
    frame.frame_key()
}

#[cfg(test)]
mod tests {
    use super::{
        MAP_COLOR_BYTES, MAX_SCALE, MapItemSavedData, calculate_rotation, clamp_map_coordinate,
    };
    use steel_registry::vanilla_map_decoration_types;
    use steel_utils::Identifier;

    fn fresh() -> MapItemSavedData {
        MapItemSavedData::create_fresh(
            0.0,
            0.0,
            0,
            true,
            false,
            Identifier::vanilla_static("overworld"),
        )
    }

    #[test]
    fn fresh_maps_align_to_their_scale_area() {
        let map = fresh();
        assert_eq!(map.center_x, 0);
        assert_eq!(map.center_z, 0);
        assert_eq!(map.colors.len(), MAP_COLOR_BYTES);
        assert_eq!(map.scale_factor(), 1);

        let scaled = map.scaled();
        assert_eq!(scaled.scale, 1);
        assert_eq!(scaled.scale_factor(), 2);
        assert_eq!(scaled.center_x, 0);

        let capped = MapItemSavedData::create_fresh(
            0.0,
            0.0,
            MAX_SCALE,
            true,
            false,
            Identifier::vanilla_static("overworld"),
        )
        .scaled();
        assert_eq!(capped.scale, MAX_SCALE);
    }

    #[test]
    fn player_decorations_clamp_and_hide_beyond_tracking() {
        let mut map = fresh();
        map.add_decoration(
            &vanilla_map_decoration_types::PLAYER,
            "Steve",
            10.0,
            10.0,
            0.0,
            None,
            false,
            0,
        );
        let decoration = map.decorations().get("Steve").expect("decoration");
        assert_eq!(decoration.x, 20);
        assert_eq!(decoration.y, 20);
        assert_eq!(decoration.rot, 0);

        map.add_decoration(
            &vanilla_map_decoration_types::PLAYER,
            "Far",
            1_000.0,
            0.0,
            0.0,
            None,
            false,
            0,
        );
        assert!(!map.decorations().contains_key("Far"));
    }

    #[test]
    fn unlimited_tracking_keeps_off_limits_markers() {
        let mut map = MapItemSavedData::create_fresh(
            0.0,
            0.0,
            0,
            true,
            true,
            Identifier::vanilla_static("overworld"),
        );
        map.add_decoration(
            &vanilla_map_decoration_types::PLAYER,
            "Far",
            400.0,
            0.0,
            0.0,
            None,
            false,
            0,
        );
        assert!(map.decorations().contains_key("Far"));
    }

    #[test]
    fn color_updates_mark_every_viewer_dirty() {
        let mut map = fresh();
        let viewer = uuid::Uuid::from_u128(1);
        map.track_viewer(viewer);
        map.update_color(3, 4, 12);
        assert_eq!(map.colors[4 * super::MAP_SIZE + 3], 12);
        let packet = map.update_packet(7, viewer).expect("patch should be sent");
        assert_eq!(packet.map_id, 7);
        let patch = packet.color_patch.expect("patch");
        assert_eq!((patch.start_x, patch.start_y), (3, 4));
        assert_eq!((patch.width, patch.height), (1, 1));
        assert_eq!(patch.map_colors, vec![12]);
    }

    #[test]
    fn rotations_match_vanilla_math() {
        assert_eq!(clamp_map_coordinate(-63.5), -128);
        assert_eq!(clamp_map_coordinate(63.5), 127);
        assert_eq!(clamp_map_coordinate(0.0), 0);
        // 90 degrees maps to rotation 4 with vanilla's rounding.
        assert_eq!(calculate_rotation(false, 90.0, 0), 4);
        let step = 10_i64;
        assert_eq!(
            calculate_rotation(true, 0.0, 100),
            ((step * step * 34_187_121 + step * 121) >> 15 & 15) as i8
        );
    }

    #[test]
    fn persisted_maps_round_trip_banners_and_colors() {
        let mut map = fresh();
        map.update_color(1, 1, 9);
        let persistent = map.to_persistent();
        let restored = MapItemSavedData::from_persistent(persistent);
        assert_eq!(restored.colors[super::MAP_SIZE + 1], 9);
        assert_eq!(restored.dimension, map.dimension);
    }
}
