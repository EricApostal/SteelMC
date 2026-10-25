//! World border state and collision helpers.

use std::{error::Error, fmt, sync::Arc};

use glam::DVec3;
use steel_utils::{BlockPos, WorldAabb};

use crate::level_data::WorldBorderData;

pub(crate) const MAX_SIZE: f64 = 5.999_997E7_f32 as f64;
pub(crate) const MAX_CENTER_COORDINATE: f64 = 2.999_998_4E7;
const DEFAULT_ABSOLUTE_MAX_SIZE: i32 = 29_999_984;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BorderStatus {
    Growing,
    Shrinking,
    Stationary,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldBorderSnapshot {
    pub(crate) center_x: f64,
    pub(crate) center_z: f64,
    pub(crate) old_size: f64,
    pub(crate) new_size: f64,
    pub(crate) lerp_time: i64,
    pub(crate) absolute_max_size: i32,
    pub(crate) warning_blocks: i32,
    pub(crate) warning_time: i32,
    pub(crate) damage_per_block: f64,
    pub(crate) safe_zone: f64,
    pub(crate) min_x: f64,
    pub(crate) min_z: f64,
    pub(crate) max_x: f64,
    pub(crate) max_z: f64,
    pub(crate) status: BorderStatus,
}

impl WorldBorderSnapshot {
    #[must_use]
    pub(crate) fn is_block_within_bounds(self, pos: BlockPos) -> bool {
        self.is_within_bounds_with_margin(f64::from(pos.x()), f64::from(pos.z()), 0.0)
    }

    #[must_use]
    pub(crate) fn is_within_bounds_with_margin(self, x: f64, z: f64, margin: f64) -> bool {
        x >= self.min_x - margin
            && x < self.max_x + margin
            && z >= self.min_z - margin
            && z < self.max_z + margin
    }

    #[must_use]
    pub(crate) fn clamp_to_bounds(self, x: f64, y: f64, z: f64) -> BlockPos {
        BlockPos::from(self.clamp_vec3_to_bound(DVec3::new(x, y, z)))
    }

    #[must_use]
    pub(crate) fn clamp_vec3_to_bound(self, position: DVec3) -> DVec3 {
        let epsilon = f64::from(1.0E-5_f32);
        DVec3::new(
            clamp_f64(position.x, self.min_x, self.max_x - epsilon),
            position.y,
            clamp_f64(position.z, self.min_z, self.max_z - epsilon),
        )
    }

    #[must_use]
    pub(crate) fn is_within_bounds(self, bounding_box: WorldAabb) -> bool {
        let epsilon = f64::from(1.0E-5_f32);
        self.is_within_bounds_with_margin(bounding_box.min_x(), bounding_box.min_z(), 0.0)
            && self.is_within_bounds_with_margin(
                bounding_box.max_x() - epsilon,
                bounding_box.max_z() - epsilon,
                0.0,
            )
    }

    #[must_use]
    pub(crate) fn distance_to_border(self, x: f64, z: f64) -> f64 {
        let from_north = z - self.min_z;
        let from_south = self.max_z - z;
        let from_west = x - self.min_x;
        let from_east = self.max_x - x;
        from_west.min(from_east).min(from_north).min(from_south)
    }

    #[must_use]
    pub(crate) fn is_inside_close_to_border(self, x: f64, z: f64, bounding_box: WorldAabb) -> bool {
        let box_max = bounding_box
            .width()
            .abs()
            .max(bounding_box.depth().abs())
            .max(1.0);
        self.distance_to_border(x, z) < box_max * 2.0
            && self.is_within_bounds_with_margin(x, z, box_max)
    }

    #[must_use]
    pub(crate) fn outside_damage_amount(
        self,
        x: f64,
        z: f64,
        bounding_box: WorldAabb,
    ) -> Option<f32> {
        if self.is_within_bounds(bounding_box) {
            return None;
        }

        let distance = self.distance_to_border(x, z) + self.safe_zone;
        if distance >= 0.0 || self.damage_per_block <= 0.0 {
            return None;
        }

        Some((-distance * self.damage_per_block).floor().max(1.0) as f32)
    }

    #[must_use]
    pub(crate) fn collision_shapes_for(self, aabb: WorldAabb) -> Vec<WorldAabb> {
        let min_x = self.min_x.floor();
        let min_z = self.min_z.floor();
        let max_x = self.max_x.ceil();
        let max_z = self.max_z.ceil();
        let shapes = [
            WorldAabb::new(
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                min_x,
                f64::INFINITY,
                f64::INFINITY,
            ),
            WorldAabb::new(
                max_x,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            ),
            WorldAabb::new(
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::INFINITY,
                min_z,
            ),
            WorldAabb::new(
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
                max_z,
                f64::INFINITY,
                f64::INFINITY,
                f64::INFINITY,
            ),
        ];

        shapes
            .into_iter()
            .filter(|shape| shape.intersects(aabb))
            .collect()
    }
}

/// Invalid world border state or update.
#[derive(Debug, Clone, PartialEq)]
pub enum WorldBorderError {
    /// A floating-point field was NaN or infinite.
    NonFinite(&'static str, f64),
    /// A center coordinate exceeded vanilla's allowed range.
    CenterOutOfRange(&'static str, f64),
    /// A size lerp was requested with a negative duration.
    NegativeLerpTime(i64),
}

impl fmt::Display for WorldBorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NonFinite(field, value) => {
                write!(f, "world border {field} must be finite, got {value}")
            }
            Self::CenterOutOfRange(field, value) => write!(
                f,
                "world border {field} must be within +/-{MAX_CENTER_COORDINATE}, got {value}"
            ),
            Self::NegativeLerpTime(value) => {
                write!(
                    f,
                    "world border lerp time must be non-negative, got {value}"
                )
            }
        }
    }
}

