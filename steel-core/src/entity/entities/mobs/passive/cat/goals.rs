//! Vanilla cat AI pieces that are inner classes of `Cat` (and `TamableAnimal`).
//!
//! `Cat.CatTemptGoal`, `Cat.CatAvoidEntityGoal`, `Cat.CatRelaxOnOwnerGoal` and
//! `TamableAnimal.TamableAnimalPanicGoal` are private vanilla inner classes, so they live
//! here rather than in `entity/ai/goal`. `CatLieOnBlockGoal`, `CatSitOnBlockGoal`,
//! `OcelotAttackGoal` and `LeapAtTargetGoal` are shared goals and are used from
//! `entity/ai/goal` instead.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _};
use steel_utils::{BlockPos, Downcast as _};

use crate::entity::ai::goal::{
    AvoidEntityGoal, Goal, GoalControls, PanicGoal, Tameable, TemptGoal,
};
use crate::entity::{LivingEntity, PathfinderMob, SharedEntity};
use crate::world::World;

/// Vanilla `CatRelaxOnOwnerGoal`: the owner must be within ten blocks.
const OWNER_SEARCH_RANGE_SQR: f64 = 100.0;
/// Vanilla `CatRelaxOnOwnerGoal.tick`: the cat lies down once it is this close to the owner.
const OWNER_CLOSE_RANGE_SQR: f64 = 2.5;
/// Vanilla `CatRelaxOnOwnerGoal.start`: the walk speed towards the bed.
const BED_WALK_SPEED: f64 = 1.1;
/// Vanilla `CatRelaxOnOwnerGoal.tick`: `adjustedTickDelay(16)`.
const LYING_TICKS: i32 = (16 + 1) / 2;
/// Vanilla `CatRelaxOnOwnerGoal.stop`: the owner must have slept at least this long.
const GIFT_MIN_SLEEP_TICKS: i32 = 100;
/// Vanilla `CatRelaxOnOwnerGoal.tick`: look speed and pitch clamp at the owner.
const OWNER_LOOK_SPEED: f32 = 45.0;
const OWNER_LOOK_ANGLE: f32 = 45.0;

/// Vanilla `TamableAnimal.TamableAnimalPanicGoal`: panics, and teleports home when far.
pub(super) struct TamableAnimalPanicGoal {
    inner: PanicGoal,
}

impl TamableAnimalPanicGoal {
    pub(super) const fn new(speed_modifier: f64) -> Self {
        Self {
            inner: PanicGoal::new(speed_modifier),
        }
    }
}

impl Goal for TamableAnimalPanicGoal {
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

    fn is_panic_goal(&self) -> bool {
        self.inner.is_panic_goal()
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
        if let Some(tameable) = mob.as_tameable()
            && !tameable.unable_to_move_to_owner()
            && tameable.should_try_teleport_to_owner()
        {
            tameable.try_to_teleport_to_owner();
        }

        self.inner.tick(mob, entity);
    }
}

/// Vanilla `Cat.CatTemptGoal`: like `TemptGoal`, but tamed cats ignore food.
///
/// GAP: vanilla also suppresses the "scared by the player" behavior for the player it
/// picked as its tempt source (`CatTemptGoal.canScare`). Steel's shared `TemptGoal` takes a
/// fixed `can_scare` flag with no override hook, so the per-player selection is not
/// reproduced.
pub(super) struct CatTemptGoal {
    inner: TemptGoal,
    /// Mirrors `Cat.temptGoal.isRunning()` for the begging sound in `Cat.tick`.
    running: Arc<AtomicBool>,
}

impl CatTemptGoal {
    pub(super) fn new(speed_modifier: f64, running: Arc<AtomicBool>) -> Self {
        Self {
            inner: TemptGoal::new(
                speed_modifier,
                |item_stack| {
                    REGISTRY
                        .items
                        .is_in_tag(item_stack.item(), &ItemTag::CAT_FOOD)
                },
                true,
            ),
            running,
        }
    }
}

impl Goal for CatTemptGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        // Vanilla `CatTemptGoal.canUse`: tamed cats are never tempted.
        !mob.as_tameable().is_some_and(Tameable::is_tame) && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
        self.running.store(true, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
        self.running.store(false, Ordering::Relaxed);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `Cat.CatAvoidEntityGoal`: untamed cats only.
pub(super) struct CatAvoidEntityGoal {
    inner: AvoidEntityGoal,
}

impl CatAvoidEntityGoal {
    pub(super) fn new(max_dist: f32, walk_speed_modifier: f64, sprint_speed_modifier: f64) -> Self {
        Self {
            inner: AvoidEntityGoal::new(max_dist, walk_speed_modifier, sprint_speed_modifier),
        }
    }
}

impl Goal for CatAvoidEntityGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        // Vanilla removes this goal once tamed; keeping it registered and vetoing here is
        // equivalent because the goal is only ever entered through `canUse`.
        !mob.as_tameable().is_some_and(Tameable::is_tame) && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.as_tameable().is_some_and(Tameable::is_tame) && self.inner.can_continue_to_use(mob)
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

/// Vanilla `Cat.CatRelaxOnOwnerGoal`: a tamed, awake cat sleeps on its sleeping owner's bed.
pub(super) struct CatRelaxOnOwnerGoal {
    owner: Option<SharedEntity>,
    goal_pos: Option<BlockPos>,
    on_bed_ticks: i32,
}

impl CatRelaxOnOwnerGoal {
    pub(super) const fn new() -> Self {
        Self {
            owner: None,
            goal_pos: None,
            on_bed_ticks: 0,
        }
    }

    /// Vanilla `spaceIsOccupied`: another cat is already lying or relaxing there.
    fn space_is_occupied(mob: &dyn PathfinderMob, goal_pos: BlockPos) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        let center = DVec3::new(
            f64::from(goal_pos.x()) + 0.5,
            f64::from(goal_pos.y()) + 0.5,
            f64::from(goal_pos.z()) + 0.5,
        );
        let search_box =
            steel_utils::WorldAabb::of_size(center, 1.0, 1.0, 1.0).inflate_xyz(2.0, 2.0, 2.0);
        let self_id = mob.as_entity_event_source().id();

        !world
            .get_entities_in_aabb_matching(&search_box, |entity| {
                entity.id() != self_id
                    && entity
                        .downcast_ref::<super::CatEntity>()
                        .is_some_and(|cat| cat.is_lying() || cat.is_relax_state_one())
            })
            .is_empty()
    }

    /// Vanilla `CatRelaxOnOwnerGoal.canUse` bed resolution: the block the owner sleeps in,
    /// stepped back to the bed's foot when it has a facing.
    fn bed_goal_pos(world: &World, owner_pos: BlockPos) -> Option<BlockPos> {
        let state = world.get_block_state(owner_pos);
        if !state.get_block().has_tag(&BlockTag::CATS_CAN_LIE_ON) {
            return None;
        }

        let bed_dir = state.try_get_value(&BlockStateProperties::FACING);
        Some(match bed_dir {
            Some(direction) => owner_pos.relative(direction.opposite()),
            None => owner_pos,
        })
    }
}

