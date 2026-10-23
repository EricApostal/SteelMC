//! Vanilla `SetEntityLookTarget`: look at the closest visible entity of a type or
//! mob category.

use std::sync::Arc;

use steel_registry::entity_type::{EntityTypeRef, MobCategory};

use super::utils;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `SetEntityLookTarget.create(Predicate, maxDist)` test.
#[derive(Clone)]
pub enum EntityLookTargetFilter {
    /// Vanilla `mob -> true`.
    Any,
    /// Vanilla `mob -> mob.is(type)`.
    Type(EntityTypeRef),
    /// Vanilla `mob -> category.equals(mob.getType().getCategory())`.
    Category(MobCategory),
    /// Vanilla `mob -> predicate.test(mob)`.
    Predicate(Arc<dyn Fn(&SharedEntity) -> bool + Send + Sync>),
}

impl EntityLookTargetFilter {
    #[must_use]
    pub fn matches(&self, entity: &SharedEntity) -> bool {
        match self {
            Self::Any => true,
            Self::Type(entity_type) => entity.entity_type() == *entity_type,
            Self::Category(category) => entity.entity_type().mob_category == *category,
            Self::Predicate(predicate) => predicate(entity),
        }
    }
}

/// Vanilla `SetEntityLookTarget.create(Predicate, maxDist)`.
pub struct SetEntityLookTarget {
    filter: EntityLookTargetFilter,
    max_dist_sqr: f64,
}

impl SetEntityLookTarget {
    #[must_use]
    pub fn new(filter: EntityLookTargetFilter, max_dist: f32) -> Self {
        Self {
            filter,
            max_dist_sqr: f64::from(max_dist) * f64::from(max_dist),
        }
    }

    /// Vanilla `SetEntityLookTarget.create(EntityType, maxDist)`.
    #[must_use]
    pub fn for_type(entity_type: EntityTypeRef, max_dist: f32) -> OneShot<Self> {
        Self::boxed_for_filter(EntityLookTargetFilter::Type(entity_type), max_dist)
    }

    /// Vanilla `SetEntityLookTarget.create(MobCategory, maxDist)`.
    #[must_use]
    pub fn for_category(category: MobCategory, max_dist: f32) -> OneShot<Self> {
        Self::boxed_for_filter(EntityLookTargetFilter::Category(category), max_dist)
    }

    /// Vanilla `SetEntityLookTarget.create(maxDist)`.
    #[must_use]
    pub fn any(max_dist: f32) -> OneShot<Self> {
        Self::boxed_for_filter(EntityLookTargetFilter::Any, max_dist)
    }

    /// Vanilla `SetEntityLookTarget.create(Predicate, maxDist)`.
    #[must_use]
    pub fn create_with_predicate(
        predicate: impl Fn(&SharedEntity) -> bool + Send + Sync + 'static,
        max_dist: f32,
    ) -> OneShot<Self> {
        Self::boxed_for_filter(
            EntityLookTargetFilter::Predicate(Arc::new(predicate)),
            max_dist,
        )
    }

    fn boxed_for_filter(filter: EntityLookTargetFilter, max_dist: f32) -> OneShot<Self> {
        OneShot::new(Self::new(filter, max_dist)).with_required_memories(vec![
            &mem::LOOK_TARGET,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ])
    }
}

impl Trigger for SetEntityLookTarget {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.has_memory_value(&mem::LOOK_TARGET) {
            return false;
        }

        let Some(visible) = ctx.memories().get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES) else {
            return false;
        };

        let body = ctx.body;
        let filter = &self.filter;
        let max_dist_sqr = self.max_dist_sqr;
        let Some(target) = visible.find_closest(|entity| {
            filter.matches(entity)
                && entity.distance_to_sqr(body.position()) <= max_dist_sqr
                && !body.has_passenger(entity.as_ref())
        }) else {
            return false;
        };

        utils::set_look_target(ctx, Arc::new(EntityTracker::new(target, true)));
        true
    }
}
