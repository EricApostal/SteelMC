//! Vanilla `LongJumpToPreferredBlock`: long-jump toward a preferred block when the
//! roll asks for it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::registry::TaggedRegistryExt as _;
use steel_utils::BlockPos;
use steel_utils::Identifier;
use steel_utils::value_providers::UniformIntProvider;

use super::long_jump_to_random_pos::{
    JumpCandidatePicker, LongJumpToRandomPos, PossibleJump, WeightedPicker,
};
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};

/// Vanilla `LongJumpToPreferredBlock.getJumpCandidate` override state.
struct PreferredBlockPicker {
    weighted: WeightedPicker,
    preferred_block_tag: &'static Identifier,
    wanting_preferred_ones: Arc<AtomicBool>,
    not_preferred_jump_candidates: Vec<PossibleJump>,
}

impl JumpCandidatePicker for PreferredBlockPicker {
    fn pick(
        &mut self,
        ctx: &BehaviorContext<'_>,
        pool: &mut Vec<PossibleJump>,
    ) -> Option<PossibleJump> {
        if !self.wanting_preferred_ones.load(Ordering::Relaxed) {
            return self.weighted.pick(ctx, pool);
        }

        while !pool.is_empty() {
            let Some(jump_candidate) = self.weighted.pick(ctx, pool) else {
                break;
            };
            if block_is_in_tag(
                ctx,
                jump_candidate.target_pos().below(),
                self.preferred_block_tag,
            ) {
                return Some(jump_candidate);
            }

            self.not_preferred_jump_candidates.push(jump_candidate);
        }

        if self.not_preferred_jump_candidates.is_empty() {
            None
        } else {
            Some(self.not_preferred_jump_candidates.remove(0))
        }
    }
}

/// Vanilla `LongJumpToPreferredBlock<E extends Mob>`.
pub struct LongJumpToPreferredBlock {
    random_pos: LongJumpToRandomPos,
    preferred_blocks_chance: f32,
    wanting_preferred_ones: Arc<AtomicBool>,
}

impl LongJumpToPreferredBlock {
    /// Vanilla `LongJumpToPreferredBlock(timeBetweenLongJumps, maxLongJumpHeight,
    /// maxLongJumpWidth, maxJumpVelocity, getJumpSound, preferredBlockTag,
    /// preferredBlocksChance, acceptableLandingSpot)`.
    #[must_use]
    pub fn new(
        time_between_long_jumps: UniformIntProvider,
        max_long_jump_height: i32,
        max_long_jump_width: i32,
        max_jump_velocity_multiplier: f32,
        preferred_block_tag: &'static Identifier,
        preferred_blocks_chance: f32,
    ) -> Self {
        let wanting_preferred_ones = Arc::new(AtomicBool::new(false));
        Self {
            random_pos: LongJumpToRandomPos::with_picker(
                time_between_long_jumps,
                max_long_jump_height,
                max_long_jump_width,
                max_jump_velocity_multiplier,
                Box::new(PreferredBlockPicker {
                    weighted: WeightedPicker,
                    preferred_block_tag,
                    wanting_preferred_ones: Arc::clone(&wanting_preferred_ones),
                    not_preferred_jump_candidates: Vec::new(),
                }),
            ),
            preferred_blocks_chance,
            wanting_preferred_ones,
        }
    }

    /// Vanilla `LongJumpToPreferredBlock`'s `acceptableLandingSpot` constructor
    /// argument, forwarded to the wrapped `LongJumpToRandomPos`.
    #[must_use]
    pub fn with_acceptable_landing_spot(
        mut self,
        acceptable_landing_spot: fn(&SharedEntity, BlockPos) -> bool,
    ) -> Self {
        self.random_pos = self
            .random_pos
            .with_acceptable_landing_spot(acceptable_landing_spot);
        self
    }
}

/// Vanilla `level.getBlockState(pos).is(TagKey<Block>)`.
fn block_is_in_tag(ctx: &BehaviorContext<'_>, pos: BlockPos, tag: &Identifier) -> bool {
    steel_registry::REGISTRY
        .blocks
        .is_in_tag(ctx.level.get_block_state(pos).get_block(), tag)
}

impl Behavior for LongJumpToPreferredBlock {
    fn state(&self) -> &BehaviorState {
        self.random_pos.state()
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        self.random_pos.state_mut()
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.random_pos.check_extra_start_conditions(ctx)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.random_pos.can_still_use(ctx)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.random_pos.start(ctx);
        // Vanilla `super.start` then `currentlyWantingPreferredOnes = random < chance`.
        self.wanting_preferred_ones.store(
            rand::random::<f32>() < self.preferred_blocks_chance,
            Ordering::Relaxed,
        );
        if let Some(picker) = self.random_pos.picker_as_mut::<PreferredBlockPicker>() {
            picker.not_preferred_jump_candidates.clear();
        }
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.random_pos.tick(ctx);
    }
}
