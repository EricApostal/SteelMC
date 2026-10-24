//! Vanilla `LongJumpToRandomPos`: prepare, then perform, a long jump to a random spot.

use std::any::Any;
use std::mem::take;
use std::sync::Arc;

use glam::DVec3;
use steel_utils::BlockPos;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::value_providers::UniformIntProvider;

use super::long_jump_util;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::{BlockPosTracker, PositionTracker};
use crate::entity::ai::path::PathfindingContext;
use crate::entity::{LivingEntity, SharedEntity};

/// Vanilla `LongJumpToRandomPos.FIND_JUMP_TRIES`.
pub const FIND_JUMP_TRIES: i32 = 20;
/// Vanilla `LongJumpToRandomPos.MIN_PATHFIND_DISTANCE_TO_VALID_JUMP`.
pub const MIN_PATHFIND_DISTANCE_TO_VALID_JUMP: i32 = 8;
/// Vanilla `LongJumpToRandomPos.TIME_OUT_DURATION`.
const TIME_OUT_DURATION: i32 = 200;
/// Vanilla `LongJumpToRandomPos.PREPARE_JUMP_DURATION`.
const PREPARE_JUMP_DURATION: i64 = 40;
/// Vanilla `LongJumpToRandomPos.ALLOWED_ANGLES`.
const ALLOWED_ANGLES: [i32; 4] = [65, 70, 75, 80];

/// Vanilla `LongJumpToRandomPos.PossibleJump`.
#[derive(Clone)]
pub struct PossibleJump {
    target_pos: BlockPos,
    weight: i32,
}

impl PossibleJump {
    #[must_use]
    pub const fn target_pos(&self) -> BlockPos {
        self.target_pos
    }

    #[must_use]
    pub const fn weight(&self) -> i32 {
        self.weight
    }
}

/// Vanilla `LongJumpToRandomPos.getJumpCandidate` override point.
///
/// Vanilla subclasses (`LongJumpToPreferredBlock`) override this single method;
/// Steel expresses the same extension seam as a trait over the candidate pool.
pub trait JumpCandidatePicker: Any + Send + Sync {
    /// Picks (and removes) the next candidate from `pool`, or `None` when the
    /// pool holds no acceptable candidate.
    fn pick(
        &mut self,
        ctx: &BehaviorContext<'_>,
        pool: &mut Vec<PossibleJump>,
    ) -> Option<PossibleJump>;
}

/// Vanilla `LongJumpToRandomPos.getJumpCandidate`: weighted random pick.
pub struct WeightedPicker;

impl JumpCandidatePicker for WeightedPicker {
    fn pick(
        &mut self,
        _ctx: &BehaviorContext<'_>,
        pool: &mut Vec<PossibleJump>,
    ) -> Option<PossibleJump> {
        let total_weight: i32 = pool.iter().map(PossibleJump::weight).sum();
        if total_weight <= 0 {
            return None;
        }

        // Vanilla `WeightedRandom.getRandomItem(random, jumpCandidates, PossibleJump::weight)`.
        let mut roll = rand::random_range(0..total_weight);
        let mut index = pool.len() - 1;
        for (candidate_index, jump) in pool.iter().enumerate() {
            roll -= jump.weight();
            if roll < 0 {
                index = candidate_index;
                break;
            }
        }
        Some(pool.swap_remove(index))
    }
}

/// Vanilla `LongJumpToRandomPos<E extends Mob>`.
pub struct LongJumpToRandomPos {
    state: BehaviorState,
    time_between_long_jumps: UniformIntProvider,
    max_long_jump_height: i32,
    max_long_jump_width: i32,
    max_jump_velocity_multiplier: f32,
    picker: Box<dyn JumpCandidatePicker>,
    acceptable_landing_spot: fn(&SharedEntity, BlockPos) -> bool,
    jump_candidates: Vec<PossibleJump>,
    initial_position: Option<DVec3>,
    chosen_jump: Option<DVec3>,
    find_jump_tries: i32,
    prepare_jump_start: i64,
}

impl LongJumpToRandomPos {
    /// Vanilla `LongJumpToRandomPos(timeBetweenLongJumps, maxLongJumpHeight,
    /// maxLongJumpWidth, maxJumpVelocity, getJumpSound)`.
    ///
    /// Steel carries no sound handle on the behavior layer, so `getJumpSound` is
    /// not taken here; the jump-vector preparation half is complete.
    #[must_use]
    pub fn new(
        time_between_long_jumps: UniformIntProvider,
        max_long_jump_height: i32,
        max_long_jump_width: i32,
        max_jump_velocity_multiplier: f32,
    ) -> Self {
        Self::with_picker(
            time_between_long_jumps,
            max_long_jump_height,
            max_long_jump_width,
            max_jump_velocity_multiplier,
            Box::new(WeightedPicker),
        )
    }

