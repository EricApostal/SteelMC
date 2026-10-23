//! Per-world map item storage.
//!
//! A world owns its maps; this manager allocates map ids (`MapIndex`) and loads,
//! saves, ticks and broadcasts each map (`MapItemSavedData`). It is the Steel
//! analogue of vanilla `Level.getFreeMapId` / `Level.getMapData` /
//! `Level.setMapData`, all keyed by the map id stored in the
//! `minecraft:map_id` component of each filled-map stack.

use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};
use steel_registry::vanilla_map_decoration_types;
use steel_utils::Identifier;
use steel_utils::locks::{SyncMutex, SyncRwLock};
use steel_utils::saved_data::{SavedDataManager, names as saved_data_names};
use uuid::Uuid;

use super::data::{MapItemSavedData, PersistentMapData};
use super::decoration::MapFrame;
use super::index::MapIndex;
use crate::entity::Entity as _;
use crate::player::Player;
use crate::world::World;

/// All maps belonging to one world, persisted through the world saved data.
pub struct MapStorage {
    inner: SyncRwLock<MapStorageInner>,
    save_lock: SyncMutex<()>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PersistentMaps {
    index: MapIndex,
    maps: BTreeMap<i32, PersistentMapData>,
}

struct MapStorageInner {
    index: MapIndex,
    maps: BTreeMap<i32, MapItemSavedData>,
}

impl Default for MapStorageInner {
    fn default() -> Self {
        Self {
            index: MapIndex::new(),
            maps: BTreeMap::new(),
        }
    }
}

impl MapStorage {
    /// Creates an empty map store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: SyncRwLock::new(MapStorageInner::default()),
            save_lock: SyncMutex::new(()),
        }
    }

    /// Loads a world's maps from its saved data.
    ///
    /// Takes the saved-data manager rather than the world so a world can load
    /// its maps while it is still being constructed.
    ///
    /// # Errors
    ///
    /// Returns an error when the saved maps cannot be read.
    pub async fn load(saved_data: &SavedDataManager) -> io::Result<Self> {
        let persistent: PersistentMaps = saved_data.load_or_default(saved_data_names::MAPS).await?;
        let mut inner = MapStorageInner {
            index: persistent.index,
            maps: BTreeMap::new(),
        };
        for (id, map) in persistent.maps {
            inner.index.observe(id);
            inner
                .maps
                .insert(id, MapItemSavedData::from_persistent(map));
        }
        Ok(Self {
            inner: SyncRwLock::new(inner),
            save_lock: SyncMutex::new(()),
        })
    }

    /// Saves every map when the store is dirty and returns the number written.
    ///
    /// # Errors
    ///
    /// Returns an error when the saved maps cannot be written.
    pub async fn save(&self, saved_data: &SavedDataManager) -> io::Result<usize> {
        let _save_guard = self.save_lock.lock();
        let inner = self.inner.read();
        let persistent = PersistentMaps {
            index: inner.index,
            maps: inner
                .maps
                .iter()
                .map(|(id, map)| (*id, map.to_persistent()))
                .collect(),
        };
        saved_data.save(saved_data_names::MAPS, &persistent).await?;
        Ok(persistent.maps.len())
    }

    /// Vanilla `Level.getFreeMapId`.
    pub fn next_map_id(&self) -> i32 {
        self.inner.write().index.next_map_id()
    }

    /// Vanilla `Level.getMapData`.
    #[must_use]
    pub fn get_map(&self, map_id: i32) -> Option<MapItemSnapshot> {
        let inner = self.inner.read();
        inner.maps.get(&map_id).map(|map| MapItemSnapshot {
            map_id,
            center_x: map.center_x,
            center_z: map.center_z,
            scale: map.scale,
            locked: map.locked,
        })
    }

    /// Vanilla `Level.setMapData`.
    pub fn set_map(&self, map_id: i32, map: MapItemSavedData) {
        let mut inner = self.inner.write();
        inner.index.observe(map_id);
        inner.maps.insert(map_id, map);
    }

    /// Returns whether a map id exists in this world.
    #[must_use]
    pub fn contains_map(&self, map_id: i32) -> bool {
        self.inner.read().maps.contains_key(&map_id)
    }

    /// Returns every map id tracked by this world, in stable order.
    #[must_use]
    pub fn map_ids(&self) -> Vec<i32> {
        self.inner.read().maps.keys().copied().collect()
    }

    /// Runs one map mutation with exclusive access.
    pub fn with_map_mut<R>(
        &self,
        map_id: i32,
        f: impl FnOnce(&mut MapItemSavedData) -> R,
    ) -> Option<R> {
        let mut inner = self.inner.write();
        inner.maps.get_mut(&map_id).map(f)
    }

    /// Vanilla `MapItemSavedData.tickCarriedBy` for a carried map: registers the
    /// viewer, refreshes nearby banner markers, records the viewer's arrow and
    /// sends the queued map update.
    ///
    /// Locked maps still track their viewers; only the rasterizer
    /// ([`Self::update_colors`]) is skipped for them.
    ///
    /// Returns `false` when the map id is unknown.
    pub fn tick_carried_by(&self, world: &World, map_id: i32, player: &Player) -> bool {
        let player_uuid = player.gameprofile.id();
        let packet = {
            let mut inner = self.inner.write();
            let Some(map) = inner.maps.get_mut(&map_id) else {
                return false;
            };
            map.check_banners_near_player(world, player);
            map.track_viewer(player_uuid);
            let position = player.position();
            let has_ceiling = world.dimension_type.has_ceiling;
            let game_time = world.game_time();
            map.add_decoration(
                &vanilla_map_decoration_types::PLAYER,
                &player.gameprofile.name().to_owned(),
                position.x,
                position.z,
                player.rotation().0,
                None,
                has_ceiling,
                game_time,
            );
            map.update_packet(map_id, player_uuid)
        };
        if let Some(packet) = packet {
            player.connection.send_packet(packet);
        }
        true
    }

    /// Returns whether the stored map is locked, or `None` for an unknown id.
    #[must_use]
    pub fn is_locked(&self, map_id: i32) -> Option<bool> {
        self.inner.read().maps.get(&map_id).map(|map| map.locked)
    }

    /// Vanilla `MapItem.update`: rasterizes the terrain around one viewer into
    /// a held map.
    pub fn update_colors(&self, world: &World, map_id: i32, player: &Player) {
        let mut inner = self.inner.write();
        let Some(map) = inner.maps.get_mut(&map_id) else {
            return;
        };
        if map.locked {
            return;
        }
        let step = map.next_viewer_step(player.gameprofile.id());
        super::colors::update_colors(map, world, player.position(), step);
    }

    /// Vanilla `MapItemSavedData.toggleBanner` for one stored map.
    ///
    /// Returns `None` when the map id is unknown.
    pub fn toggle_banner(
        &self,
        level: &dyn crate::world::level_reader::LevelReader,
        map_id: i32,
        pos: steel_utils::BlockPos,
    ) -> Option<bool> {
        self.with_map_mut(map_id, |map| map.toggle_banner(level, pos))
    }

    /// Records an item-frame marker for a map, mirroring vanilla's
    /// `MapItemSavedData.tickCarriedBy` frame half.
    pub fn track_frame_for_map(&self, world: &World, map_id: i32, frame: MapFrame) {
        let mut inner = self.inner.write();
        let Some(map) = inner.maps.get_mut(&map_id) else {
            return;
        };
        map.track_frame(
            frame,
            f64::from(frame.pos().x()),
            f64::from(frame.pos().z()),
            world.dimension_type.has_ceiling,
            world.game_time(),
        );
    }
}