impl Error for WorldBorderError {}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BorderExtent {
    Static {
        size: f64,
    },
    Moving {
        from: f64,
        to: f64,
        lerp_duration: i64,
        lerp_progress: i64,
        size: f64,
        previous_size: f64,
    },
}

impl BorderExtent {
    const fn static_size(size: f64) -> Self {
        Self::Static { size }
    }

    #[expect(
        clippy::float_cmp,
        reason = "Vanilla collapses lerps when start and target sizes are exactly equal."
    )]
    fn moving(from: f64, to: f64, ticks: i64) -> Result<Self, WorldBorderError> {
        if ticks < 0 {
            return Err(WorldBorderError::NegativeLerpTime(ticks));
        }
        if ticks == 0 || from == to {
            return Ok(Self::static_size(to));
        }

        Ok(Self::Moving {
            from,
            to,
            lerp_duration: ticks,
            lerp_progress: ticks,
            size: from,
            previous_size: from,
        })
    }

    const fn size(self) -> f64 {
        match self {
            Self::Static { size } | Self::Moving { size, .. } => size,
        }
    }

    const fn lerp_time(self) -> i64 {
        match self {
            Self::Static { .. } => 0,
            Self::Moving { lerp_progress, .. } => lerp_progress,
        }
    }

    const fn lerp_target(self) -> f64 {
        match self {
            Self::Static { size } => size,
            Self::Moving { to, .. } => to,
        }
    }

    fn status(self) -> BorderStatus {
        match self {
            Self::Static { .. } => BorderStatus::Stationary,
            Self::Moving { from, to, .. } if to < from => BorderStatus::Shrinking,
            Self::Moving { .. } => BorderStatus::Growing,
        }
    }

    fn tick(self) -> Self {
        let Self::Moving {
            from,
            to,
            lerp_duration,
            lerp_progress,
            size,
            ..
        } = self
        else {
            return self;
        };

        let next_progress = lerp_progress - 1;
        if next_progress <= 0 {
            return Self::Static { size: to };
        }

        let progress = (lerp_duration - next_progress) as f64 / lerp_duration as f64;
        Self::Moving {
            from,
            to,
            lerp_duration,
            lerp_progress: next_progress,
            size: from + (to - from) * progress,
            previous_size: size,
        }
    }
}

