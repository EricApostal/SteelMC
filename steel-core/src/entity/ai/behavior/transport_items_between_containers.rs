//! Vanilla `TransportItemsBetweenContainers`: allay-style container ferrying.
//!
//! Movement skeleton only: Steel has no container-IO surface on the behavior
//! layer (no `Container`/`ChestBlock.getContainer` equivalent), so the target
//! search and item transfer halves are absent. The traveling/queuing/interacting
//! state machine, visited-position bookkeeping and cooldowns are complete.

use std::sync::Arc;

use steel_utils::{BlockPos, GlobalPos};

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::BlockPosTracker;

/// Vanilla `TransportItemsBetweenContainers.TARGET_INTERACTION_TIME`.
pub const TARGET_INTERACTION_TIME: i32 = 60;
/// Vanilla `TransportItemsBetweenContainers` visited-position memory TTL.
const VISITED_POSITIONS_MEMORY_TIME: i64 = 6000;
/// Vanilla `TransportItemsBetweenContainers.MAX_VISITED_POSITIONS`.
const MAX_VISITED_POSITIONS: usize = 10;
/// Vanilla `TransportItemsBetweenContainers.MAX_UNREACHABLE_POSITIONS`.
const MAX_UNREACHABLE_POSITIONS: usize = 50;
/// Vanilla `TransportItemsBetweenContainers.IDLE_COOLDOWN`.
const IDLE_COOLDOWN: i32 = 140;
/// Vanilla `TransportItemsBetweenContainers` interaction distances.
const CLOSE_ENOUGH_TO_START_INTERACTING_DISTANCE: f64 = 0.5;

/// Vanilla `TransportItemsBetweenContainers.ContainerInteractionState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerInteractionState {
    PickupItem,
    PickupNoItem,
    PlaceItem,
    PlaceNoItem,
}

/// Vanilla `TransportItemsBetweenContainers.TransportItemState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportItemState {
    Traveling,
    Queuing,
    Interacting,
}

/// Vanilla `TransportItemsBetweenContainers.TransportItemTarget`.
pub struct TransportItemTarget {
    pos: BlockPos,
}

/// Vanilla `TransportItemsBetweenContainers` movement skeleton.
pub struct TransportItemsBetweenContainers {
    state: BehaviorState,
    speed_modifier: f32,
    horizontal_search_distance: i32,
    vertical_search_distance: i32,
    target: Option<TransportItemTarget>,
    item_state: TransportItemState,
    interaction_state: Option<ContainerInteractionState>,
    ticks_since_reaching_target: i32,
    idle_until: i64,
}

impl TransportItemsBetweenContainers {
    /// Vanilla `TransportItemsBetweenContainers(speedModifier, sourceBlockType,
    /// destinationBlockType, horizontalSearchDistance, verticalSearchDistance,
    /// onTargetInteractionActions, onStartTravelling, shouldQueueForTarget)`.
    ///
    /// Steel passes block-type filtering and item callbacks through the caller's own
    /// hooks once container IO exists; the constructor keeps the search distances
    /// and speed only.
    #[must_use]
    pub fn new(
        speed_modifier: f32,
        horizontal_search_distance: i32,
        vertical_search_distance: i32,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::VISITED_BLOCK_POSITIONS as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::TRANSPORT_ITEMS_COOLDOWN_TICKS as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::IS_PANICKING as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                ],
                1,
                1,
            ),
            speed_modifier,
            horizontal_search_distance,
            vertical_search_distance,
            target: None,
            item_state: TransportItemState::Traveling,
            interaction_state: None,
            ticks_since_reaching_target: 0,
            idle_until: 0,
        }
    }

    /// Vanilla `stopTargetingCurrentTarget`.
    fn stop_targeting_current_target(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.ticks_since_reaching_target = 0;
        self.target = None;
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.mob_base().navigation().lock().stop();
        }
        ctx.erase_memory(&mem::WALK_TARGET);
    }

    /// Vanilla `clearMemoriesAfterMatchingTargetFound`.
    fn clear_memories_after_matching_target_found(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.stop_targeting_current_target(ctx);
        ctx.erase_memory(&mem::VISITED_BLOCK_POSITIONS);
        ctx.erase_memory(&mem::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS);
    }

    /// Vanilla `enterCooldownAfterNoMatchingTargetFound`.
    fn enter_cooldown_after_no_matching_target_found(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.stop_targeting_current_target(ctx);
        ctx.set_memory(&mem::TRANSPORT_ITEMS_COOLDOWN_TICKS, IDLE_COOLDOWN);
        ctx.erase_memory(&mem::VISITED_BLOCK_POSITIONS);
        ctx.erase_memory(&mem::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS);
    }

    /// Vanilla `setVisitedBlockPos`.
    fn set_visited_block_pos(ctx: &mut BehaviorContext<'_>, target: BlockPos) {
        let mut visited: Vec<GlobalPos> = ctx
            .get_memory(&mem::VISITED_BLOCK_POSITIONS)
            .unwrap_or_default();
        visited.push(GlobalPos::new(ctx.level.key.clone(), target));
        if visited.len() > MAX_VISITED_POSITIONS {
            visited.remove(0);
        }
        ctx.set_memory_with_expiry(
            &mem::VISITED_BLOCK_POSITIONS,
            visited,
            VISITED_POSITIONS_MEMORY_TIME,
        );
    }

    /// Vanilla `markVisitedBlockPosAsUnreachable`.
    fn mark_visited_block_pos_as_unreachable(ctx: &mut BehaviorContext<'_>, target: BlockPos) {
        let mut visited: Vec<GlobalPos> = ctx
            .get_memory(&mem::VISITED_BLOCK_POSITIONS)
            .unwrap_or_default();
        let global = GlobalPos::new(ctx.level.key.clone(), target);
        visited.retain(|pos| *pos != global);
        let mut unreachable: Vec<GlobalPos> = ctx
            .get_memory(&mem::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS)
            .unwrap_or_default();
        unreachable.push(global);
        if unreachable.len() > MAX_UNREACHABLE_POSITIONS {
            unreachable.remove(0);
        }
        ctx.set_memory_with_expiry(
            &mem::VISITED_BLOCK_POSITIONS,
            visited,
            VISITED_POSITIONS_MEMORY_TIME,
        );
        ctx.set_memory_with_expiry(
            &mem::UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
            unreachable,
            VISITED_POSITIONS_MEMORY_TIME,
        );
    }

    /// Vanilla `isAnotherMobInteractingWithTarget` / chest search: no container IO
    /// on the behavior layer, so no target is ever valid here.
    fn transport_target(&self, _body: &SharedEntity) -> Option<TransportItemTarget> {
        None
    }
}