    /// Vanilla's subclass constructor path: `super(...)` plus a `getJumpCandidate`
    /// override.
    #[must_use]
    pub fn with_picker(
        time_between_long_jumps: UniformIntProvider,
        max_long_jump_height: i32,
        max_long_jump_width: i32,
        max_jump_velocity_multiplier: f32,
        picker: Box<dyn JumpCandidatePicker>,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::WALK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::LOOK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::LONG_JUMP_COOLDOWN_TICKS as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                ],
                TIME_OUT_DURATION,
                TIME_OUT_DURATION,
            ),
            time_between_long_jumps,
            max_long_jump_height,
            max_long_jump_width,
            max_jump_velocity_multiplier,
            picker,
            acceptable_landing_spot: Self::default_acceptable_landing_spot,
            jump_candidates: Vec::new(),
            initial_position: None,
            chosen_jump: None,
            find_jump_tries: 0,
            prepare_jump_start: 0,
        }
    }

    /// Vanilla's subclass constructor path: `super(...)` plus a
    /// `getJumpCandidate`/`acceptableLandingSpot` override.
    #[must_use]
    pub fn with_acceptable_landing_spot(
        mut self,
        acceptable_landing_spot: fn(&SharedEntity, BlockPos) -> bool,
    ) -> Self {
        self.acceptable_landing_spot = acceptable_landing_spot;
        self
    }

    /// Mutable access to the candidate picker for subclass reset hooks.
    pub fn picker_as_mut<T: JumpCandidatePicker + 'static>(&mut self) -> Option<&mut T> {
        (self.picker.as_mut() as &mut dyn Any).downcast_mut::<T>()
    }

    /// Vanilla's `acceptableLandingSpot` default predicate.
    #[must_use]
    pub fn default_acceptable_landing_spot(body: &SharedEntity, pos: BlockPos) -> bool {
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;

        use crate::world::LevelReader as _;

        let Some(world) = body.level() else {
            return false;
        };
        let Some(mob) = body.as_pathfinder_mob() else {
            return false;
        };
        if !world.get_block_state(pos.below()).is_solid_render() {
            return false;
        }

        // Vanilla `WalkNodeEvaluator.getPathTypeStatic(body, targetPos)`.
        let mut context = PathfindingContext::new(world.as_ref(), body.block_position());
        let path_type = context.get_path_type_from_state(pos.x(), pos.y(), pos.z());
        mob.get_pathfinding_malus(path_type) == 0.0
    }

    fn sample_time_between_long_jumps(&self) -> i32 {
        let mut random = LegacyRandom::from_seed(rand::random());
        self.time_between_long_jumps.sample(&mut random)
    }

    /// Vanilla `isAcceptableLandingPosition`.
    fn is_acceptable_landing_position(&self, ctx: &BehaviorContext<'_>, pos: BlockPos) -> bool {
        let body_pos = ctx.body.block_position();
        if body_pos.x() == pos.x() && body_pos.z() == pos.z() {
            return false;
        }
        (self.acceptable_landing_spot)(ctx.body, pos)
    }

    /// Vanilla `calculateOptimalJumpVector`.
    fn calculate_optimal_jump_vector(
        &self,
        body: &SharedEntity,
        target_pos: DVec3,
    ) -> Option<DVec3> {
        let mut angles = ALLOWED_ANGLES;
        // Vanilla `Collections.shuffle(allowedAngles)`.
        for i in (1..angles.len()).rev() {
            let j = rand::random_range(0..=i);
            angles.swap(i, j);
        }

        let max_jump_velocity = jump_strength(body) * self.max_jump_velocity_multiplier;
        angles.into_iter().find_map(|angle| {
            long_jump_util::calculate_jump_vector_for_angle(
                body,
                target_pos,
                max_jump_velocity,
                angle,
                true,
            )
        })
    }

    /// Vanilla `pickCandidate`.
    fn pick_candidate(&mut self, ctx: &mut BehaviorContext<'_>) {
        let mut pool = take(&mut self.jump_candidates);
        let mut picked = None;
        while !pool.is_empty() {
            let Some(jump) = self.picker.pick(ctx, &mut pool) else {
                break;
            };
            let target_pos = jump.target_pos();
            if !self.is_acceptable_landing_position(ctx, target_pos) {
                continue;
            }

            let (center_x, center_y, center_z) = target_pos.get_center();
            let jump_vector = self
                .calculate_optimal_jump_vector(ctx.body, DVec3::new(center_x, center_y, center_z));
            let Some(jump_vector) = jump_vector else {
                continue;
            };

            ctx.set_memory(
                &mem::LOOK_TARGET,
                Arc::new(BlockPosTracker::from_block_pos(target_pos)) as Arc<dyn PositionTracker>,
            );

            // Vanilla rejects spots already reachable by pathfinding: the jump is
            // only committed when `createPath(target, 0, 8)` fails to reach.
            let path_reaches = ctx.body.as_pathfinder_mob().is_some_and(|mob| {
                mob.create_path_to(target_pos, 0)
                    .is_some_and(|path| path.can_reach())
            });
            if !path_reaches {
                picked = Some(jump_vector);
                break;
            }
        }
        self.jump_candidates = pool;
        if let Some(jump_vector) = picked {
            self.chosen_jump = Some(jump_vector);
            self.prepare_jump_start = ctx.timestamp;
        }
    }

    fn cool_down(&self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory(
            &mem::LONG_JUMP_COOLDOWN_TICKS,
            self.sample_time_between_long_jumps() / 2,
        );
    }
}