/// One world-border mutation reported to [`BorderChangeListener`]s.
///
/// Vanilla's `BorderChangeListener` receives the whole `WorldBorder`; Steel
/// passes the post-mutation [`WorldBorderSnapshot`] instead because the border is
/// held behind a lock and listeners must never re-enter it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BorderChange {
    /// Vanilla `BorderChangeListener.onSetSize`.
    Size {
        /// New border size.
        new_size: f64,
    },
    /// Vanilla `BorderChangeListener.onLerpSize`.
    ///
    /// Vanilla also passes the current `gameTime`; Steel's lerp state stores the
    /// remaining progress instead of an absolute end tick, so no game time is
    /// needed to describe the change.
    LerpSize {
        /// Size the lerp starts from.
        from_size: f64,
        /// Size the lerp targets.
        target_size: f64,
        /// Lerp duration in ticks.
        ticks: i64,
    },
    /// Vanilla `BorderChangeListener.onSetCenter`.
    Center {
        /// New center X coordinate.
        x: f64,
        /// New center Z coordinate.
        z: f64,
    },
    /// Vanilla `BorderChangeListener.onSetWarningTime`.
    WarningTime {
        /// New warning time in ticks.
        time: i32,
    },
    /// Vanilla `BorderChangeListener.onSetWarningBlocks`.
    WarningBlocks {
        /// New warning distance in blocks.
        blocks: i32,
    },
    /// Vanilla `BorderChangeListener.onSetDamagePerBlock`.
    DamagePerBlock {
        /// New damage per block outside the safe zone.
        damage_per_block: f64,
    },
    /// Vanilla `BorderChangeListener.onSetSafeZone`.
    SafeZone {
        /// New safe zone width outside the border.
        safe_zone: f64,
    },
}

/// Vanilla `BorderChangeListener`: an observer of world-border mutations.
///
/// Steel's built-in packet broadcast stays inline in the `World` setters (the
/// equivalent of vanilla's single `PlayerList` listener); registered listeners
/// are additional observers such as spawn-suppression or gametest hooks.
pub trait BorderChangeListener: Send + Sync {
    /// Called after the border size changed statically.
    fn on_set_size(&self, border: WorldBorderSnapshot, new_size: f64);
    /// Called after a border size lerp started.
    fn on_lerp_size(
        &self,
        border: WorldBorderSnapshot,
        from_size: f64,
        target_size: f64,
        ticks: i64,
    );
    /// Called after the border center changed.
    fn on_set_center(&self, border: WorldBorderSnapshot, x: f64, z: f64);
    /// Called after the warning time changed.
    fn on_set_warning_time(&self, border: WorldBorderSnapshot, time: i32);
    /// Called after the warning distance changed.
    fn on_set_warning_blocks(&self, border: WorldBorderSnapshot, blocks: i32);
    /// Called after the damage per block changed.
    fn on_set_damage_per_block(&self, border: WorldBorderSnapshot, damage_per_block: f64);
    /// Called after the safe zone changed.
    fn on_set_safe_zone(&self, border: WorldBorderSnapshot, safe_zone: f64);
}

#[derive(Clone)]
pub(crate) struct WorldBorder {
    center_x: f64,
    center_z: f64,
    damage_per_block: f64,
    safe_zone: f64,
    warning_time: i32,
    warning_blocks: i32,
    absolute_max_size: i32,
    extent: BorderExtent,
    listeners: Vec<Arc<dyn BorderChangeListener>>,
}

impl fmt::Debug for WorldBorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorldBorder")
            .field("center_x", &self.center_x)
            .field("center_z", &self.center_z)
            .field("damage_per_block", &self.damage_per_block)
            .field("safe_zone", &self.safe_zone)
            .field("warning_time", &self.warning_time)
            .field("warning_blocks", &self.warning_blocks)
            .field("absolute_max_size", &self.absolute_max_size)
            .field("extent", &self.extent)
            .field("listeners", &self.listeners.len())
            .finish()
    }
}

impl PartialEq for WorldBorder {
    /// Border state equality; registered change listeners are observers and are not compared.
    fn eq(&self, other: &Self) -> bool {
        self.center_x == other.center_x
            && self.center_z == other.center_z
            && self.damage_per_block == other.damage_per_block
            && self.safe_zone == other.safe_zone
            && self.warning_time == other.warning_time
            && self.warning_blocks == other.warning_blocks
            && self.absolute_max_size == other.absolute_max_size
            && self.extent == other.extent
    }
}

