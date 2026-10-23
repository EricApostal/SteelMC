//! Vanilla `SetEntityLookTargetSometimes`: occasionally glance at a nearby entity.

use std::sync::Arc;

use steel_registry::entity_type::EntityTypeRef;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `SetEntityLookTargetSometimes.Ticker`: counts down the interval and
/// returns `true` on the tick the look target should be set.
pub struct Ticker {
    interval: UniformIntProvider,
    ticks_until_next_start: i32,
}

impl Ticker {
    /// Vanilla `Ticker(UniformInt interval)`.
    ///
    /// # Panics
    ///
    /// Panics when `interval.min_inclusive <= 1`, mirroring vanilla.
    #[must_use]
    pub fn new(interval: UniformIntProvider) -> Self {
        assert!(interval.min_inclusive > 1, "interval must start above 1");
        Self {
            interval,
            ticks_until_next_start: 0,
        }
    }

    /// Vanilla `Ticker.tickDownAndCheck`.
    ///
    /// Vanilla samples with `level.getRandom()`; Steel's behavior RNG convention is
    /// `rand::random_range` over the same inclusive bounds.
    pub fn tick_down_and_check(&mut self) -> bool {
        if self.ticks_until_next_start == 0 {
            self.ticks_until_next_start =
                rand::random_range(self.interval.min_inclusive..=self.interval.max_inclusive) - 1;
            false
        } else {
            self.ticks_until_next_start -= 1;
            self.ticks_until_next_start == 0
        }
    }
}

/// Vanilla `SetEntityLookTargetSometimes` entity filter.
#[derive(Clone, Copy)]
pub enum LookTargetFilter {
    /// Vanilla `mob -> true`.
    Any,
    /// Vanilla `mob -> mob.is(type)`.
    Type(EntityTypeRef),
}

impl LookTargetFilter {
    fn matches(self, entity: &SharedEntity) -> bool {
        match self {
            Self::Any => true,
            Self::Type(entity_type) => entity.entity_type() == entity_type,
        }
    }
}

/// Vanilla `SetEntityLookTargetSometimes`.
pub struct SetEntityLookTargetSometimes {
    filter: LookTargetFilter,
    max_dist_sqr: f64,
    ticker: Ticker,
}

impl SetEntityLookTargetSometimes {
    /// Vanilla `SetEntityLookTargetSometimes.create(maxDist, interval)`.
    #[must_use]
    pub fn create(max_dist: f32, interval: UniformIntProvider) -> OneShot<Self> {
        Self::create_with_filter(LookTargetFilter::Any, max_dist, interval)
    }

    /// Vanilla `SetEntityLookTargetSometimes.create(EntityType, maxDist, interval)`.
    #[must_use]
    pub fn for_type(
        entity_type: EntityTypeRef,
        max_dist: f32,
        interval: UniformIntProvider,
    ) -> OneShot<Self> {
        Self::create_with_filter(LookTargetFilter::Type(entity_type), max_dist, interval)
    }

    fn create_with_filter(
        filter: LookTargetFilter,
        max_dist: f32,
        interval: UniformIntProvider,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            filter,
            max_dist_sqr: f64::from(max_dist) * f64::from(max_dist),
            ticker: Ticker::new(interval),
        })
        .with_required_memories(vec![
            &mem::LOOK_TARGET,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ])
    }
}

impl Trigger for SetEntityLookTargetSometimes {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.check_memory(&mem::LOOK_TARGET, MemoryStatus::ValuePresent) {
            return false;
        }

        let body = ctx.body;
        let body_position = body.position();
        let filter = self.filter;
        let max_dist_sqr = self.max_dist_sqr;
        let Some(target) = ctx
            .memories()
            .get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES)
            .and_then(|visible| {
                visible.find_closest(|entity| {
                    filter.matches(entity) && entity.distance_to_sqr(body_position) <= max_dist_sqr
                })
            })
        else {
            return false;
        };

        if !self.ticker.tick_down_and_check() {
            return false;
        }

        ctx.set_memory(
            &mem::LOOK_TARGET,
            Arc::new(EntityTracker::new(target, true)),
        );
        true
    }
}
