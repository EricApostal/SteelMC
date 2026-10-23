use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::blocks::BlockRef;
use crate::items::ItemRef;
use crate::poi::PoiTypeRef;
use crate::registry::TaggedRegistryExt;
use crate::sound_event::SoundEventRef;
use crate::trade::TradeSet;
use crate::vanilla_poi_type_tags::PoiTag;

#[derive(Debug)]
pub struct VillagerProfession {
    pub key: Identifier,
    pub work_sound: Option<SoundEventRef>,
    /// Vanilla `VillagerProfession.heldJobSite` for professions whose job site is
    /// a single POI type.
    ///
    /// Vanilla passes the matching `PoiTypes` entry for every job-site profession
    /// (e.g. `FARMER -> PoiTypes.FARMER`), so this is derived from the POI types
    /// that share the profession's key; `none`/`nitwit` have no job site.
    pub held_job_site: Option<PoiTypeRef>,
    /// Vanilla `VillagerProfession.requestedItems` (`ImmutableSet<Item>`).
    ///
    /// Vanilla hard-codes these in `VillagerProfession.bootstrap` (farmer:
    /// `WHEAT, WHEAT_SEEDS, BEETROOT_SEEDS, BONE_MEAL`); every other profession
    /// passes an empty set. They are not datapack or registry JSON data, so the
    /// vendored extractor output cannot supply them and the build generator reads
    /// them from `build_assets/villager_professions.json` instead.
    pub requested_items: &'static [ItemRef],
    /// Vanilla `VillagerProfession.secondaryPoi` (`ImmutableSet<Block>`).
    ///
    /// Vanilla hard-codes these in `VillagerProfession.bootstrap` (farmer:
    /// `FARMLAND`); every other profession passes an empty set. Same provenance
    /// note as [`Self::requested_items`].
    pub secondary_poi: &'static [BlockRef],
    /// Vanilla `VillagerProfession.tradeSetsByLevel`, derived from the
    /// `minecraft:trade_set` entries named `<profession>/level_<n>`.
    pub trade_sets_by_level: &'static [(i32, &'static TradeSet)],
}

pub type VillagerProfessionRef = &'static VillagerProfession;

impl VillagerProfession {
    /// Vanilla `VillagerProfession.heldJobSite().test(poiType)`.
    #[must_use]
    pub fn is_held_job_site(&self, poi_type: PoiTypeRef) -> bool {
        self.held_job_site
            .is_some_and(|held| held.key == poi_type.key)
    }

    /// Vanilla `VillagerProfession.ALL_ACQUIRABLE_JOBS`:
    /// `poiType.is(PoiTypeTags.ACQUIRABLE_JOB_SITE)`.
    #[must_use]
    pub fn is_acquirable_job_site(poi_type: PoiTypeRef) -> bool {
        crate::REGISTRY
            .poi_types
            .is_in_tag(poi_type, &PoiTag::ACQUIRABLE_JOB_SITE)
    }

    /// Vanilla `PoiTypes.MEETING` membership used for `MEETING_POINT` memories.
    #[must_use]
    pub fn is_meeting_point(poi_type: PoiTypeRef) -> bool {
        poi_type.key == crate::vanilla_poi_types::MEETING.key
    }
}

pub struct VillagerProfessionRegistry {
    villager_professions_by_id: Vec<VillagerProfessionRef>,
    villager_professions_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl VillagerProfessionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            villager_professions_by_id: Vec::new(),
            villager_professions_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    VillagerProfessionRegistry,
    VillagerProfessionRef,
    villager_professions_by_id,
    villager_professions_by_key,
    allows_registering
);

crate::impl_registry!(
    VillagerProfessionRegistry,
    VillagerProfession,
    villager_professions_by_id,
    villager_professions_by_key,
    villager_professions
);
