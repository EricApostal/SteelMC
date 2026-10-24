//! Vanilla `RandomStroll`: pick a random nearby position as the walk target.

use std::f64::consts::FRAC_PI_2;
use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;

use super::random_pos;
use crate::behavior::BlockStateBehaviorExt as _;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{BlockPosTracker, WalkTarget};
use crate::entity::ai::path::PathComputationType;
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `RandomStroll.MAX_XZ_DIST`.
const MAX_XZ_DIST: i32 = 10;
/// Vanilla `RandomStroll.MAX_Y_DIST`.
const MAX_Y_DIST: i32 = 7;

/// Vanilla `RandomStroll.SWIM_XY_DISTANCE_TIERS`.
const SWIM_XY_DISTANCE_TIERS: [(i32, i32); 6] = [(1, 1), (3, 3), (5, 5), (6, 5), (7, 7), (10, 7)];

/// Vanilla `RandomStroll` walk-target fetch mode.
#[derive(Clone, Copy)]
enum StrollMode {
    /// Vanilla `LandRandomPos.getPos(body, maxHorizontalDistance, maxVerticalDistance)`.
    Land {
        max_horizontal_distance: i32,
        max_vertical_distance: i32,
    },
    /// Vanilla `RandomStroll.getTargetFlyPos`.
    Fly,
    /// Vanilla `RandomStroll.getTargetSwimPos`.
    Swim,
}

/// Vanilla `RandomStroll`.
pub struct RandomStroll {
    speed_modifier: f32,
    may_stroll_from_water: bool,
    mode: StrollMode,
}

impl RandomStroll {
    /// Vanilla `RandomStroll.stroll(speedModifier)`: stroll anywhere, including water.
    #[must_use]
    pub fn stroll(speed_modifier: f32) -> OneShot<Self> {
        Self::stroll_with_restriction(speed_modifier, true)
    }

    /// Vanilla `RandomStroll.stroll(speedModifier, mayStrollFromWater)`.
    #[must_use]
    pub fn stroll_with_restriction(
        speed_modifier: f32,
        may_stroll_from_water: bool,
    ) -> OneShot<Self> {
        Self::for_mode(
            speed_modifier,
            may_stroll_from_water,
            StrollMode::Land {
                max_horizontal_distance: MAX_XZ_DIST,
                max_vertical_distance: MAX_Y_DIST,
            },
        )
    }

    /// Vanilla `RandomStroll.stroll(speedModifier, maxHorizontalDistance, maxVerticalDistance)`.
    #[must_use]
    pub fn stroll_with_distances(
        speed_modifier: f32,
        max_horizontal_distance: i32,
        max_vertical_distance: i32,
    ) -> OneShot<Self> {
        Self::for_mode(
            speed_modifier,
            true,
            StrollMode::Land {
                max_horizontal_distance,
                max_vertical_distance,
            },
        )
    }

    /// Vanilla `RandomStroll.fly(speedModifier)`.
    #[must_use]
    pub fn fly(speed_modifier: f32) -> OneShot<Self> {
        Self::for_mode(speed_modifier, true, StrollMode::Fly)
    }

    /// Vanilla `RandomStroll.swim(speedModifier)`.
    #[must_use]
    pub fn swim(speed_modifier: f32) -> OneShot<Self> {
        Self::for_mode(speed_modifier, true, StrollMode::Swim)
    }

