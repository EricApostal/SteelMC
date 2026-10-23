//! Village systems shared by villagers, iron golems and raids (vanilla
//! `net.minecraft.world.entity.ai.village`).
//!
//! Steel keeps no separate `PoiManager` object: [`World::poi_storage`] already is the
//! per-level POI store, so the vanilla `PoiManager` query API is provided here as
//! inherent methods on [`PointOfInterestStorage`](crate::poi::PointOfInterestStorage)
//! (see [`poi`]), together with the village-distance tracker and the level-level helpers
//! (vanilla `ServerLevel.isVillage`, `BehaviorUtils.findSectionClosestToVillage`).
pub mod bed;
pub mod bell;
pub mod golem;
pub mod golem_build;
pub mod poi;
pub mod reputation;

pub use bed::kick_villager_out_of_bed;
pub use bell::{HEAR_BELL_RADIUS, SEARCH_RADIUS, notify_heard_bell, wake_sleeping_villagers};
pub use golem::{
    GOLEM_DETECTED_TTL, GOLEM_SLEEP_WINDOW, MAX_AGREEING_VILLAGERS, SPAWN_ATTEMPTS, SPAWN_RANGE_XZ,
    SPAWN_RANGE_Y, golem_detected, spawn_golem_if_needed, villager_wants_golem,
    wants_to_spawn_golem,
};
pub use golem_build::{can_spawn_golem, try_spawn_golem};
pub use poi::{
    MAX_VILLAGE_DISTANCE, PoiTypePredicate, find_section_closest_to_village, is_village_poi,
    section_center, sections_in_cube,
};
