//! Vanilla ocelot AI pieces that are private inner classes of `Ocelot`.
//!
//! `Ocelot.OcelotAvoidEntityGoal` and `Ocelot.OcelotTemptGoal` are private vanilla inner
//! classes. `OcelotAttackGoal` and `LeapAtTargetGoal` are shared goals and are used from
//! `entity/ai/goal` instead.

use steel_utils::Downcast as _;

use crate::entity::ai::goal::{AvoidEntityGoal, Goal, GoalControls, TemptGoal};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `Ocelot.OcelotAvoidEntityGoal`: only untrusting ocelots flee players.
pub(super) struct OcelotAvoidEntityGoal {
    inner: AvoidEntityGoal,
}

impl OcelotAvoidEntityGoal {
    pub(super) fn new(max_dist: f32, walk_speed_modifier: f64, sprint_speed_modifier: f64) -> Self {
        Self {
            inner: AvoidEntityGoal::new(max_dist, walk_speed_modifier, sprint_speed_modifier),
        }
    }
}

impl Goal for OcelotAvoidEntityGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        // Vanilla removes this goal once trusting; vetoing here is equivalent because the
        // goal is only ever entered through `canUse`.
        !is_trusting(mob) && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !is_trusting(mob) && self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `Ocelot.OcelotTemptGoal`: trusting ocelots are no longer scared by the holder.
///
/// GAP: vanilla only suppresses the scare when the tempting player is the one that started
/// the goal (`OcelotTemptGoal.canScare`). Steel's shared `TemptGoal` takes a fixed
/// `can_scare` flag with no override hook, so the suppression is unconditional here.
pub(super) struct OcelotTemptGoal {
    inner: TemptGoal,
}

impl OcelotTemptGoal {
    pub(super) fn new(
        speed_modifier: f64,
        items: impl Fn(&steel_registry::item_stack::ItemStack) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: TemptGoal::new(speed_modifier, items, true),
        }
    }
}

impl Goal for OcelotTemptGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Reads the ocelot trusting flag through the concrete entity.
fn is_trusting(mob: &dyn PathfinderMob) -> bool {
    mob.as_entity_event_source()
        .downcast_ref::<super::OcelotEntity>()
        .is_some_and(super::OcelotEntity::is_trusting)
}