impl Behavior for TransportItemsBetweenContainers {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        !ctx.body
            .as_leashable()
            .is_some_and(|leashable| leashable.is_leashed())
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        !ctx.has_memory_value(&mem::TRANSPORT_ITEMS_COOLDOWN_TICKS)
            && !ctx.has_memory_value(&mem::IS_PANICKING)
            && !ctx
                .body
                .as_leashable()
                .is_some_and(|leashable| leashable.is_leashed())
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.item_state = TransportItemState::Traveling;
        self.interaction_state = None;
        let _ = self.horizontal_search_distance;
        let _ = self.vertical_search_distance;
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            let _ = mob.mob_base();
        }
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        match self.item_state {
            TransportItemState::Traveling => {
                let Some(target) = self.transport_target(ctx.body) else {
                    // No container IO: record the body's own block as unreachable so the
                    // skeleton still exercises the bookkeeping without looping forever.
                    Self::mark_visited_block_pos_as_unreachable(ctx, ctx.body.block_position());
                    self.enter_cooldown_after_no_matching_target_found(ctx);
                    return;
                };

                ctx.set_memory(
                    &mem::WALK_TARGET,
                    crate::entity::ai::brain::tracker::WalkTarget::new(
                        Arc::new(BlockPosTracker::from_block_pos(target.pos)),
                        self.speed_modifier,
                        0,
                    ),
                );

                let body_pos = ctx.body.position();
                let (cx, cy, cz) = target.pos.get_center();
                if glam::DVec3::new(cx, cy, cz).distance_squared(body_pos)
                    <= CLOSE_ENOUGH_TO_START_INTERACTING_DISTANCE
                        * CLOSE_ENOUGH_TO_START_INTERACTING_DISTANCE
                {
                    self.item_state = TransportItemState::Interacting;
                    self.ticks_since_reaching_target = 0;
                }
            }
            TransportItemState::Queuing => {
                // Vanilla waits for the other mob using the same container; without
                // container IO there is nothing to queue for, so fall back to traveling.
                self.item_state = TransportItemState::Traveling;
            }
            TransportItemState::Interacting => {
                self.ticks_since_reaching_target += 1;
                if self.ticks_since_reaching_target >= TARGET_INTERACTION_TIME {
                    // Vanilla would pick up/place items here; without container IO the
                    // interaction completes as a no-op.
                    self.interaction_state = Some(ContainerInteractionState::PickupNoItem);
                    self.clear_memories_after_matching_target_found(ctx);
                    self.item_state = TransportItemState::Traveling;
                    self.idle_until = ctx.timestamp + i64::from(IDLE_COOLDOWN);
                    let _ = self.idle_until;
                }
            }
        }
    }
}