impl Goal for CatRelaxOnOwnerGoal {
    fn controls(&self) -> GoalControls {
        // Vanilla `CatRelaxOnOwnerGoal` sets no flags.
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(tameable) = mob.as_tameable() else {
            return false;
        };
        if !tameable.is_tame() || tameable.is_ordered_to_sit() {
            return false;
        }

        let Some(owner) = tameable.owner() else {
            return false;
        };
        let Some(owner_player) = owner.as_player() else {
            return false;
        };
        if !owner_player.is_sleeping() {
            return false;
        }
        if mob.position().distance_squared(owner.position()) > OWNER_SEARCH_RANGE_SQR {
            return false;
        }

        let Some(world) = mob.level() else {
            return false;
        };
        let Some(goal_pos) = Self::bed_goal_pos(world.as_ref(), owner.block_position()) else {
            return false;
        };
        if Self::space_is_occupied(mob, goal_pos) {
            return false;
        }

        self.owner = Some(owner);
        self.goal_pos = Some(goal_pos);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(tameable) = mob.as_tameable() else {
            return false;
        };
        if !tameable.is_tame() || tameable.is_ordered_to_sit() {
            return false;
        }

        let Some(owner) = &self.owner else {
            return false;
        };
        let Some(owner_player) = owner.as_player() else {
            return false;
        };
        if !owner_player.is_sleeping() {
            return false;
        }

        let Some(goal_pos) = self.goal_pos else {
            return false;
        };
        !Self::space_is_occupied(mob, goal_pos)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(goal_pos) = self.goal_pos {
            if let Some(tameable) = mob.as_tameable() {
                tameable.set_in_sitting_pose(false);
            }
            mob.move_to_pos(
                DVec3::new(
                    f64::from(goal_pos.x()),
                    f64::from(goal_pos.y()),
                    f64::from(goal_pos.z()),
                ),
                BED_WALK_SPEED,
            );
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(tameable) = mob.as_tameable() {
            if let Some(cat) = mob
                .as_entity_event_source()
                .downcast_ref::<super::CatEntity>()
            {
                cat.set_lying(false);
            }
            // GAP: `CatRelaxOnOwnerGoal.giveMorningGift` needs the
            // `CAT_WAKING_UP_GIFT_CHANCE` environment attribute and the
            // `cat_morning_gift` loot table, neither of which Steel models yet. The
            // owner-sleep gate vanilla checks first is kept for the future gift path.
            if let Some(owner) = &self.owner
                && let Some(owner_player) = owner.as_player()
                && owner_player.sleep_counter() >= GIFT_MIN_SLEEP_TICKS
            {
                log::trace!("cat morning gift skipped: environment attributes are not modelled");
            }

            self.on_bed_ticks = 0;
            if let Some(cat) = mob
                .as_entity_event_source()
                .downcast_ref::<super::CatEntity>()
            {
                cat.set_relax_state_one(false);
            }
            tameable.set_in_sitting_pose(false);
        }

        mob.mob_base().navigation().lock().stop();
        self.owner = None;
        self.goal_pos = None;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let (Some(owner), Some(goal_pos)) = (&self.owner, self.goal_pos) else {
            return;
        };

        let Some(cat) = mob
            .as_entity_event_source()
            .downcast_ref::<super::CatEntity>()
        else {
            return;
        };
        let Some(tameable) = mob.as_tameable() else {
            return;
        };

        tameable.set_in_sitting_pose(false);
        mob.move_to_pos(
            DVec3::new(
                f64::from(goal_pos.x()),
                f64::from(goal_pos.y()),
                f64::from(goal_pos.z()),
            ),
            BED_WALK_SPEED,
        );

        if mob.position().distance_squared(owner.position()) < OWNER_CLOSE_RANGE_SQR {
            self.on_bed_ticks += 1;
            if self.on_bed_ticks > LYING_TICKS {
                cat.set_lying(true);
                cat.set_relax_state_one(false);
            } else {
                let owner_pos = owner.position();
                mob.mob_base().controls().lock().look_control.set_look_at(
                    DVec3::new(
                        owner_pos.x,
                        owner_pos.y + f64::from(owner.get_eye_y()),
                        owner_pos.z,
                    ),
                    OWNER_LOOK_SPEED,
                    OWNER_LOOK_ANGLE,
                );
                cat.set_relax_state_one(true);
            }
        } else {
            cat.set_lying(false);
        }
    }
}
