//! Vanilla `PlayTagWithOtherKids`.

use std::sync::Arc;

use steel_utils::BlockPos;

use super::random_pos::land_random_pos;
use super::utils;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, module_type as mem};
use crate::entity::ai::brain::tracker::{EntityTracker, WalkTarget};

/// Vanilla `PlayTagWithOtherKids.MAX_FLEE_XZ_DIST`.
const MAX_FLEE_XZ_DIST: i32 = 20;
/// Vanilla `PlayTagWithOtherKids.MAX_FLEE_Y_DIST`.
const MAX_FLEE_Y_DIST: i32 = 8;
/// Vanilla `PlayTagWithOtherKids.FLEE_SPEED_MODIFIER`.
const FLEE_SPEED_MODIFIER: f32 = 0.6;
/// Vanilla `PlayTagWithOtherKids.CHASE_SPEED_MODIFIER`.
const CHASE_SPEED_MODIFIER: f32 = 0.6;
/// Vanilla `PlayTagWithOtherKids.MAX_CHASERS_PER_TARGET`.
const MAX_CHASERS_PER_TARGET: i32 = 5;
/// Vanilla `PlayTagWithOtherKids.AVERAGE_WAIT_TIME_BETWEEN_RUNS` (1-in-10 gate).
const WAIT_CHANCE: i32 = 10;

/// Vanilla `PlayTagWithOtherKids`: babies chase one another, or scatter when chased.
pub struct PlayTagWithOtherKids;

impl PlayTagWithOtherKids {
    /// Vanilla `PlayTagWithOtherKids.create`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![
            &mem::VISIBLE_VILLAGER_BABIES as MemoryModuleTypeRef,
            &mem::WALK_TARGET as MemoryModuleTypeRef,
            &mem::LOOK_TARGET as MemoryModuleTypeRef,
            &mem::INTERACTION_TARGET as MemoryModuleTypeRef,
        ])
    }

    /// Vanilla `PlayTagWithOtherKids.whoAreYouChasing`.
    fn interaction_target(entity: &SharedEntity) -> Option<SharedEntity> {
        entity
            .as_mob()?
            .brain()?
            .lock()
            .get_memory(&mem::INTERACTION_TARGET)
    }

    /// Vanilla `PlayTagWithOtherKids.isFriendChasingMe`.
    fn is_friend_chasing_me(me: &SharedEntity, friend: &SharedEntity) -> bool {
        Self::interaction_target(friend).is_some_and(|target| Arc::ptr_eq(&target, me))
    }

    /// Vanilla `PlayTagWithOtherKids.findSomeoneBeingChased`.
    fn find_someone_being_chased(friends: &[SharedEntity]) -> Option<SharedEntity> {
        let mut chased: Vec<(SharedEntity, i32)> = Vec::new();
        for friend in friends {
            let Some(target) = Self::interaction_target(friend) else {
                continue;
            };

            if let Some(entry) = chased
                .iter_mut()
                .find(|(entity, _)| Arc::ptr_eq(entity, &target))
            {
                entry.1 += 1;
            } else {
                chased.push((target, 1));
            }
        }

        chased.sort_by_key(|(_, count)| *count);
        chased
            .into_iter()
            .find(|(_, count)| *count > 0 && *count <= MAX_CHASERS_PER_TARGET)
            .map(|(entity, _)| entity)
    }

    /// Vanilla `PlayTagWithOtherKids.chaseKid`.
    fn chase_kid(ctx: &mut BehaviorContext<'_>, kid: &SharedEntity) {
        ctx.set_memory(&mem::INTERACTION_TARGET, kid.clone());
        utils::set_look_target(ctx, Arc::new(EntityTracker::new(kid.clone(), true)));
        utils::set_walk_target(
            ctx,
            Arc::new(EntityTracker::new(kid.clone(), false)),
            CHASE_SPEED_MODIFIER,
            1,
        );
    }
}

impl Trigger for PlayTagWithOtherKids {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if rand::random_range(0..WAIT_CHANCE) != 0 {
            return false;
        }

        if !ctx.has_memory_value(&mem::VISIBLE_VILLAGER_BABIES)
            || ctx.has_memory_value(&mem::WALK_TARGET)
        {
            return false;
        }

        let Some(friends) = ctx.get_memory(&mem::VISIBLE_VILLAGER_BABIES) else {
            return false;
        };

        let being_chased = friends
            .iter()
            .any(|friend| Self::is_friend_chasing_me(ctx.body, friend));

        if !being_chased {
            if let Some(chased) = Self::find_someone_being_chased(&friends) {
                Self::chase_kid(ctx, &chased);
            } else if let Some(friend) = friends.first() {
                Self::chase_kid(ctx, friend);
            }
            return true;
        }

        let Some(mob) = ctx.body.as_pathfinder_mob() else {
            return true;
        };
        for _ in 0..10 {
            let Some(pos) = land_random_pos(mob, MAX_FLEE_XZ_DIST, MAX_FLEE_Y_DIST) else {
                continue;
            };
            if ctx
                .level
                .is_village(BlockPos::containing(pos.x, pos.y, pos.z))
            {
                ctx.set_memory(
                    &mem::WALK_TARGET,
                    WalkTarget::from_vec(pos, FLEE_SPEED_MODIFIER, 0),
                );
                break;
            }
        }

        true
    }
}
