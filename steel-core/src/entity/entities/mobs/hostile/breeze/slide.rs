//! Vanilla `monster/breeze/Slide`.
//!
//! Vanilla picks a slide target away from the attack target and walks there at
//! `SPEED_MULTIPLIER_WHEN_SLIDING`; the pose/sound swap lives on the entity
//! (`BreezeAi.SlideToTargetSink` handles the fight-activity sink variant).

use glam::DVec3;
use steel_registry::entity_data::EntityPose;
use steel_utils::BlockPos;

use super::brain::{SPEED_MULTIPLIER_WHEN_SLIDING, slide_target_position};
use crate::entity::SharedEntity;
use crate::entity::ai::behavior::random_pos::land_random_pos;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::WalkTarget;

/// Vanilla `Slide.start`'s `DefaultRandomPos.getPosAway(breeze, 5, 5, enemy.position())`.
const AWAY_HORIZONTAL_DIST: i32 = 5;
/// Vanilla `Slide.start`'s vertical retry distance.
const AWAY_VERTICAL_DIST: i32 = 5;
/// Vanilla `Slide.start`'s `new WalkTarget(BlockPos.containing(position), 0.6F, 1)`.
const SLIDE_CLOSE_ENOUGH: i32 = 1;

/// Vanilla `Slide extends Behavior<Breeze>`.
pub struct Slide {
    state: BehaviorState,
}

impl Slide {
    /// Vanilla `new Slide()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::with_entry_condition(vec![
                (&mem::ATTACK_TARGET, MemoryStatus::ValuePresent),
                (&mem::WALK_TARGET, MemoryStatus::ValueAbsent),
                (&mem::BREEZE_JUMP_COOLDOWN, MemoryStatus::ValueAbsent),
                (&mem::BREEZE_SHOOT, MemoryStatus::ValueAbsent),
            ]),
        }
    }

    /// Vanilla `DefaultRandomPos.getPosAway(mob, xzDist, yDist, awayFrom)`: the
    /// farthest sampled landing spot that is farther from `away_from` than the
    /// breeze already is.
    fn position_away(body: &SharedEntity, away_from: DVec3) -> Option<DVec3> {
        let mob = body.as_pathfinder_mob()?;
        let current = body.distance_to_sqr(away_from);

        let mut best: Option<(f64, DVec3)> = None;
        for _ in 0..10 {
            let Some(candidate) = land_random_pos(mob, AWAY_HORIZONTAL_DIST, AWAY_VERTICAL_DIST)
            else {
                continue;
            };
            let distance = candidate.distance_squared(away_from);
            if distance > current && best.is_none_or(|(best_distance, _)| distance > best_distance)
            {
                best = Some((distance, candidate));
            }
        }

        best.map(|(_, position)| position)
    }
}

impl Default for Slide {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for Slide {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.on_ground() && !ctx.body.is_in_water() && ctx.body.pose() == EntityPose::Standing
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return;
        };

        // Vanilla only tries the away-position when the breeze is inside the inner
        // circle; otherwise it jumps straight to the behind/middle-circle pick.
        let position = if super::brain::within_inner_circle_range(ctx.body, &target) {
            Self::position_away(ctx.body, target.position())
        } else {
            None
        };

        let position = position.unwrap_or_else(|| {
            let block = slide_target_position(ctx.body, &target);
            let (x, y, z) = block.get_center();
            DVec3::new(x, y, z)
        });

        let block = BlockPos::containing(position.x, position.y, position.z);
        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::from_block_pos(block, SPEED_MULTIPLIER_WHEN_SLIDING, SLIDE_CLOSE_ENOUGH),
        );
    }
}