impl WorldBorder {
    pub(crate) fn new(data: WorldBorderData) -> Result<Self, WorldBorderError> {
        validate_center("center_x", data.center_x)?;
        validate_center("center_z", data.center_z)?;
        validate_finite("damage_per_block", data.damage_per_block)?;
        validate_finite("safe_zone", data.safe_zone)?;
        validate_finite("size", data.size)?;
        validate_finite("lerp_target", data.lerp_target)?;

        let extent = if data.lerp_time > 0 {
            BorderExtent::moving(data.size, data.lerp_target, data.lerp_time)?
        } else {
            BorderExtent::static_size(data.size)
        };

        Ok(Self {
            center_x: data.center_x,
            center_z: data.center_z,
            damage_per_block: data.damage_per_block,
            safe_zone: data.safe_zone,
            warning_time: data.warning_time,
            warning_blocks: data.warning_blocks,
            absolute_max_size: DEFAULT_ABSOLUTE_MAX_SIZE,
            extent,
            listeners: Vec::new(),
        })
    }

    /// Vanilla `WorldBorder.addListener`.
    pub(crate) fn add_listener(&mut self, listener: Arc<dyn BorderChangeListener>) {
        self.listeners.push(listener);
    }

    /// Vanilla `WorldBorder.removeListener`.
    pub(crate) fn remove_listener(&mut self, listener: &Arc<dyn BorderChangeListener>) {
        self.listeners
            .retain(|registered| !Arc::ptr_eq(registered, listener));
    }

    /// Returns the registered listeners, matching vanilla's copy-on-read
    /// `getListeners` so listeners can be added or removed while notifying.
    pub(crate) fn listeners(&self) -> Vec<Arc<dyn BorderChangeListener>> {
        self.listeners.clone()
    }

    #[must_use]
    pub(crate) fn snapshot(&self) -> WorldBorderSnapshot {
        let old_size = self.extent.size();
        let new_size = self.extent.lerp_target();
        let half_size = old_size / 2.0;
        let absolute_max = f64::from(self.absolute_max_size);
        WorldBorderSnapshot {
            center_x: self.center_x,
            center_z: self.center_z,
            old_size,
            new_size,
            lerp_time: self.extent.lerp_time(),
            absolute_max_size: self.absolute_max_size,
            warning_blocks: self.warning_blocks,
            warning_time: self.warning_time,
            damage_per_block: self.damage_per_block,
            safe_zone: self.safe_zone,
            min_x: clamp_f64(self.center_x - half_size, -absolute_max, absolute_max),
            min_z: clamp_f64(self.center_z - half_size, -absolute_max, absolute_max),
            max_x: clamp_f64(self.center_x + half_size, -absolute_max, absolute_max),
            max_z: clamp_f64(self.center_z + half_size, -absolute_max, absolute_max),
            status: self.extent.status(),
        }
    }

    #[must_use]
    pub(crate) const fn to_data(&self) -> WorldBorderData {
        WorldBorderData {
            center_x: self.center_x,
            center_z: self.center_z,
            damage_per_block: self.damage_per_block,
            safe_zone: self.safe_zone,
            warning_blocks: self.warning_blocks,
            warning_time: self.warning_time,
            size: self.extent.size(),
            lerp_time: self.extent.lerp_time(),
            lerp_target: self.extent.lerp_target(),
        }
    }

    pub(crate) fn tick(&mut self) {
        self.extent = self.extent.tick();
    }

    pub(crate) fn set_center(&mut self, x: f64, z: f64) -> Result<BorderChange, WorldBorderError> {
        validate_center("center_x", x)?;
        validate_center("center_z", z)?;
        self.center_x = x;
        self.center_z = z;
        Ok(BorderChange::Center { x, z })
    }

    pub(crate) fn set_size(&mut self, size: f64) -> Result<BorderChange, WorldBorderError> {
        validate_finite("size", size)?;
        self.extent = BorderExtent::static_size(size);
        Ok(BorderChange::Size { new_size: size })
    }

    pub(crate) fn lerp_size_between(
        &mut self,
        from: f64,
        to: f64,
        ticks: i64,
    ) -> Result<BorderChange, WorldBorderError> {
        validate_finite("from", from)?;
        validate_finite("to", to)?;
        self.extent = BorderExtent::moving(from, to, ticks)?;
        Ok(BorderChange::LerpSize {
            from_size: from,
            target_size: to,
            ticks,
        })
    }