impl Default for MapStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Identity fields of a stored map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapItemSnapshot {
    /// Numeric map id (`MapId`).
    pub map_id: i32,
    /// Map center X in blocks.
    pub center_x: i32,
    /// Map center Z in blocks.
    pub center_z: i32,
    /// Zoom scale.
    pub scale: u8,
    /// Whether the map is locked.
    pub locked: bool,
}

#[cfg(test)]
mod tests {
    use super::{MapItemSnapshot, MapStorage};
    use crate::map::data::MapItemSavedData;
    use steel_utils::Identifier;

    fn test_world_id() -> Identifier {
        Identifier::vanilla_static("overworld")
    }

    #[test]
    fn map_ids_are_unique_and_observed() {
        let storage = MapStorage::new();
        let first = storage.next_map_id();
        let second = storage.next_map_id();
        assert_ne!(first, second);

        storage.set_map(
            9,
            MapItemSavedData::create_for_client(1, true, test_world_id()),
        );
        assert!(storage.contains_map(9));
        assert_eq!(storage.next_map_id(), 10);
    }

    #[test]
    fn snapshots_expose_map_identity() {
        let storage = MapStorage::new();
        let id = storage.next_map_id();
        let mut map = MapItemSavedData::create_fresh(0.0, 0.0, 2, true, false, test_world_id());
        map.track_viewer(uuid::Uuid::from_u128(1));
        storage.set_map(id, map);

        let snapshot = storage.get_map(id).expect("map should exist");
        assert_eq!(
            snapshot,
            MapItemSnapshot {
                map_id: id,
                // Vanilla `MapItemSavedData.createFresh` snaps the center to the
                // 128 * 2^scale block grid centered on the requested position.
                center_x: 192,
                center_z: 192,
                scale: 2,
                locked: false,
            }
        );
        assert_eq!(storage.map_ids(), vec![id]);
    }
}
