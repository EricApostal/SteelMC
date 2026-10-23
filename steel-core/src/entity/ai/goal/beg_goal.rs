//! Vanilla `BegGoal` (`goal/BegGoal.java`): a wolf stares at the nearest player holding food
//! it wants.
//!
//! Vanilla stores the `Wolf` and asks `wolf.isFood(item)`; Steel takes that food test as a
//! constructor predicate, matching how [`TemptGoal`](super::tempt_goal) receives its item
//! filter, and keeps the interested flag on the mob through
//! [`Tameable::set_is_interested`](crate::entity::ai::goal::Tameable::set_is_interested).

use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;

use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{LivingEntity as _, PathfinderMob, SharedEntity};
use crate::player::Player;
use crate::world::World;

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};

/// Vanilla `BegGoal.start`: `40 + random.nextInt(40)` look ticks.
const MIN_LOOK_TIME_TICKS: i32 = 40;
const RANDOM_LOOK_TIME_TICKS: i32 = 40;
/// Vanilla `BegGoal.tick` look speed.
const LOOK_Y_MAX_ROT_SPEED: f32 = 10.0;

type IsFood = Box<dyn Fn(&ItemStack) -> bool + Send + Sync>;

/// Vanilla `BegGoal`.
pub struct BegGoal {
    player: Option<SharedEntity>,
    look_distance: f32,
    look_time: i32,
    beg_targeting: TargetingConditions,
    is_food: IsFood,
}

impl BegGoal {
    /// Vanilla `new BegGoal(wolf, lookDistance)`; `is_food` mirrors `Wolf.isFood`.
    #[must_use]
    pub fn new(
        look_distance: f32,
        is_food: impl Fn(&ItemStack) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            player: None,
            look_distance,
            look_time: 0,
            beg_targeting: TargetingConditions::for_non_combat().range(f64::from(look_distance)),
            is_food: Box::new(is_food),
        }
    }

    /// Vanilla `BegGoal.playerHoldingInteresting`: bones or the mob's own food, in either hand.
    fn player_holding_interesting(&self, player: &Player) -> bool {
        let holds_interesting = |slot: EquipmentSlot| {
            let mut holds = false;
            player.with_equipment_slot(slot, &mut |item_stack| {
                holds = (self.is_food)(item_stack)
                    || std::ptr::eq(item_stack.item(), &*vanilla_items::BONE);
            });
            holds
        };

        holds_interesting(EquipmentSlot::MainHand) || holds_interesting(EquipmentSlot::OffHand)
    }
}

impl Goal for BegGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        let origin = mob.position();
        let follow = f64::from(self.look_distance);
        let conditions = self.beg_targeting.clone();
        let level: &World = world.as_ref();
        let Some(player) = level.nearest_player(origin, follow, |player| {
            conditions.test(level, Some(mob), player)
        }) else {
            return false;
        };

        if !self.player_holding_interesting(&player) {
            return false;
        }

        self.player = Some(player);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(player) = &self.player else {
            return false;
        };
        if !player.is_alive() {
            return false;
        }

        let look_distance = f64::from(self.look_distance);
        if mob.position().distance_squared(player.position()) > look_distance * look_distance {
            return false;
        }

        let holds_interesting = player
            .as_player()
            .is_some_and(|player| self.player_holding_interesting(player));
        self.look_time > 0 && holds_interesting
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(tameable) = mob.as_tameable() {
            tameable.set_is_interested(true);
        }
        self.look_time =
            reduced_tick_delay(MIN_LOOK_TIME_TICKS + rand::random_range(0..RANDOM_LOOK_TIME_TICKS));
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(tameable) = mob.as_tameable() {
            tameable.set_is_interested(false);
        }
        self.player = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(player) = &self.player else {
            return;
        };

        let player_position = player.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            glam::DVec3::new(player_position.x, player.get_eye_y(), player_position.z),
            LOOK_Y_MAX_ROT_SPEED,
            mob.max_head_x_rot(),
        );
        self.look_time -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beg_goal_claims_only_the_look_control() {
        let goal = BegGoal::new(8.0, |_| false);

        assert_eq!(goal.controls(), GoalControls::LOOK);
    }
}
