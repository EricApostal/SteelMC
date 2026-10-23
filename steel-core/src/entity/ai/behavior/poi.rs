//! Point-of-interest access shared by the villager behaviors.
//!
//! Vanilla keeps these operations on `PoiManager` (queries, ticket taking) and on
//! `Villager` (`POI_MEMORIES`, `releasePoi`, `AcquirePoi.findPathToPois`). Steel
//! expresses them as free functions over `World::poi_storage` so the behavior
//! library never needs entity-side POI state.

use std::sync::Arc;

use steel_registry::poi::PoiTypeRef;
use steel_registry::registry::TaggedRegistryExt as _;
use steel_registry::villager_profession::VillagerProfessionRef;
use steel_registry::{REGISTRY, vanilla_poi_types};
use steel_utils::{BlockPos, GlobalPos, Identifier};

use super::super::path::Path;
use crate::entity::PathfinderMob;
use crate::world::World;

/// Vanilla `PoiManager.Occupancy`.
pub use crate::poi::poi_storage::OccupationStatus as Occupancy;

/// Vanilla `Predicate<Holder<PoiType>>`.
///
/// A closed enum keeps POI predicates comparable and `Copy` instead of boxing a
/// closure per behavior instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PoiPredicate {
    /// Matches every POI type, vanilla `p -> true`.
    Any,
    /// Matches no POI type, vanilla `PoiType.NONE`.
    Never,
    /// Matches one POI type, vanilla `p -> p.is(poiType)`.
    Type(PoiTypeRef),
    /// Matches a POI type tag, vanilla `p -> p.is(PoiTypeTags.X)`.
    Tag(&'static Identifier),
}

impl PoiPredicate {
    #[must_use]
    pub fn matches(self, poi_type: PoiTypeRef) -> bool {
        match self {
            Self::Any => true,
            Self::Never => false,
            Self::Type(expected) => expected.key == poi_type.key,
            Self::Tag(tag) => REGISTRY.poi_types.is_in_tag(poi_type, tag),
        }
    }

    /// Vanilla `Holder<PoiType>` predicate for the POI storage queries.
    #[must_use]
    pub fn as_query(self) -> impl Fn(PoiTypeRef) -> bool {
        move |poi_type| self.matches(poi_type)
    }
}

/// Vanilla `Villager.POI_MEMORIES`: the POI memories whose release is gated on the
/// POI type still matching the villager's profession.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoiMemory {
    Home,
    JobSite,
    PotentialJobSite,
    MeetingPoint,
}

impl PoiMemory {
    /// Vanilla `Villager.POI_MEMORIES.get(memoryType)`.
    #[must_use]
    pub fn predicate(self, profession: VillagerProfessionRef) -> PoiPredicate {
        match self {
            Self::Home => PoiPredicate::Type(&vanilla_poi_types::HOME),
            Self::JobSite => profession_held_job_site(profession),
            Self::PotentialJobSite => profession_acquirable_job_site(profession),
            Self::MeetingPoint => PoiPredicate::Type(&vanilla_poi_types::MEETING),
        }
    }
}

/// Vanilla `VillagerProfession.heldJobSite()`: for every job-site profession this is
/// the POI type sharing the profession's key (`armorer` → `PoiTypes.ARMORER`), and
/// `PoiType.NONE` for `none`/`nitwit`.
#[must_use]
pub fn profession_held_job_site(profession: VillagerProfessionRef) -> PoiPredicate {
    match profession.held_job_site {
        Some(poi_type) => PoiPredicate::Type(poi_type),
        None => PoiPredicate::Never,
    }
}

/// Vanilla `VillagerProfession.ALL_ACQUIRABLE_JOBS`' `#minecraft:acquirable_job_site`.
static ACQUIRABLE_JOB_SITE_TAG: std::sync::LazyLock<Identifier> =
    std::sync::LazyLock::new(|| steel_registry::vanilla_poi_type_tags::PoiTag::ACQUIRABLE_JOB_SITE);

/// Vanilla `VillagerProfession.acquirableJobSite()`.
///
/// Vanilla registers each job-site profession with `poiType -> poiType.is(jobSite)`,
/// so an employed villager only re-acquires its own workstation; `minecraft:none`
/// uses `ALL_ACQUIRABLE_JOBS` (`#minecraft:acquirable_job_site`) and
/// `minecraft:nitwit` matches nothing.
#[must_use]
pub fn profession_acquirable_job_site(profession: VillagerProfessionRef) -> PoiPredicate {
    if profession.held_job_site.is_some() {
        return profession_held_job_site(profession);
    }

    if profession.key.path == "nitwit" {
        PoiPredicate::Never
    } else {
        PoiPredicate::Tag(&ACQUIRABLE_JOB_SITE_TAG)
    }
}

/// Vanilla `PoiManager.getType(pos)`.
#[must_use]
pub fn poi_type_at(level: &World, pos: BlockPos) -> Option<PoiTypeRef> {
    level.poi_storage.lock().poi_type(pos)
}

/// Vanilla `GlobalPos.of(level.dimension(), pos)`.
#[must_use]
pub fn global_pos(level: &World, pos: BlockPos) -> GlobalPos {
    GlobalPos::new(level.key.clone(), pos)
}

/// Vanilla `PoiManager.findAllClosestFirstWithType`, closest first.
#[must_use]
pub fn find_all_closest_first_with_type(
    level: &World,
    poi_type: PoiPredicate,
    filter: impl Fn(BlockPos) -> bool,
    center: BlockPos,
    radius: i32,
    occupancy: Occupancy,
) -> Vec<(BlockPos, PoiTypeRef)> {
    let query = poi_type.as_query();
    level
        .poi_storage
        .lock()
        .find_all_closest_first_with_type(&query, center, radius, occupancy)
        .into_iter()
        .filter(|(_, pos)| filter(*pos))
        .map(|(poi_type, pos)| (pos, poi_type))
        .collect()
}