    fn for_mode(
        speed_modifier: f32,
        may_stroll_from_water: bool,
        mode: StrollMode,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            speed_modifier,
            may_stroll_from_water,
            mode,
        })
        .with_required_memories(vec![&mem::WALK_TARGET])
    }

    /// Vanilla `strollFlyOrSwim` target fetch plus the `canRun` gate.
    fn target_pos(&self, _ctx: &BehaviorContext<'_>, body: &SharedEntity) -> Option<DVec3> {
        if !self.may_stroll_from_water && body.is_in_water() {
            // Vanilla gates non-water strolls on `b -> !b.isInWater()`.
            return None;
        }
        match self.mode {
            StrollMode::Land {
                max_horizontal_distance,
                max_vertical_distance,
            } => {
                let mob = body.as_pathfinder_mob()?;
                random_pos::land_random_pos(mob, max_horizontal_distance, max_vertical_distance)
            }
            StrollMode::Fly => {
                let mob = body.as_pathfinder_mob()?;
                let body_pos = body.position();
                let view = body.look_angle();
                random_pos::default_random_pos_towards(
                    mob,
                    MAX_XZ_DIST,
                    MAX_Y_DIST,
                    body_pos + DVec3::new(view.x, 0.0, view.z) * 10.0,
                    FRAC_PI_2,
                )
                // Vanilla `AirAndWaterRandomPos.getPos(..., -2, x, z, PI/2)`:
                // `-2` is an extra downward offset for the landing-height search
                // that the Steel reimplementation folds into air candidates.
            }
            StrollMode::Swim => self.target_swim_pos(body),
        }
    }

    /// Vanilla `RandomStroll.getTargetSwimPos`.
    ///
    /// Vanilla calls `BehaviorUtils.getRandomSwimmablePos(body, tierXz, tierY)`
    /// for the first tier and then probes along the previous fallback direction
    /// for the remaining tiers; Steel ports both halves.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `RandomStroll.getTargetSwimPos`"
    )]
    fn target_swim_pos(&self, body: &SharedEntity) -> Option<DVec3> {
        let mob = body.as_pathfinder_mob()?;
        let body_pos = body.position();
        let mut fallback: Option<DVec3> = None;
        let mut target_pos: Option<DVec3> = None;

        for (horizontal_dist, vertical_dist) in SWIM_XY_DISTANCE_TIERS {
            target_pos = match fallback {
                None => random_swimmable_pos(mob, horizontal_dist, vertical_dist),
                Some(fallback_pos) => {
                    let direction = (fallback_pos - body_pos).normalize_or_zero()
                        * DVec3::new(
                            f64::from(horizontal_dist),
                            f64::from(vertical_dist),
                            f64::from(horizontal_dist),
                        );
                    Some(body_pos + direction)
                }
            };

            let Some(candidate) = target_pos else {
                return fallback;
            };
            // Vanilla rejects tiers whose position has no fluid or is restricted;
            // Steel's `BlockPos` tracker has no restrict surface, so the fluid
            // half is checked through the block-state's fluid state.
            if !is_restricted_or_dry(body, candidate) {
                return fallback;
            }

            fallback = Some(candidate);
        }

        target_pos
    }
}

/// Vanilla `BehaviorUtils.getRandomSwimmablePos`: retry a random position until
/// its block state is water-pathfindable, at most `RANDOM_SWIMMABLE_POS_RETRIES`.
fn random_swimmable_pos(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
) -> Option<DVec3> {
    const RETRIES: i32 = 10;

    // Steel's behavior-local reimplementation has no `generateRandomPos`
    // toward-direction half, so it retries `land_random_pos` for a
    // pathfindable water target instead.
    let mut target_pos = random_pos::land_random_pos(mob, horizontal_dist, vertical_dist);
    let mut count = 0;
    while target_pos.is_some_and(|pos| {
        !mob.level().is_some_and(|world| {
            world
                .get_block_state(steel_utils::BlockPos::containing(pos.x, pos.y, pos.z))
                .is_pathfindable(PathComputationType::Water)
        })
    }) && count < RETRIES
    {
        count += 1;
        target_pos = random_pos::land_random_pos(mob, horizontal_dist, vertical_dist);
    }

    target_pos
}

/// Vanilla's `pos == null || fluidState.isEmpty() || GoalUtils.isRestricted(...)`
/// early return in `getTargetSwimPos`, minus the restrict half Steel cannot test.
fn is_restricted_or_dry(body: &SharedEntity, pos: DVec3) -> bool {
    let Some(world) = body.level() else {
        return true;
    };
    world
        .get_block_state(steel_utils::BlockPos::containing(pos.x, pos.y, pos.z))
        .get_fluid_state()
        .is_empty()
}

impl Trigger for RandomStroll {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.check_memory(&mem::WALK_TARGET, MemoryStatus::ValuePresent) {
            return false;
        }

        let target = self.target_pos(ctx, ctx.body);
        ctx.set_memory_or_erase(
            &mem::WALK_TARGET,
            target.map(|pos| {
                WalkTarget::new(
                    Arc::new(BlockPosTracker::from_vec(pos)),
                    self.speed_modifier,
                    0,
                )
            }),
        );
        true
    }
}
