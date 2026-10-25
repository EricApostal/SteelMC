//! Village POI queries and the village-distance tracker (vanilla `PoiManager`).
//!
//! Vanilla's `PoiManager` owns the POI sections of one level; Steel's counterpart is
//! [`World::poi_storage`], so the vanilla query API is added here as inherent methods
//! on [`PointOfInterestStorage`]. Every query takes a type *predicate* (vanilla
//! `Predicate<Holder<PoiType>>`) rather than a list of types, which is what the
//! villager profession helpers (`VillagerProfession::is_held_job_site`, …) and the
//! village tag checks need.
//!
//! `PoiManager.sectionsToVillage` is backed by the vanilla `DistanceTracker`, an
//! incremental 26-neighbour graph whose fixed point is the hop distance to the closest
//! *village center* section (a section holding an occupied POI in the `village` tag),
//! capped at `MAX_VILLAGE_DISTANCE` (sections at level 7 are simply untracked). Because
//! the reported level only depends on village centers within `MAX_VILLAGE_DISTANCE`
//! sections, the distance field is recomputed on demand from those seeds instead of
//! being updated incrementally.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};
use steel_registry::REGISTRY;
use steel_registry::RegistryExt as _;
use steel_registry::TaggedRegistryExt as _;
use steel_registry::poi::PoiTypeRef;
use steel_registry::vanilla_poi_type_tags::PoiTag;
use steel_utils::random::Random;
use steel_utils::{BlockPos, SectionPos};

use crate::poi::{OccupationStatus, PointOfInterestStorage};
use crate::world::World;

/// Vanilla `PoiManager.MAX_VILLAGE_DISTANCE`: the furthest tracked section distance.
pub const MAX_VILLAGE_DISTANCE: i32 = 6;

/// Reported distance for sections outside the tracked range (vanilla's
/// `DistanceTracker` default return value of `MAX_VILLAGE_DISTANCE + 1`).
const UNTRACKED_VILLAGE_DISTANCE: i32 = MAX_VILLAGE_DISTANCE + 1;

/// A predicate over POI types (vanilla `Predicate<Holder<PoiType>>`).
pub type PoiTypePredicate<'a> = &'a dyn Fn(PoiTypeRef) -> bool;

/// Returns whether `poi_type` belongs to the vanilla `village` POI tag.
///
/// Village centers are sections holding an occupied POI of this tag.
#[must_use]
pub fn is_village_poi(poi_type: PoiTypeRef) -> bool {
    REGISTRY.poi_types.is_in_tag(poi_type, &PoiTag::VILLAGE)
}