/// Vanilla `PoiManager.take(typePredicate, posPredicate, pos, distance)`: takes the
/// closest matching free POI and reserves one ticket.
#[must_use]
pub fn take(
    level: &World,
    poi_type: PoiPredicate,
    filter: impl Fn(BlockPos) -> bool,
    center: BlockPos,
    radius: i32,
) -> Option<BlockPos> {
    let query = poi_type.as_query();
    level
        .poi_storage
        .lock()
        .take(&query, &|_, pos| filter(pos), center, radius)
}

/// Vanilla `PoiManager.release(pos)`: returns one ticket to the POI at `pos`.
pub fn release(level: &World, pos: BlockPos) {
    level.poi_storage.lock().release(pos);
}

/// Vanilla `PoiManager.exists(pos, predicate)`.
#[must_use]
pub fn exists(level: &World, pos: BlockPos, poi_type: PoiPredicate) -> bool {
    let query = poi_type.as_query();
    level.poi_storage.lock().exists(pos, &query)
}

/// Vanilla `PoiRecord.isOccupied`: whether every ticket of the POI is reserved.
#[must_use]
pub fn is_poi_occupied(level: &World, pos: BlockPos) -> bool {
    level.poi_storage.lock().is_occupied(pos)
}

/// Vanilla `AcquirePoi.findPathToPois`: paths to the closest coordinate of a
/// `PoiType`-typed position set, using the largest `validRange` of the candidates.
#[must_use]
pub fn find_path_to_pois(mob: &dyn PathfinderMob, pois: &[(BlockPos, PoiTypeRef)]) -> Option<Path> {
    if pois.is_empty() {
        return None;
    }

    let reach_range = pois
        .iter()
        .map(|(_, poi_type)| poi_type.search_distance as i32)
        .max()
        .unwrap_or(1)
        .max(1);

    let world: Arc<World> = mob.level()?;
    let targets: Vec<BlockPos> = pois.iter().map(|(pos, _)| *pos).collect();
    mob.create_path_to_targets(&world, &targets, reach_range)
}
#[cfg(test)]
mod tests {
    use steel_registry::villager_profession::VillagerProfessionRef;
    use steel_registry::{
        RegistryExt as _, init_vanilla_registry, vanilla_poi_types, vanilla_villager_professions,
    };

    use super::*;
    use crate::test_support::fresh_test_world;

    fn home_type_id() -> usize {
        REGISTRY
            .poi_types
            .id_from_key(&vanilla_poi_types::HOME.key)
            .expect("home POI type is registered")
    }

    #[test]
    fn take_claims_a_ticket_and_release_returns_it() {
        init_vanilla_registry();
        let world = fresh_test_world("poi_ticket_claim");
        let pos = BlockPos::new(8, 64, 8);
        world.world.poi_storage.lock().add(
            pos,
            home_type_id(),
            vanilla_poi_types::HOME.ticket_count,
        );

        let home = PoiPredicate::Type(&vanilla_poi_types::HOME);
        let center = BlockPos::new(0, 64, 0);

        assert!(!is_poi_occupied(&world.world, pos));
        assert_eq!(take(&world.world, home, |_| true, center, 48), Some(pos));
        assert!(is_poi_occupied(&world.world, pos));
        assert_eq!(take(&world.world, home, |_| true, center, 48), None);

        release(&world.world, pos);
        assert!(!is_poi_occupied(&world.world, pos));
        assert_eq!(take(&world.world, home, |_| true, center, 48), Some(pos));
    }

    #[test]
    fn take_respects_the_position_filter() {
        init_vanilla_registry();
        let world = fresh_test_world("poi_ticket_filter");
        let wanted = BlockPos::new(2, 64, 2);
        let rejected = BlockPos::new(3, 64, 3);
        {
            let mut storage = world.world.poi_storage.lock();
            storage.add(wanted, home_type_id(), vanilla_poi_types::HOME.ticket_count);
            storage.add(
                rejected,
                home_type_id(),
                vanilla_poi_types::HOME.ticket_count,
            );
        }

        let claimed = take(
            &world.world,
            PoiPredicate::Type(&vanilla_poi_types::HOME),
            |pos| pos == wanted,
            BlockPos::new(0, 64, 0),
            48,
        );
        assert_eq!(claimed, Some(wanted));
        assert!(!is_poi_occupied(&world.world, rejected));
    }

    #[test]
    fn acquirable_job_site_follows_the_profession() {
        init_vanilla_registry();
        let farmer: VillagerProfessionRef = &vanilla_villager_professions::FARMER;
        let none: VillagerProfessionRef = &vanilla_villager_professions::NONE;
        let nitwit: VillagerProfessionRef = &vanilla_villager_professions::NITWIT;

        // An employed villager only re-acquires its own workstation.
        assert_eq!(
            profession_acquirable_job_site(farmer),
            PoiPredicate::Type(&vanilla_poi_types::FARMER)
        );
        assert_eq!(
            profession_held_job_site(farmer),
            PoiPredicate::Type(&vanilla_poi_types::FARMER)
        );

        // `none` takes any acquirable job site, `nitwit` never takes one.
        let any_job = profession_acquirable_job_site(none);
        assert!(any_job.matches(&vanilla_poi_types::ARMORER));
        assert!(!any_job.matches(&vanilla_poi_types::HOME));
        assert_eq!(profession_acquirable_job_site(nitwit), PoiPredicate::Never);
        assert_eq!(profession_held_job_site(nitwit), PoiPredicate::Never);
    }
}
