//! Vanilla `AssignProfessionFromJobSite`.

use steel_registry::vanilla_villager_professions;
use steel_utils::entity_events::EntityStatus;

use super::poi;
use super::villager;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `AssignProfessionFromJobSite`: standing on the potential job site claims
/// it as the villager's job site and, for a jobless villager, assigns the profession
/// of the claiming POI type.
pub struct AssignProfessionFromJobSite;

/// Vanilla `AssignProfessionFromJobSite` reach distance.
const CLAIM_DISTANCE: f64 = 2.0;

impl AssignProfessionFromJobSite {
    /// Vanilla `AssignProfessionFromJobSite.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![&mem::POTENTIAL_JOB_SITE, &mem::JOB_SITE])
    }
}

impl Trigger for AssignProfessionFromJobSite {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::POTENTIAL_JOB_SITE, MemoryStatus::ValuePresent) {
            return false;
        }
        let Some(potential) = ctx.get_memory(&mem::POTENTIAL_JOB_SITE) else {
            return false;
        };
        if !potential
            .pos
            .closer_to_center_than(ctx.body.position(), CLAIM_DISTANCE)
        {
            return false;
        }

        ctx.erase_memory(&mem::POTENTIAL_JOB_SITE);
        ctx.set_memory(&mem::JOB_SITE, potential.clone());
        ctx.body.broadcast_entity_event(EntityStatus::VillagerHappy);

        let Some(villager) = villager::villager(ctx.level, ctx.body) else {
            return true;
        };
        if villager.profession().key != vanilla_villager_professions::NONE.key {
            return true;
        }

        if let Some(poi_type) = poi::poi_type_at(ctx.level, potential.pos) {
            let _ = villager.apply_profession_from_poi(poi_type);
        }

        true
    }
}