/// Adapts a type predicate to the storage's POI type id predicate.
fn type_id_predicate(predicate: PoiTypePredicate<'_>) -> impl Fn(usize) -> bool + '_ {
    move |type_id| REGISTRY.poi_types.by_id(type_id).is_some_and(predicate)
}

/// The `village` POI tag as a type-id predicate.
fn is_village_poi_type(type_id: usize) -> bool {
    REGISTRY
        .poi_types
        .by_id(type_id)
        .is_some_and(is_village_poi)
}

/// Vanilla `Vec3i.distSqr`.
fn distance_sq(a: BlockPos, b: BlockPos) -> i64 {
    let dx = i64::from(a.x() - b.x());
    let dy = i64::from(a.y() - b.y());
    let dz = i64::from(a.z() - b.z());
    dx * dx + dy * dy + dz * dz
}

/// Vanilla `Util.shuffle`: a Fisher–Yates shuffle driven by the entity's RNG.
fn shuffle<T>(values: &mut [T], random: &mut impl Random) {
    for i in (1..values.len()).rev() {
        let j = random.next_i32_bounded(i as i32 + 1) as usize;
        values.swap(i, j);
    }
}

impl PointOfInterestStorage {
    /// Returns the POI type at `pos` (vanilla `PoiManager.getType`).
    #[must_use]
    pub fn poi_type(&self, pos: BlockPos) -> Option<PoiTypeRef> {
        self.get_type(pos)
            .and_then(|type_id| REGISTRY.poi_types.by_id(type_id))
    }

    /// Returns whether a POI matching `predicate` exists at `pos`
    /// (vanilla `PoiManager.exists`).
    #[must_use]
    pub fn exists(&self, pos: BlockPos, predicate: PoiTypePredicate<'_>) -> bool {
        self.poi_type(pos).is_some_and(predicate)
    }

    /// Returns every matching POI within a `radius` sphere around `center`
    /// (vanilla `PoiManager.getInRange`).
    #[must_use]
    pub fn get_in_range(
        &self,
        predicate: PoiTypePredicate<'_>,
        center: BlockPos,
        radius: i32,
        status: OccupationStatus,
    ) -> Vec<(PoiTypeRef, BlockPos)> {
        let radius_sq = i64::from(radius) * i64::from(radius);
        self.get_in_horizontal_square(&type_id_predicate(predicate), center, radius, status)
            .into_iter()
            .filter(|(pos, _)| distance_sq(*pos, center) <= radius_sq)
            .filter_map(|(pos, type_id)| {
                REGISTRY
                    .poi_types
                    .by_id(type_id)
                    .map(|poi_type| (poi_type, pos))
            })
            .collect()
    }

    /// Returns every matching POI within `radius` of `center`, closest first
    /// (vanilla `PoiManager.findAllClosestFirstWithType`).
    #[must_use]
    pub fn find_all_closest_first_with_type(
        &self,
        predicate: PoiTypePredicate<'_>,
        center: BlockPos,
        radius: i32,
        status: OccupationStatus,
    ) -> Vec<(PoiTypeRef, BlockPos)> {
        let mut records = self.get_in_range(predicate, center, radius, status);
        records.sort_by_key(|(_, pos)| distance_sq(*pos, center));
        records
    }

    /// Returns the closest matching POI within `radius` of `center`
    /// (vanilla `PoiManager.findClosest`).
    #[must_use]
    pub fn find_closest(
        &self,
        predicate: PoiTypePredicate<'_>,
        center: BlockPos,
        radius: i32,
        status: OccupationStatus,
    ) -> Option<BlockPos> {
        self.find_closest_with_type(predicate, center, radius, status)
            .map(|(_, pos)| pos)
    }

    /// Vanilla `PoiManager.findClosestWithType`.
    #[must_use]
    pub fn find_closest_with_type(
        &self,
        predicate: PoiTypePredicate<'_>,
        center: BlockPos,
        radius: i32,
        status: OccupationStatus,
    ) -> Option<(PoiTypeRef, BlockPos)> {
        self.get_in_range(predicate, center, radius, status)
            .into_iter()
            .min_by_key(|(_, pos)| distance_sq(*pos, center))
    }

    /// Reserves a ticket on the first free matching POI within `radius` of `center`
    /// and returns its position (vanilla `PoiManager.take`).
    ///
    /// The returned POI is only *reserved*; the caller records it (usually as a
    /// `GlobalPos` memory) and releases it with [`Self::release`]. Vanilla tracks no
    /// ticket owner, so neither does the storage: releasing a ticket that a different
    /// entity took is possible, and the owning behaviors must only release POIs they
    /// claimed.
    pub fn take(
        &mut self,
        predicate: PoiTypePredicate<'_>,
        filter: &dyn Fn(PoiTypeRef, BlockPos) -> bool,
        center: BlockPos,
        radius: i32,
    ) -> Option<BlockPos> {
        let target = self
            .get_in_range(predicate, center, radius, OccupationStatus::Free)
            .into_iter()
            .find(|(poi_type, pos)| filter(poi_type, *pos))
            .map(|(_, pos)| pos)?;
        self.reserve_ticket(target).then_some(target)
    }

    /// Releases a ticket at `pos` (vanilla `PoiManager.release`).
    ///
    /// Returns `false` when no POI is registered at `pos`, or when every ticket was
    /// already free. Vanilla throws an `IllegalStateException` for the first case;
    /// Steel reports it instead, because the storage is per-world while a villager's
    /// remembered POI may belong to an unloaded chunk.
    pub fn release(&mut self, pos: BlockPos) -> bool {
        self.release_ticket(pos)
    }

    /// Returns a random matching POI within `radius` of `center`
    /// (vanilla `PoiManager.getRandom`).
    ///
    /// Vanilla shuffles the whole candidate stream with `random` and then applies
    /// `filter` to the shuffled list, so different entities with different RNGs pick
    /// different POIs.
    #[must_use]
    pub fn get_random(
        &self,
        predicate: PoiTypePredicate<'_>,
        filter: &dyn Fn(BlockPos) -> bool,
        center: BlockPos,
        radius: i32,
        status: OccupationStatus,
        random: &mut impl Random,
    ) -> Option<BlockPos> {
        let mut candidates = self.get_in_range(predicate, center, radius, status);
        shuffle(&mut candidates, random);
        candidates
            .into_iter()
            .find(|(_, pos)| filter(*pos))
            .map(|(_, pos)| pos)
    }

    /// Returns whether `section` holds an occupied POI of the `village` tag
    /// (vanilla `PoiManager.isVillageCenter`).
    #[must_use]
    pub fn is_village_center(&self, section: SectionPos) -> bool {
        self.get_in_chunk(
            &is_village_poi_type,
            section.x(),
            section.z(),
            OccupationStatus::Occupied,
        )
        .into_iter()
        .any(|(pos, _)| SectionPos::from_block_pos(pos) == section)
    }

    /// Returns the vanilla distance-tracker level of `section`
    /// (vanilla `PoiManager.sectionsToVillage`): the hop distance to the closest village
    /// center section, or [`MAX_VILLAGE_DISTANCE`] + 1 when no village is in range.
    #[must_use]
    pub fn sections_to_village(&self, section: SectionPos) -> i32 {
        self.village_distance_levels(section, MAX_VILLAGE_DISTANCE)
            .get(&section)
            .copied()
            .unwrap_or(UNTRACKED_VILLAGE_DISTANCE)
    }

    /// Computes the distance-tracker levels for every section within
    /// [`MAX_VILLAGE_DISTANCE`] of `section`.
    ///
    /// `seed_radius` widens the village-center scan beyond the BFS range, which is what
    /// querying a whole neighborhood needs: a section `d` further away can still be
    /// within the BFS range of a center that is `d` sections further out. Sections at
    /// level > [`MAX_VILLAGE_DISTANCE`] are absent, exactly like vanilla's tracker,
    /// which drops them from its map.
    fn village_distance_levels(
        &self,
        section: SectionPos,
        seed_radius: i32,
    ) -> FxHashMap<SectionPos, i32> {
        let mut levels: FxHashMap<SectionPos, i32> = FxHashMap::default();
        let mut queue: VecDeque<SectionPos> = VecDeque::new();

        for seed in self.village_center_sections_near(section, seed_radius) {
            levels.insert(seed, 0);
            queue.push_back(seed);
        }

        while let Some(current) = queue.pop_front() {
            let level = levels[&current];
            if level >= MAX_VILLAGE_DISTANCE {
                continue;
            }
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        if dx == 0 && dy == 0 && dz == 0 {
                            continue;
                        }
                        let neighbor =
                            SectionPos::new(current.x() + dx, current.y() + dy, current.z() + dz);
                        if levels.contains_key(&neighbor) {
                            continue;
                        }
                        levels.insert(neighbor, level + 1);
                        queue.push_back(neighbor);
                    }
                }
            }
        }

        levels
    }

    /// Collects the village-center sections within `radius` sections of `section`.
    fn village_center_sections_near(
        &self,
        section: SectionPos,
        radius: i32,
    ) -> FxHashSet<SectionPos> {
        let mut centers = FxHashSet::default();

        for chunk_x in section.x() - radius..=section.x() + radius {
            for chunk_z in section.z() - radius..=section.z() + radius {
                for (pos, _) in self.get_in_chunk(
                    &is_village_poi_type,
                    chunk_x,
                    chunk_z,
                    OccupationStatus::Occupied,
                ) {
                    let center = SectionPos::from_block_pos(pos);
                    if (center.x() - section.x()).abs() <= radius
                        && (center.y() - section.y()).abs() <= radius
                        && (center.z() - section.z()).abs() <= radius
                    {
                        centers.insert(center);
                    }
                }
            }
        }

        centers
    }
}

