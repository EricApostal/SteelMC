//! Vanilla `LlamaFollowCaravanGoal` (`goal/LlamaFollowCaravanGoal.java`): a loose llama
//! joins a passing caravan and trails the head at two blocks.
//!
//! Vanilla stores the concrete `Llama`; Steel reads the caravan state through
//! [`CaravanLlama`](crate::entity::ai::goal::CaravanLlama), exactly as
//! `FollowFlockLeaderGoal` does for schooling fish.

use std::ptr;

use steel_registry::vanilla_entities::{LEASH_KNOT, LLAMA, TRADER_LLAMA};

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::leash::Leashable;
use crate::entity::{Entity, PathfinderMob, SharedEntity};

/// Vanilla `LlamaFollowCaravanGoal.CARAVAN_LIMIT`.
const CARAVAN_LIMIT: i32 = 8;
/// Vanilla `LlamaFollowCaravanGoal.tick` trail distance.
const TRAIL_DISTANCE: f64 = 2.0;
/// Vanilla `LlamaFollowCaravanGoal.canUse` search box (9 x 4 x 9).
const SEARCH_XZ: f64 = 9.0;
const SEARCH_Y: f64 = 4.0;
/// Vanilla `LlamaFollowCaravanGoal.canContinueToUse` far threshold (26 blocks squared).
const FAR_DISTANCE_SQR: f64 = 676.0;
/// Vanilla `LlamaFollowCaravanGoal.stop` speed reset.
const DEFAULT_SPEED_MODIFIER: f64 = 2.1;

/// Vanilla `LlamaFollowCaravanGoal`.
pub struct LlamaFollowCaravanGoal {
    speed_modifier: f64,
    dist_check_counter: i32,
}

impl LlamaFollowCaravanGoal {
    /// Vanilla `new LlamaFollowCaravanGoal(llama, speedModifier)`.
    #[must_use]
    pub const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            dist_check_counter: 0,
        }
    }

    /// Vanilla `LlamaFollowCaravanGoal.canUse` entity filter:
    /// `e.is(EntityTypes.LLAMA) || e.is(EntityTypes.TRADER_LLAMA)`.
    fn is_llama(entity: &dyn Entity) -> bool {
        let entity_type = entity.entity_type();
        ptr::eq(entity_type, &raw const LLAMA) || ptr::eq(entity_type, &raw const TRADER_LLAMA)
    }

    /// The closest candidate matching `predicate` by squared distance to `mob`.
    fn closest(
        mob: &dyn PathfinderMob,
        candidates: &[SharedEntity],
        predicate: impl Fn(&SharedEntity) -> bool,
    ) -> Option<SharedEntity> {
        candidates
            .iter()
            .filter(|entity| predicate(entity))
            .min_by(|a, b| {
                mob.position()
                    .distance_squared(a.position())
                    .total_cmp(&mob.position().distance_squared(b.position()))
            })
            .cloned()
    }

    /// Vanilla `Entity.isLeashed` for a shared entity handle.
    fn is_leashed(entity: &SharedEntity) -> bool {
        entity.as_leashable().is_some_and(Leashable::is_leashed)
    }

    /// Vanilla `LlamaFollowCaravanGoal.firstIsLeashed(currentMob, counter)`.
    fn first_is_leashed(current: &SharedEntity, counter: i32) -> bool {
        if counter > CARAVAN_LIMIT {
            return false;
        }

        let Some(llama) = current.as_caravan_llama() else {
            return false;
        };
        if !llama.in_caravan() {
            return false;
        }

        llama.caravan_head().is_some_and(|head| {
            Self::is_leashed(&head) || Self::first_is_leashed(&head, counter + 1)
        })
    }
}

impl Goal for LlamaFollowCaravanGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(self_llama) = mob.as_caravan_llama() else {
            return false;
        };
        if mob.is_leashed() || self_llama.in_caravan() {
            return false;
        }

        let Some(world) = mob.level() else {
            return false;
        };
        let search_box = mob
            .bounding_box()
            .inflate_xyz(SEARCH_XZ, SEARCH_Y, SEARCH_XZ);
        let llamas = world.get_entities_in_aabb_matching(&search_box, Self::is_llama);

        // Vanilla looks for a caravan member without a tail first, then a leashed llama.
        let closest = Self::closest(mob, &llamas, |entity| {
            entity
                .as_caravan_llama()
                .is_some_and(|llama| llama.in_caravan() && !llama.has_caravan_tail())
        })
        .or_else(|| {
            Self::closest(mob, &llamas, |entity| {
                Self::is_leashed(entity)
                    && entity
                        .as_caravan_llama()
                        .is_some_and(|llama| !llama.has_caravan_tail())
            })
        });

        let Some(closest) = closest else {
            return false;
        };
        if mob.position().distance_squared(closest.position()) < 4.0 {
            return false;
        }

        if !Self::is_leashed(&closest) && !Self::first_is_leashed(&closest, 1) {
            return false;
        }

        self_llama.join_caravan(&closest);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(self_llama) = mob.as_caravan_llama() else {
            return false;
        };
        if !self_llama.in_caravan() {
            return false;
        }
        let Some(mob_handle) = mob
            .level()
            .and_then(|world| world.get_entity_by_uuid(&mob.uuid()))
        else {
            return false;
        };
        if !Self::first_is_leashed(&mob_handle, 0) {
            return false;
        }

        let Some(head) = self_llama.caravan_head() else {
            return false;
        };
        if !head.is_alive() {
            return false;
        }

        if mob.position().distance_squared(head.position()) > FAR_DISTANCE_SQR {
            if self.speed_modifier <= 3.0 {
                self.speed_modifier *= 1.2;
                self.dist_check_counter = reduced_tick_delay(40);
                return true;
            }
            if self.dist_check_counter == 0 {
                return false;
            }
        }

        if self.dist_check_counter > 0 {
            self.dist_check_counter -= 1;
        }
        true
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(llama) = mob.as_caravan_llama() {
            llama.leave_caravan();
        }
        self.speed_modifier = DEFAULT_SPEED_MODIFIER;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(self_llama) = mob.as_caravan_llama() else {
            return;
        };
        if !self_llama.in_caravan() {
            return;
        }

        let Some(head) = self_llama.caravan_head() else {
            return;
        };
        // Vanilla checks the moving llama's own leash holder
        // (`this.llama.getLeashHolder() instanceof LeashFenceKnotEntity`): a llama tied to a
        // fence knot stays put instead of chasing the caravan head.
        let tied_to_fence_knot = mob.as_leashable().is_some_and(|leashable| {
            leashable
                .leash_holder()
                .is_some_and(|holder| ptr::eq(holder.entity_type(), &raw const LEASH_KNOT))
        });
        if tied_to_fence_knot {
            return;
        }

        let delta = head.position() - mob.position();
        let wanted = (delta.length() - TRAIL_DISTANCE).max(0.0);
        let step = delta.normalize_or_zero() * wanted;
        mob.move_to_pos(mob.position() + step, self.speed_modifier);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_follow_caravan_goal_claims_only_the_move_control() {
        let goal = LlamaFollowCaravanGoal::new(2.1);

        assert_eq!(goal.controls(), GoalControls::MOVE);
    }
}
