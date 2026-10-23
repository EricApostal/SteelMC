//! Vanilla `JumpOnBed`.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::BlockPos;

use super::utils;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};
use crate::entity::ai::brain::tracker::BlockPosTracker;
use crate::entity::{LivingEntity, SharedEntity};

/// Vanilla `JumpOnBed.MAX_TIME_TO_REACH_BED`.
const MAX_TIME_TO_REACH_BED: i32 = 100;
/// Vanilla `JumpOnBed.MIN_JUMPS`.
const MIN_JUMPS: i32 = 3;
/// Vanilla `JumpOnBed.MAX_JUMPS` (exclusive roll bound of `3 + nextInt(4)`).
const MAX_JUMPS: i32 = 6;
/// Vanilla `JumpOnBed.COOLDOWN_BETWEEN_JUMPS`.
const COOLDOWN_BETWEEN_JUMPS: i32 = 5;

/// Vanilla `JumpOnBed`: a baby bounces on a nearby bed.
///
/// Steel's `Behavior` has no `timedOut` override, so the run is bounded at the
/// 100-tick walk budget instead of vanilla's "never times out".
pub struct JumpOnBed {
    state: BehaviorState,
    speed_modifier: f32,
    target_bed: Option<BlockPos>,
    remaining_time_to_reach_bed: i32,
    remaining_jumps: i32,
    remaining_cooldown_until_next_jump: i32,
}

impl JumpOnBed {
    /// Vanilla `JumpOnBed(speedModifier)`.
    #[must_use]
    pub fn new(speed_modifier: f32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::NEAREST_BED as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                    (
                        &mem::WALK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                ],
                MAX_TIME_TO_REACH_BED,
                MAX_TIME_TO_REACH_BED,
            ),
            speed_modifier,
            target_bed: None,
            remaining_time_to_reach_bed: 0,
            remaining_jumps: 0,
            remaining_cooldown_until_next_jump: 0,
        }
    }

    /// Vanilla `JumpOnBed.nearBed`.
    fn near_bed(ctx: &BehaviorContext<'_>, body: &SharedEntity) -> bool {
        Self::on_or_over_bed(ctx, body) || ctx.has_memory_value(&mem::NEAREST_BED)
    }

    /// Vanilla `JumpOnBed.onOrOverBed`.
    fn on_or_over_bed(ctx: &BehaviorContext<'_>, body: &SharedEntity) -> bool {
        let pos = body.block_position();
        Self::is_jumpable(ctx, pos) || Self::is_jumpable(ctx, pos.below())
    }

    /// Vanilla `JumpOnBed.onBedSurface`.
    fn on_bed_surface(ctx: &BehaviorContext<'_>, body: &SharedEntity) -> bool {
        Self::is_jumpable(ctx, body.block_position())
    }

    /// Vanilla `JumpOnBed.isJumpable`.
    fn is_jumpable(ctx: &BehaviorContext<'_>, pos: BlockPos) -> bool {
        ctx.level
            .get_block_state(pos)
            .get_block()
            .has_tag(&BlockTag::VILLAGER_BABIES_CAN_JUMP_ON_BED)
    }

    /// Vanilla `JumpOnBed.tiredOfWalking`.
    fn tired_of_walking(&self, ctx: &BehaviorContext<'_>) -> bool {
        !Self::on_or_over_bed(ctx, ctx.body) && self.remaining_time_to_reach_bed <= 0
    }

    /// Vanilla `JumpOnBed.tiredOfJumping`.
    fn tired_of_jumping(&self, ctx: &BehaviorContext<'_>) -> bool {
        Self::on_or_over_bed(ctx, ctx.body) && self.remaining_jumps <= 0
    }
}

impl Behavior for JumpOnBed {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body
            .as_living_entity()
            .is_some_and(LivingEntity::is_baby)
            && Self::near_bed(ctx, ctx.body)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(bed_pos) = ctx.get_memory(&mem::NEAREST_BED) else {
            return;
        };

        self.target_bed = Some(bed_pos);
        self.remaining_time_to_reach_bed = MAX_TIME_TO_REACH_BED;
        self.remaining_jumps = MIN_JUMPS + rand::random_range(0..(MAX_JUMPS - MIN_JUMPS));
        self.remaining_cooldown_until_next_jump = 0;

        utils::set_walk_target(
            ctx,
            Arc::new(BlockPosTracker::from_block_pos(bed_pos)),
            self.speed_modifier,
            0,
        );
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body
            .as_living_entity()
            .is_some_and(LivingEntity::is_baby)
            && self
                .target_bed
                .is_some_and(|bed| Self::is_jumpable(ctx, bed))
            && !self.tired_of_walking(ctx)
            && !self.tired_of_jumping(ctx)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if !Self::on_or_over_bed(ctx, ctx.body) {
            self.remaining_time_to_reach_bed -= 1;
        } else if self.remaining_cooldown_until_next_jump > 0 {
            self.remaining_cooldown_until_next_jump -= 1;
        } else if Self::on_bed_surface(ctx, ctx.body) {
            if let Some(mob) = ctx.body.as_mob() {
                mob.mob_base().controls().lock().jump_control.jump();
            }
            self.remaining_jumps -= 1;
            self.remaining_cooldown_until_next_jump = COOLDOWN_BETWEEN_JUMPS;
        }
    }

    fn stop(&mut self, _ctx: &mut BehaviorContext<'_>) {
        self.target_bed = None;
        self.remaining_time_to_reach_bed = 0;
        self.remaining_jumps = 0;
        self.remaining_cooldown_until_next_jump = 0;
    }
}
