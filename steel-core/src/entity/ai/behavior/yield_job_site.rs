//! Vanilla `YieldJobSite`.

use std::sync::Arc;

use steel_registry::poi::PoiTypeRef;
use steel_registry::vanilla_villager_professions;

use super::poi;
use super::utils;
use super::villager;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::{LivingEntity, SharedEntity};

/// Vanilla `YieldJobSite`: an unemployed villager yields its potential job site to a
/// nearby villager that wants it.
///
/// Deviation (foundation gap): vanilla writes the *other* villager's `WALK_TARGET`,
/// `LOOK_TARGET` and `POTENTIAL_JOB_SITE` memories and reads its `JOB_SITE` and
/// `POTENTIAL_JOB_SITE`. Steel behaviors only receive their own memory store, so the
/// other-villager half cannot be performed and the "other already has a site" checks
/// cannot be read; the body erases its own targets and walks to the site itself.
/// Reported to Main.
pub struct YieldJobSite {
    speed_modifier: f32,
}

impl YieldJobSite {
    /// Vanilla `YieldJobSite.create(speedModifier)`.
    #[must_use]
    pub fn create(speed_modifier: f32) -> OneShot<Self> {
        OneShot::new(Self { speed_modifier }).with_required_memories(vec![
            &mem::POTENTIAL_JOB_SITE,
            &mem::JOB_SITE,
            &mem::NEAREST_LIVING_ENTITIES,
            &mem::WALK_TARGET,
            &mem::LOOK_TARGET,
        ])
    }

    /// Vanilla `YieldJobSite.nearbyWantsJobsite`.
    fn nearby_wants_job_site(
        poi_type: PoiTypeRef,
        nearby: &SharedEntity,
        site: steel_utils::BlockPos,
    ) -> bool {
        let Some(other) = villager::as_villager(nearby) else {
            return false;
        };
        if other
            .profession()
            .held_job_site
            .is_none_or(|held| held.key != poi_type.key)
        {
            return false;
        }
        if !nearby
            .as_living_entity()
            .is_some_and(LivingEntity::is_alive)
        {
            return false;
        }
        // The `JOB_SITE`/`POTENTIAL_JOB_SITE` memory checks are unreadable here; the
        // reachability test remains, so only a villager that can path to the site is
        // treated as a taker.
        can_reach_pos(nearby, site, poi_type)
    }
}

/// Vanilla `YieldJobSite.canReachPos`.
fn can_reach_pos(nearby: &SharedEntity, site: steel_utils::BlockPos, poi_type: PoiTypeRef) -> bool {
    let Some(mob) = nearby.as_pathfinder_mob() else {
        return false;
    };
    mob.create_path_to(site, poi_type.search_distance as i32)
        .is_some_and(|path| path.can_reach())
}

impl Trigger for YieldJobSite {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let is_baby = ctx
            .body
            .as_living_entity()
            .is_some_and(LivingEntity::is_baby);
        if is_baby
            || !ctx.check_memory(&mem::POTENTIAL_JOB_SITE, MemoryStatus::ValuePresent)
            || !ctx.check_memory(&mem::JOB_SITE, MemoryStatus::ValueAbsent)
            || !ctx.check_memory(&mem::NEAREST_LIVING_ENTITIES, MemoryStatus::ValuePresent)
        {
            return false;
        }

        let employed = villager::profession_of(ctx.body)
            .is_some_and(|profession| profession.key != vanilla_villager_professions::NONE.key);
        if employed {
            return false;
        }

        let Some(site) = ctx.get_memory(&mem::POTENTIAL_JOB_SITE) else {
            return false;
        };
        let Some(poi_type) = poi::poi_type_at(ctx.level, site.pos) else {
            return true;
        };

        let nearby: Vec<SharedEntity> = ctx
            .get_memory(&mem::NEAREST_LIVING_ENTITIES)
            .unwrap_or_default();
        let wants = nearby.iter().any(|entity| {
            !Arc::ptr_eq(entity, ctx.body)
                && Self::nearby_wants_job_site(poi_type, entity, site.pos)
        });

        if wants {
            utils::clear_walk_target(ctx);
            utils::clear_look_target(ctx);
            ctx.erase_memory(&mem::POTENTIAL_JOB_SITE);
            let speed = self.speed_modifier;
            utils::set_walk_and_look_target_to_pos(ctx, site.pos, speed, 1);
        }

        true
    }
}