impl World {
    /// Returns the type of the POI registered at `pos`
    /// (vanilla `PoiManager.getType`).
    #[must_use]
    pub fn poi_type_at(&self, pos: BlockPos) -> Option<PoiTypeRef> {
        self.poi_storage.lock().poi_type(pos)
    }

    /// Returns whether this position is in a village
    /// (vanilla `ServerLevel.isVillage`).
    #[must_use]
    pub fn is_village(&self, pos: BlockPos) -> bool {
        self.is_close_to_village(pos, 1)
    }

    /// Returns whether this section's center is in a village
    /// (vanilla `ServerLevel.isVillage(SectionPos)`).
    #[must_use]
    pub fn is_village_section(&self, section: SectionPos) -> bool {
        self.is_village(section_center(section))
    }

    /// Returns whether `pos` is at most `section_distance` sections from a village
    /// (vanilla `ServerLevel.isCloseToVillage`).
    #[must_use]
    pub fn is_close_to_village(&self, pos: BlockPos, section_distance: i32) -> bool {
        section_distance <= MAX_VILLAGE_DISTANCE
            && self.sections_to_village(SectionPos::from_block_pos(pos)) <= section_distance
    }

    /// Vanilla `ServerLevel.sectionsToVillage`.
    #[must_use]
    pub fn sections_to_village(&self, section: SectionPos) -> i32 {
        self.poi_storage.lock().sections_to_village(section)
    }

