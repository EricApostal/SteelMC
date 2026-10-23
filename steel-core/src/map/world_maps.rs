//! Inherent `World` bridge to this world's [`MapStorage`].
//!
//! Vanilla `ServerLevel` owns its map saved data and exposes it through
//! `Level.getFreeMapId` / `Level.getMapData` / `Level.setMapData`. Steel keeps
//! that storage on `World` and mirrors those three accessors here, next to the
//! map model they expose.

use crate::map::{MapItemSavedData, MapItemSnapshot, MapStorage};
use crate::world::World;

impl World {
    /// Vanilla `Level.getFreeMapId`: allocates the next free map id.
    pub fn get_free_map_id(&self) -> i32 {
        self.maps.next_map_id()
    }

    /// Vanilla `Level.getMapData`: the stored identity of `map_id`.
    #[must_use]
    pub fn get_map_data(&self, map_id: i32) -> Option<MapItemSnapshot> {
        self.maps.get_map(map_id)
    }

    /// Vanilla `Level.setMapData`: stores a map under `map_id`.
    pub fn set_map_data(&self, map_id: i32, map: MapItemSavedData) {
        self.maps.set_map(map_id, map);
    }

    /// The level's map storage (vanilla `ServerLevel`'s map saved data).
    #[must_use]
    pub fn maps(&self) -> &MapStorage {
        &self.maps
    }
}