    pub(crate) const fn set_warning_time(&mut self, warning_time: i32) -> BorderChange {
        self.warning_time = warning_time;
        BorderChange::WarningTime { time: warning_time }
    }

    pub(crate) const fn set_warning_blocks(&mut self, warning_blocks: i32) -> BorderChange {
        self.warning_blocks = warning_blocks;
        BorderChange::WarningBlocks {
            blocks: warning_blocks,
        }
    }

    pub(crate) fn set_damage_per_block(
        &mut self,
        damage_per_block: f64,
    ) -> Result<BorderChange, WorldBorderError> {
        validate_finite("damage_per_block", damage_per_block)?;
        self.damage_per_block = damage_per_block;
        Ok(BorderChange::DamagePerBlock { damage_per_block })
    }

    pub(crate) fn set_safe_zone(
        &mut self,
        safe_zone: f64,
    ) -> Result<BorderChange, WorldBorderError> {
        validate_finite("safe_zone", safe_zone)?;
        self.safe_zone = safe_zone;
        Ok(BorderChange::SafeZone { safe_zone })
    }
}

fn validate_center(field: &'static str, value: f64) -> Result<(), WorldBorderError> {
    validate_finite(field, value)?;
    if !(-MAX_CENTER_COORDINATE..=MAX_CENTER_COORDINATE).contains(&value) {
        return Err(WorldBorderError::CenterOutOfRange(field, value));
    }
    Ok(())
}

const fn validate_finite(field: &'static str, value: f64) -> Result<(), WorldBorderError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(WorldBorderError::NonFinite(field, value))
    }
}

