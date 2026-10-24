//! Vanilla `DefendVillageTargetGoal` (`goal/target/DefendVillageTargetGoal.java`).
//!
//! The iron golem attacks a player whose reputation with any nearby villager dropped to `-100`
//! or lower, i.e. who has been seen hurting or killing villagers.

use steel_registry::vanilla_entities;
use steel_utils::types::GameType;
use uuid::Uuid;

use super::super::selector::{Goal, GoalControls};
use super::super::target_goal::TargetGoalBase;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{Entity, PathfinderMob, SharedEntity};

const HORIZONTAL_RANGE: f64 = 10.0;
const VERTICAL_RANGE: f64 = 8.0;
const VILLAGE_DEFENSE_REPUTATION: i32 = -100;
/// Vanilla `DefendVillageTargetGoal.attackTargeting` range.
const ATTACK_TARGETING_RANGE: f64 = 64.0;

/// Reputation lookup for a candidate villager (vanilla `Villager.getPlayerReputation`).
pub(crate) type PlayerReputation = fn(&dyn Entity, &Uuid) -> Option<i32>;

pub(crate) struct DefendVillageTargetGoal {
    target_goal: TargetGoalBase,
    potential_target: Option<SharedEntity>,
    player_reputation: PlayerReputation,
}

impl DefendVillageTargetGoal {
    /// Creates the goal with the villager-reputation provider used to spot village attackers.
    #[must_use]
    pub(crate) fn new(player_reputation: PlayerReputation) -> Self {
        Self {
            // Vanilla `super(golem, false, true)`: no line-of-sight requirement, must be reachable.
            target_goal: TargetGoalBase::new(false, true),
            potential_target: None,
            player_reputation,
        }
    }
}

impl Goal for DefendVillageTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        // Vanilla `attackTargeting`: `TargetingConditions.forCombat().range(64.0)` filters both
        // the nearby villagers and the nearby players.
        let attack_targeting = TargetingConditions::for_combat().range(ATTACK_TARGETING_RANGE);
        let grow =
            mob.bounding_box()
                .inflate_xyz(HORIZONTAL_RANGE, VERTICAL_RANGE, HORIZONTAL_RANGE);
        let villagers = world.get_entities_in_aabb_matching(&grow, |entity| {
            entity.entity_type() == &vanilla_entities::VILLAGER
                && entity.as_living_entity().is_some_and(|villager| {
                    attack_targeting.test(world.as_ref(), Some(mob), villager)
                })
        });
        let players: Vec<SharedEntity> = world.get_entities_in_aabb_matching(&grow, |entity| {
            entity.as_player().is_some()
                && entity
                    .as_living_entity()
                    .is_some_and(|player| attack_targeting.test(world.as_ref(), Some(mob), player))
        });

        // Vanilla never clears `potentialTarget`: it is only overwritten when a villager/player
        // pair qualifies, so a previous candidate survives calls with no villagers in range.
        for villager in &villagers {
            for player in &players {
                let reputation = (self.player_reputation)(villager.as_ref(), &player.uuid());
                if reputation.is_some_and(|value| value <= VILLAGE_DEFENSE_REPUTATION) {
                    self.potential_target = Some(SharedEntity::clone(player));
                }
            }
        }

        self.potential_target.as_ref().is_some_and(|target| {
            !target.as_player().is_some_and(|player| {
                player.is_spectator() || player.game_mode() == GameType::Creative
            })
        })
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.target_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let _ = mob.set_target(self.potential_target.as_ref());
        // Vanilla `TargetGoal.start` only resets the reach/unseen caches: `targetMob` stays
        // unset in this goal.
        self.target_goal.start();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        // Vanilla `DefendVillageTargetGoal` does not override `stop`, so `potentialTarget`
        // survives; only `TargetGoal.stop` clears the live target.
        self.target_goal.stop(mob);
    }
}