    /// Releases the POI ticket at `pos` (vanilla `PoiManager.release`).
    pub fn release_poi(&self, pos: BlockPos) -> bool {
        self.poi_storage.lock().release(pos)
    }
}

/// Vanilla `SectionPos.center`: the block position at the center of `section`.
#[must_use]
pub const fn section_center(section: SectionPos) -> BlockPos {
    BlockPos::new(
        section.x() * 16 + 8,
        section.y() * 16 + 8,
        section.z() * 16 + 8,
    )
}

/// Iterates every section within `radius` of `center` (vanilla `SectionPos.cube`).
#[must_use]
pub fn sections_in_cube(center: SectionPos, radius: i32) -> Vec<SectionPos> {
    let mut sections = Vec::new();
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                sections.push(SectionPos::new(
                    center.x() + dx,
                    center.y() + dy,
                    center.z() + dz,
                ));
            }
        }
    }
    sections
}

/// Returns the section in the `radius` cube around `center` that is closest to a
/// village, or `center` when none is strictly closer
/// (vanilla `BehaviorUtils.findSectionClosestToVillage`).
#[must_use]
pub fn find_section_closest_to_village(
    world: &World,
    center: SectionPos,
    radius: i32,
) -> SectionPos {
    let levels = world
        .poi_storage
        .lock()
        .village_distance_levels(center, MAX_VILLAGE_DISTANCE + radius.max(0));
    let distance_of = |section: SectionPos| {
        levels
            .get(&section)
            .copied()
            .unwrap_or(UNTRACKED_VILLAGE_DISTANCE)
    };

    let current = distance_of(center);
    sections_in_cube(center, radius)
        .into_iter()
        .map(|section| (section, distance_of(section)))
        .filter(|(_, distance)| *distance < current)
        .min_by_key(|(_, distance)| *distance)
        .map_or(center, |(section, _)| section)
}

#[cfg(test)]
mod tests;