fn clamp_f64(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

use super::{
    CInitializeBorder, CSetBorderCenter, CSetBorderLerpSize, CSetBorderSize,
    CSetBorderWarningDelay, CSetBorderWarningDistance, HeightmapType, RespawnData, World,
    initialize_border_packet,
};

impl World {
    /// Vanilla `WorldBorder.addListener`: registers an observer of border mutations.
    pub fn add_world_border_listener(&self, listener: Arc<dyn BorderChangeListener>) {
        self.world_border.lock().add_listener(listener);
    }

    /// Vanilla `WorldBorder.removeListener`.
    pub fn remove_world_border_listener(&self, listener: &Arc<dyn BorderChangeListener>) {
        self.world_border.lock().remove_listener(listener);
    }

    /// Notifies every registered listener of one border mutation.
    fn notify_border_listeners(&self, snapshot: WorldBorderSnapshot, change: BorderChange) {
        let listeners = self.world_border.lock().listeners();
        for listener in listeners {
            match change {
                BorderChange::Size { new_size } => listener.on_set_size(snapshot, new_size),
                BorderChange::LerpSize {
                    from_size,
                    target_size,
                    ticks,
                } => listener.on_lerp_size(snapshot, from_size, target_size, ticks),
                BorderChange::Center { x, z } => listener.on_set_center(snapshot, x, z),
                BorderChange::WarningTime { time } => listener.on_set_warning_time(snapshot, time),
                BorderChange::WarningBlocks { blocks } => {
                    listener.on_set_warning_blocks(snapshot, blocks);
                }
                BorderChange::DamagePerBlock { damage_per_block } => {
                    listener.on_set_damage_per_block(snapshot, damage_per_block);
                }
                BorderChange::SafeZone { safe_zone } => {
                    listener.on_set_safe_zone(snapshot, safe_zone);
                }
            }
        }
    }

    #[must_use]
    pub(crate) fn world_border_snapshot(&self) -> WorldBorderSnapshot {
        self.world_border.lock().snapshot()
    }

    /// Returns whether a block position is inside this world's vanilla world border.
    #[must_use]
    pub fn is_block_within_world_border(&self, pos: BlockPos) -> bool {
        self.world_border_snapshot().is_block_within_bounds(pos)
    }

    /// Clamps a world-space position to this world's vanilla world border and floors it to a block position.
    #[must_use]
    pub fn clamp_to_world_border(&self, x: f64, y: f64, z: f64) -> BlockPos {
        self.world_border_snapshot().clamp_to_bounds(x, y, z)
    }

    #[must_use]
    pub(crate) fn initialize_border_packet(&self) -> CInitializeBorder {
        initialize_border_packet(self.world_border_snapshot())
    }

    #[must_use]
    pub(crate) fn world_border_adjusted_respawn_data(
        &self,
        respawn_data: RespawnData,
    ) -> RespawnData {
        let pos = respawn_data.pos();
        if self.is_block_within_world_border(pos) {
            return respawn_data;
        }

        let border = self.world_border_snapshot();
        let center_pos = BlockPos::containing(border.center_x, 0.0, border.center_z);
        let new_pos = self.heightmap_pos(HeightmapType::MotionBlocking, center_pos);
        RespawnData::of(
            respawn_data.dimension().clone(),
            new_pos,
            respawn_data.yaw,
            respawn_data.pitch,
        )
    }

    /// Sets the world border center and broadcasts the vanilla center update packet.
    pub fn set_world_border_center(&self, x: f64, z: f64) -> Result<(), WorldBorderError> {
        let (snapshot, data, change) = {
            let mut border = self.world_border.lock();
            let change = border.set_center(x, z)?;
            (border.snapshot(), border.to_data(), change)
        };
        self.store_world_border_data_if_changed(data);
        self.broadcast_to_all(CSetBorderCenter {
            new_center_x: snapshot.center_x,
            new_center_z: snapshot.center_z,
        });
        self.notify_border_listeners(snapshot, change);
        Ok(())
    }

    /// Sets a static world border size and broadcasts the vanilla size update packet.
    pub fn set_world_border_size(&self, size: f64) -> Result<(), WorldBorderError> {
        let (snapshot, data, change) = {
            let mut border = self.world_border.lock();
            let change = border.set_size(size)?;
            (border.snapshot(), border.to_data(), change)
        };
        self.store_world_border_data_if_changed(data);
        self.broadcast_to_all(CSetBorderSize {
            size: snapshot.new_size,
        });
        self.notify_border_listeners(snapshot, change);
        Ok(())
    }

    /// Starts a vanilla world border size lerp and broadcasts the lerp update packet.
    pub fn lerp_world_border_size_between(
        &self,
        from: f64,
        to: f64,
        ticks: i64,
    ) -> Result<(), WorldBorderError> {
        let (snapshot, data, change) = {
            let mut border = self.world_border.lock();
            let change = border.lerp_size_between(from, to, ticks)?;
            (border.snapshot(), border.to_data(), change)
        };
        self.store_world_border_data_if_changed(data);
        self.broadcast_to_all(CSetBorderLerpSize {
            old_size: snapshot.old_size,
            new_size: snapshot.new_size,
            lerp_time: snapshot.lerp_time,
        });
        self.notify_border_listeners(snapshot, change);
        Ok(())
    }

    /// Sets the client warning time and broadcasts the vanilla warning-delay packet.
    pub fn set_world_border_warning_time(&self, warning_time: i32) {
        let (data, change) = {
            let mut border = self.world_border.lock();
            let change = border.set_warning_time(warning_time);
            (border.to_data(), change)
        };
        self.store_world_border_data_if_changed(data);
        self.broadcast_to_all(CSetBorderWarningDelay {
            warning_delay: warning_time,
        });
        self.notify_border_listeners(self.world_border_snapshot(), change);
    }

    /// Sets the client warning distance and broadcasts the vanilla warning-distance packet.
    pub fn set_world_border_warning_blocks(&self, warning_blocks: i32) {
        let (data, change) = {
            let mut border = self.world_border.lock();
            let change = border.set_warning_blocks(warning_blocks);
            (border.to_data(), change)
        };
        self.store_world_border_data_if_changed(data);
        self.broadcast_to_all(CSetBorderWarningDistance { warning_blocks });
        self.notify_border_listeners(self.world_border_snapshot(), change);
    }

    /// Sets world border damage per block outside the safe zone.
    pub fn set_world_border_damage_per_block(
        &self,
        damage_per_block: f64,
    ) -> Result<(), WorldBorderError> {
        let (data, change) = {
            let mut border = self.world_border.lock();
            let change = border.set_damage_per_block(damage_per_block)?;
            (border.to_data(), change)
        };
        self.store_world_border_data_if_changed(data);
        self.notify_border_listeners(self.world_border_snapshot(), change);
        Ok(())
    }

    /// Sets the safe distance outside the world border before damage starts.
    pub fn set_world_border_safe_zone(&self, safe_zone: f64) -> Result<(), WorldBorderError> {
        let (data, change) = {
            let mut border = self.world_border.lock();
            let change = border.set_safe_zone(safe_zone)?;
            (border.to_data(), change)
        };
        self.store_world_border_data_if_changed(data);
        self.notify_border_listeners(self.world_border_snapshot(), change);
        Ok(())
    }

    pub(super) fn tick_world_border(&self) {
        let data = {
            let mut border = self.world_border.lock();
            border.tick();
            border.to_data()
        };
        self.store_world_border_data_if_changed(data);
    }

    pub(super) fn sync_world_border_to_level_data(&self) {
        let data = self.world_border.lock().to_data();
        self.store_world_border_data_if_changed(data);
    }

    pub(super) fn store_world_border_data_if_changed(&self, data: WorldBorderData) {
        let mut level_data = self.level_data.write();
        if level_data.data().world_border != data {
            level_data.data_mut().world_border = data;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_border() -> WorldBorder {
        WorldBorder::new(WorldBorderData::default()).expect("default border data is valid")
    }

    use std::sync::Arc;

    /// A `BorderChangeListener` used to exercise the registry; callbacks are no-ops.
    #[derive(Debug)]
    struct NoopBorderListener;

    impl BorderChangeListener for NoopBorderListener {
        fn on_set_size(&self, _border: WorldBorderSnapshot, _new_size: f64) {}

        fn on_lerp_size(
            &self,
            _border: WorldBorderSnapshot,
            _from_size: f64,
            _target_size: f64,
            _ticks: i64,
        ) {
        }

        fn on_set_center(&self, _border: WorldBorderSnapshot, _x: f64, _z: f64) {}

        fn on_set_warning_time(&self, _border: WorldBorderSnapshot, _time: i32) {}

        fn on_set_warning_blocks(&self, _border: WorldBorderSnapshot, _blocks: i32) {}

        fn on_set_damage_per_block(&self, _border: WorldBorderSnapshot, _damage_per_block: f64) {}

        fn on_set_safe_zone(&self, _border: WorldBorderSnapshot, _safe_zone: f64) {}
    }

    #[test]
    fn border_mutators_report_vanilla_change_events() {
        let mut border = default_border();

        let center = border.set_center(4.0, -8.0).expect("valid center");
        let size = border.set_size(64.0).expect("valid size");
        let lerp = border
            .lerp_size_between(64.0, 32.0, 20)
            .expect("valid lerp");
        let warning_time = border.set_warning_time(42);
        let warning_blocks = border.set_warning_blocks(9);
        let damage = border
            .set_damage_per_block(0.5)
            .expect("valid damage per block");
        let safe_zone = border.set_safe_zone(3.0).expect("valid safe zone");

        assert_eq!(center, BorderChange::Center { x: 4.0, z: -8.0 });
        assert_eq!(size, BorderChange::Size { new_size: 64.0 });
        assert_eq!(
            lerp,
            BorderChange::LerpSize {
                from_size: 64.0,
                target_size: 32.0,
                ticks: 20,
            }
        );
        assert_eq!(warning_time, BorderChange::WarningTime { time: 42 });
        assert_eq!(warning_blocks, BorderChange::WarningBlocks { blocks: 9 });
        assert_eq!(
            damage,
            BorderChange::DamagePerBlock {
                damage_per_block: 0.5,
            }
        );
        assert_eq!(safe_zone, BorderChange::SafeZone { safe_zone: 3.0 });
    }

    #[test]
    fn listener_registry_add_remove_matches_vanilla_list_semantics() {
        let listener: Arc<dyn BorderChangeListener> = Arc::new(NoopBorderListener);

        let mut border = default_border();
        assert!(border.listeners().is_empty());

        border.add_listener(Arc::clone(&listener));
        assert_eq!(border.listeners().len(), 1);

        // Vanilla `getListeners` copies the list, so a copied list is unaffected
        // by later removal.
        let copied = border.listeners();
        border.remove_listener(&listener);
        assert!(border.listeners().is_empty());
        assert_eq!(copied.len(), 1);
    }
}
