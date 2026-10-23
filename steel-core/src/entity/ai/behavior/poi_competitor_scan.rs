//! Vanilla `PoiCompetitorScan`.

use super::poi;
use super::villager;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::{LivingEntity, SharedEntity};

/// Vanilla `PoiCompetitorScan`: among nearby villagers claiming the same job site,
/// the higher-xp villager wins and all losers erase their `JOB_SITE`.
///
/// Deviation (foundation gap): vanilla reads each candidate's `Brain.JOB_SITE` and
/// calls `loser.getBrain().eraseMemory(JOB_SITE)`. Steel behaviors can only read
/// their own memories, so candidate sites are matched through the villager's
/// profession-held POI type at this site instead of its memory, and only the body
/// erases its own `JOB_SITE` when a strictly higher-xp competitor of the same
/// profession is nearby. Reported to Main.
pub struct PoiCompetitorScan;

impl PoiCompetitorScan {
    /// Vanilla `PoiCompetitorScan.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self)
            .with_required_memories(vec![&mem::JOB_SITE, &mem::NEAREST_LIVING_ENTITIES])
    }
}

impl Trigger for PoiCompetitorScan {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::JOB_SITE, MemoryStatus::ValuePresent)
            || !ctx.check_memory(&mem::NEAREST_LIVING_ENTITIES, MemoryStatus::ValuePresent)
        {
            return false;
        }

        let Some(site) = ctx.get_memory(&mem::JOB_SITE) else {
            return false;
        };
        let Some(poi_type) = poi::poi_type_at(ctx.level, site.pos) else {
            return true;
        };
        let Some(v) = villager::villager(ctx.level, ctx.body) else {
            return true;
        };
        let (own_xp, own_profession) = (v.villager_xp(), v.profession().key.clone());

        let nearby: Vec<SharedEntity> = ctx
            .get_memory(&mem::NEAREST_LIVING_ENTITIES)
            .unwrap_or_default();

        let outperformed = nearby.iter().any(|entity| {
            if std::sync::Arc::ptr_eq(entity, ctx.body) {
                return false;
            }
            let Some(other) = villager::as_villager(entity) else {
                return false;
            };
            if !entity
                .as_living_entity()
                .is_some_and(LivingEntity::is_alive)
            {
                return false;
            }
            if other.profession().key != own_profession {
                return false;
            }
            if !other
                .profession()
                .held_job_site
                .is_some_and(|held| held.key == poi_type.key)
            {
                return false;
            }
            other.villager_xp() > own_xp
        });

        if outperformed {
            ctx.erase_memory(&mem::JOB_SITE);
        }

        true
    }
}