/// Vanilla `Attributes.JUMP_STRENGTH` value for the body.
fn jump_strength(body: &SharedEntity) -> f32 {
    use steel_registry::vanilla_attributes;
    body.as_living_entity().map_or(0.42, |living| {
        living
            .attributes()
            .lock()
            .required_value(vanilla_attributes::JUMP_STRENGTH) as f32
    })
}

impl Behavior for LongJumpToRandomPos {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let can_start = ctx.body.on_ground() && !ctx.body.is_in_water() && !ctx.body.is_in_lava();
        if !can_start {
            self.cool_down(ctx);
        }
        can_start
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let is_valid = self.initial_position.is_some_and(|initial| {
            initial == ctx.body.position()
                && self.find_jump_tries > 0
                && !ctx.body.is_in_water()
                && (self.chosen_jump.is_some() || !self.jump_candidates.is_empty())
        });
        if !is_valid && !ctx.has_memory_value(&mem::LONG_JUMP_MID_JUMP) {
            self.cool_down(ctx);
            ctx.erase_memory(&mem::LOOK_TARGET);
        }
        is_valid
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.chosen_jump = None;
        self.find_jump_tries = FIND_JUMP_TRIES;
        self.initial_position = Some(ctx.body.position());

        let mob_pos = ctx.body.block_position();
        self.jump_candidates.clear();
        for x in mob_pos.x() - self.max_long_jump_width..=mob_pos.x() + self.max_long_jump_width {
            for y in
                mob_pos.y() - self.max_long_jump_height..=mob_pos.y() + self.max_long_jump_height
            {
                for z in
                    mob_pos.z() - self.max_long_jump_width..=mob_pos.z() + self.max_long_jump_width
                {
                    let pos = BlockPos::new(x, y, z);
                    if pos == mob_pos {
                        continue;
                    }
                    let dx = mob_pos.x() - x;
                    let dy = mob_pos.y() - y;
                    let dz = mob_pos.z() - z;
                    // Vanilla `Mth.ceil(mobPos.distSqr(pos))`.
                    self.jump_candidates.push(PossibleJump {
                        target_pos: pos,
                        weight: dx * dx + dy * dy + dz * dz,
                    });
                }
            }
        }
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(chosen_jump) = self.chosen_jump {
            if ctx.timestamp - self.prepare_jump_start >= PREPARE_JUMP_DURATION {
                // Vanilla `body.setYRot(body.yBodyRot)`; pitch is untouched.
                let (_, pitch) = ctx.body.rotation();
                let yaw = ctx
                    .body
                    .as_living_entity()
                    .map_or(0.0, LivingEntity::y_body_rot);
                ctx.body.set_rotation((yaw, pitch));

                let jump_boost = if let Some(living) = ctx.body.as_living_entity() {
                    living.set_discard_friction(true);
                    f64::from(living.get_jump_boost_power())
                } else {
                    0.0
                };

                let org_length = chosen_jump.length();
                if org_length > f64::EPSILON {
                    ctx.body
                        .set_velocity(chosen_jump * ((org_length + jump_boost) / org_length));
                }
                ctx.set_memory(&mem::LONG_JUMP_MID_JUMP, true);
            }
        } else {
            self.find_jump_tries -= 1;
            self.pick_candidate(ctx);
        }
    }
}
