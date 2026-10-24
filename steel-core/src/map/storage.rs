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
use steel_registry::REGISTRY;
use steel_registry::TaggedRegistryExt as _;
use steel_registry::data_components::components::MapDecorations;
use steel_registry::data_components::vanilla_components::{MAP_DECORATIONS, MAP_ID};
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::vanilla_map_decoration_types;
use steel_utils::locks::{SyncMutex, SyncRwLock};
use steel_utils::saved_data::{SavedDataManager, names as saved_data_names};

use super::data::{MapItemSavedData, PersistentMapData};
use super::decoration::MapFrame;
use super::index::MapIndex;
use crate::entity::Entity as _;
use crate::entity::LivingEntity as _;
use crate::inventory::container::Container as _;
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;
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
    /// viewer, drops viewers that no longer carry the map, refreshes nearby
    /// banner markers, records every viewer's marker and sends the queued map
    /// update.
    ///
    /// Locked maps still track their viewers; only the rasterizer
    /// ([`Self::update_colors`]) is skipped for them.
    ///
    /// Returns `false` when the map id is unknown.
    pub fn tick_carried_by(&self, world: &World, map_id: i32, player: &Player) -> bool {
        let player_uuid = player.gameprofile.id();

        // Resolve the other viewers before taking the map lock: the lookups
        // need the player index and each viewer's inventory.
        let viewers = {
            let inner = self.inner.read();
            let Some(map) = inner.maps.get(&map_id) else {
                return false;
            };
            map.viewer_ids()
                .map(|uuid| (uuid, map.viewer_name(uuid).map(str::to_owned)))
                .collect::<Vec<_>>()
        };
        let mut stale = Vec::new();
        let mut invisible = Vec::new();
        for (uuid, recorded_name) in viewers {
            if uuid == player_uuid {
                continue;
            }
            match world.players.get_by_uuid(&uuid) {
                Some(viewer) if !viewer.is_removed() && carries_map(&viewer, map_id) => {
                    if has_map_invisibility_equipment(&viewer) {
                        invisible.push(viewer.gameprofile.name().to_owned());
                    }
                }
                Some(viewer) => {
                    stale.push((uuid, viewer.gameprofile.name().to_owned()));
                }
                // A viewer that left the world keeps the name recorded when it
                // last carried this map, which is also their decoration key.
                None => stale.push((uuid, recorded_name.unwrap_or_default())),
            }
        }
        let decorations = carried_map_decorations(player, map_id);

        let packet = {
            let mut inner = self.inner.write();
            let Some(map) = inner.maps.get_mut(&map_id) else {
                return false;
            };
            for (uuid, name) in stale {
                if !name.is_empty() {
                    map.remove_decoration(&name);
                }
                map.remove_viewer(uuid);
            }
            // Vanilla removes the marker of a viewer that vanished, and of a
            // viewer wearing map-invisibility equipment (`carved_pumpkin`).
            for name in invisible {
                map.remove_decoration(&name);
            }
            map.check_banners_near_player(world, player);
            map.track_viewer(player_uuid, player.gameprofile.name());

            // Vanilla only draws the marker while the map is position-tracked
            // and the viewer is in the map's dimension.
            if map.tracking_position && player.get_world().key == map.dimension {
                let position = player.position();
                let has_ceiling = world.dimension_type.has_ceiling;
                let game_time = world.game_time();
                map.add_decoration(
                    &vanilla_map_decoration_types::PLAYER,
                    player.gameprofile.name(),
                    position.x,
                    position.z,
                    player.rotation().0,
                    None,
                    has_ceiling,
                    game_time,
                );
            }

            // Vanilla replays the carried stack's `map_decorations` component
            // (explorer maps, trial-chamber maps) on every tick.
            if let Some(decorations) = decorations {
                let missing = decorations
                    .decorations()
                    .keys()
                    .any(|key| !map.decorations().contains_key(key));
                if missing {
                    for (id, entry) in decorations.decorations() {
                        map.add_decoration(
                            entry.decoration_type().value(),
                            id,
                            entry.x(),
                            entry.z(),
                            entry.rotation(),
                            None,
                            world.dimension_type.has_ceiling,
                            world.game_time(),
                        );
                    }
                }
            }

            map.update_packet(map_id, player_uuid)
        };
        if let Some(packet) = packet {
            player.send_packet(packet);
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

/// Vanilla `Inventory.contains(ItemStack::mapMatcher)`: whether `player`
/// carries a filled map with `map_id` in a vanilla inventory slot.
fn carries_map(player: &Player, map_id: i32) -> bool {
    let inventory = player.inventory.lock();
    (0..=PlayerInventory::SLOT_OFFHAND).any(|slot| {
        inventory
            .get_item(slot)
            .get(MAP_ID)
            .is_some_and(|id| id.id() == map_id)
    })
}

/// Vanilla `MapItemSavedData.hasMapInvisibilityItemEquipped`: a
/// `#minecraft:map_invisibility_equipment` item worn in an armor slot hides the
/// player's marker from every map.
fn has_map_invisibility_equipment(player: &Player) -> bool {
    let mut hidden = false;
    for slot in EquipmentSlot::ALL {
        if matches!(slot, EquipmentSlot::MainHand | EquipmentSlot::OffHand) {
            continue;
        }
        player.with_equipment_slot(slot, &mut |stack| {
            if !stack.is_empty()
                && REGISTRY
                    .items
                    .is_in_tag(stack.item(), &ItemTag::MAP_INVISIBILITY_EQUIPMENT)
            {
                hidden = true;
            }
        });
    }
    hidden
}

/// The `map_decorations` component of the carried stack for `map_id`.
///
/// Vanilla passes the ticking stack into `tickCarriedBy`; Steel's inventory
/// tick only knows the map id, so the first matching carried stack supplies the
/// component (stacks of the same map id share it in practice).
fn carried_map_decorations(player: &Player, map_id: i32) -> Option<MapDecorations> {
    let inventory = player.inventory.lock();
    (0..=PlayerInventory::SLOT_OFFHAND).find_map(|slot| {
        let stack = inventory.get_item(slot);
        if !stack.get(MAP_ID).is_some_and(|id| id.id() == map_id) {
            return None;
        }
        stack.get(MAP_DECORATIONS).cloned()
    })
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
        map.track_viewer(uuid::Uuid::from_u128(1), "viewer");
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

    /// Vanilla `MapItemSavedData.tickCarriedBy` replays the carried stack's
    /// `map_decorations` component onto the saved data and adds the viewer's
    /// own marker.
    #[test]
    fn tick_carried_by_applies_the_stack_decoration_component() {
        use std::collections::BTreeMap;
        use std::sync::Arc;

        use steel_registry::data_components::components::{
            MapDecorationEntry, MapDecorations, MapId,
        };
        use steel_registry::data_components::vanilla_components::{MAP_DECORATIONS, MAP_ID};
        use steel_registry::item_stack::ItemStack;
        use steel_registry::{
            REGISTRY, RegistryExt, RegistryReference, init_vanilla_registry, vanilla_items,
        };

        use crate::inventory::container::Container as _;
        use crate::test_support::{TestPlayerBuilder, fresh_test_world};

        init_vanilla_registry();
        let fixture = fresh_test_world("map_tick_decorations");
        let world = &fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "MapViewer", 1).build();

        let map_id = world.get_free_map_id();
        world.set_map_data(
            map_id,
            MapItemSavedData::create_fresh(0.0, 0.0, 0, true, false, world.key.clone()),
        );

        let mansion = REGISTRY
            .map_decoration_types
            .by_key(&Identifier::vanilla_static("mansion"))
            .expect("mansion decoration type");
        let mut entries = BTreeMap::new();
        entries.insert(
            "trial_chamber".to_owned(),
            MapDecorationEntry::new(RegistryReference::new(mansion), 10.0, 20.0, 0.0),
        );
        let mut stack = ItemStack::new(&vanilla_items::FILLED_MAP);
        stack.set(MAP_ID, MapId::new(map_id));
        stack.set(MAP_DECORATIONS, MapDecorations::new(entries));
        player.inventory.lock().set_item(0, stack);

        assert!(world.maps().tick_carried_by(world, map_id, &player));

        let decorations = world
            .maps()
            .with_map_mut(map_id, |map| map.decorations().clone())
            .expect("map should exist");
        let entry = decorations
            .get("trial_chamber")
            .expect("component decoration should be tracked");
        // (10 - 0) / 1 * 2 + 0.5 and (20 - 0) / 1 * 2 + 0.5, truncated.
        assert_eq!((entry.x, entry.y), (20, 40));
        assert!(decorations.contains_key("MapViewer"));
    }
}
