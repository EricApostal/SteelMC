//! Vanilla `SetWalkTargetAwayFrom`: walk away from a remembered entity or block.

use std::sync::Arc;

use glam::DVec3;
use steel_utils::BlockPos;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleType, module_type as mem};
use crate::entity::ai::brain::tracker::{BlockPosTracker, WalkTarget};
use crate::entity::ai::goal::default_random_pos_away;

/// Vanilla `SetWalkTargetAwayFrom` for a `BlockPos` memory (`SetWalkTargetAwayFrom.pos`).
#[derive(Clone, Copy)]
enum WalkAwayFrom {
    /// Vanilla `Vec3::atBottomCenterOf`.
    Pos(&'static MemoryModuleType<BlockPos>),
    /// Vanilla `Entity::position`.
    Entity(&'static MemoryModuleType<SharedEntity>),
}

/// Vanilla `SetWalkTargetAwayFrom`.
pub struct SetWalkTargetAwayFrom {
    memory: WalkAwayFrom,
    speed_modifier: f32,
    desired_distance: f64,
    interrupt_current_walk: bool,
}

impl SetWalkTargetAwayFrom {
    /// Vanilla `SetWalkTargetAwayFrom.entity(memory, speed, desiredDistance, interruptCurrentWalk)`.
    #[must_use]
    pub fn entity(
        memory: &'static MemoryModuleType<SharedEntity>,
        speed_modifier: f32,
        desired_distance: i32,
        interrupt_current_walk: bool,
    ) -> OneShot<Self> {
        Self::create(
            WalkAwayFrom::Entity(memory),
            speed_modifier,
            desired_distance,
            interrupt_current_walk,
        )
    }

    /// Vanilla `SetWalkTargetAwayFrom.pos(memory, speed, desiredDistance, interruptCurrentWalk)`.
    #[must_use]
    pub fn pos(
        memory: &'static MemoryModuleType<BlockPos>,
        speed_modifier: f32,
        desired_distance: i32,
        interrupt_current_walk: bool,
    ) -> OneShot<Self> {
        Self::create(
            WalkAwayFrom::Pos(memory),
            speed_modifier,
            desired_distance,
            interrupt_current_walk,
        )
    }

    fn create(
        memory: WalkAwayFrom,
        speed_modifier: f32,
        desired_distance: i32,
        interrupt_current_walk: bool,
    ) -> OneShot<Self> {
        let required: Vec<crate::entity::ai::brain::memory::MemoryModuleTypeRef> = match memory {
            WalkAwayFrom::Pos(memory) => vec![&mem::WALK_TARGET, memory],
            WalkAwayFrom::Entity(memory) => vec![&mem::WALK_TARGET, memory],
        };

        OneShot::new(Self {
            memory,
            speed_modifier,
            desired_distance: f64::from(desired_distance),
            interrupt_current_walk,
        })
        .with_required_memories(required)
    }

    fn avoid_position(&self, ctx: &BehaviorContext<'_>) -> Option<DVec3> {
        match self.memory {
            WalkAwayFrom::Pos(memory) => ctx.get_memory(memory).map(|pos| {
                let (x, y, z) = pos.get_bottom_center();
                DVec3::new(x, y, z)
            }),
            WalkAwayFrom::Entity(memory) => ctx.get_memory(memory).map(|entity| entity.position()),
        }
    }
}

impl Trigger for SetWalkTargetAwayFrom {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let target = ctx.get_memory(&mem::WALK_TARGET);
        if target.is_some() && !self.interrupt_current_walk {
            return false;
        }

        let body = ctx.body;
        let body_position = body.position();
        let Some(avoid_position) = self.avoid_position(ctx) else {
            return false;
        };

        if body_position.distance_squared(avoid_position)
            >= self.desired_distance * self.desired_distance
        {
            return false;
        }

        if let Some(target) = &target
            && target.speed_modifier() == self.speed_modifier
        {
            let current_direction = target.target().current_position() - body_position;
            let avoid_direction = avoid_position - body_position;
            if current_direction.dot(avoid_direction) < 0.0 {
                return false;
            }
        }

        let Some(mob) = body.as_pathfinder_mob() else {
            return false;
        };

        // Vanilla uses `LandRandomPos.getPosAway`, which Steel has no equivalent of;
        // `default_random_pos_away` generates the same away-facing candidates without
        // the land (solid/water) filter.
        for _ in 0..10 {
            let Some(flee_position) = default_random_pos_away(mob, 16, 7, avoid_position) else {
                continue;
            };

            ctx.set_memory(
                &mem::WALK_TARGET,
                WalkTarget::new(
                    Arc::new(BlockPosTracker::from_vec(flee_position)),
                    self.speed_modifier,
                    0,
                ),
            );
            break;
        }

        true
    }
}
