//! Vanilla `ResetUniversalAngerTargetGoal` (`goal/target/ResetUniversalAngerTargetGoal.java`).
//!
//! With `UNIVERSAL_ANGER` on, a mob that was just hurt by a player drops its current target and
//! restarts its persistent anger timer so that every player becomes a valid anger target again.

use steel_registry::vanilla_game_rules::UNIVERSAL_ANGER;
use steel_utils::WorldAabb;

use super::super::selector::{Goal, GoalControls};
use super::super::target_goal::follow_distance;
use crate::entity::{PathfinderMob, SharedEntity};

const ALERT_RANGE_Y: f64 = 10.0;

pub(crate) struct ResetUniversalAngerTargetGoal {
    alert_others_of_same_type: bool,
    last_hurt_by_player_timestamp: i32,
}

impl ResetUniversalAngerTargetGoal {
    /// Creates the goal; `alert_others_of_same_type` mirrors vanilla's constructor argument.
    #[must_use]
    pub(crate) const fn new(alert_others_of_same_type: bool) -> Self {
        Self {
            alert_others_of_same_type,
            last_hurt_by_player_timestamp: 0,
        }
    }

    /// Vanilla `ResetUniversalAngerTargetGoal.wasHurtByPlayer`.
    fn was_hurt_by_player(&self, mob: &dyn PathfinderMob) -> bool {
        mob.last_hurt_by_mob()
            .is_some_and(|hurt_by| hurt_by.as_player().is_some())
            && mob.last_hurt_by_mob_timestamp() > self.last_hurt_by_player_timestamp
    }

    /// Vanilla `ResetUniversalAngerTargetGoal.getNearbyMobsOfSameType` alert pass.
    fn alert_others(&self, mob: &dyn PathfinderMob) {
        let Some(world) = mob.level() else {
            return;
        };
        let within = follow_distance(mob);
        let position = mob.position();
        let search_box = WorldAabb::new(
            position.x,
            position.y,
            position.z,
            position.x + 1.0,
            position.y + 1.0,
            position.z + 1.0,
        )
        .inflate_xyz(within, ALERT_RANGE_Y, within);
        let mob_type_key = mob.downcast_type_key();
        let mob_uuid = mob.uuid();

        let others: Vec<SharedEntity> =
            world.get_entities_in_aabb_matching(&search_box, |entity| {
                entity.uuid() != mob_uuid
                    && entity.downcast_type_key() == mob_type_key
                    && !entity.is_spectator()
            });

        for other in others {
            // Vanilla casts every same-class neighbour to `NeutralMob`; Steel only touches the
            // ones that actually carry persistent anger state.
            if let Some(neutral) = other.as_neutral_mob() {
                neutral.forget_current_target_and_refresh_universal_anger();
            }
        }
    }
}

impl Goal for ResetUniversalAngerTargetGoal {
    fn controls(&self) -> GoalControls {
        // Vanilla sets no goal flags on this goal.
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.as_neutral_mob().is_some()
            && mob
                .level()
                .is_some_and(|world| world.get_game_rule(&UNIVERSAL_ANGER))
            && self.was_hurt_by_player(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.last_hurt_by_player_timestamp = mob.last_hurt_by_mob_timestamp();
        let Some(neutral) = mob.as_neutral_mob() else {
            return;
        };
        neutral.forget_current_target_and_refresh_universal_anger();

        if self.alert_others_of_same_type {
            self.alert_others(mob);
        }
    }
}
