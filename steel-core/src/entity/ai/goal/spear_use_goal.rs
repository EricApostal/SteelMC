//! Vanilla `SpearUseGoal` (`goal/SpearUseGoal.java`): a monster holding a kinetic weapon
//! charges, drives the spear in, then backs off to repeat.
//!
//! Vanilla stores the concrete `Monster`; Steel reads the kinetic-weapon component from the
//! mob's main hand and drives the state machine through the shared navigation.

use glam::DVec3;
use steel_registry::data_components::components::KineticWeapon;
use steel_registry::data_components::vanilla_components::KINETIC_WEAPON;
use steel_registry::equipment::EquipmentSlot;
use steel_utils::types::InteractionHand;

use super::random_pos::default_random_pos_away;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::ai::control::rotate_towards;
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `SpearUseGoal.MAX_FLEEING_TIME`.
const MAX_FLEEING_TIME: i32 = 50;
/// Vanilla `SpearUseGoal.tick` reposition/cooldown maximum distances.
const MAX_REPOSITION_DISTANCE: i32 = 7;
const MAX_COOLDOWN_DISTANCE: i32 = 11;
/// Vanilla `SpearUseGoal.tick` vertical distance passed to `getPosAway`.
const RETREAT_VERTICAL_DISTANCE: i32 = 7;
/// Vanilla `SpearUseGoal.tick` mount distance added while riding.
const MOUNT_DISTANCE: i32 = 2;
/// Vanilla `SpearUseGoal.tick` look speed and pitch clamp.
const AIM_SPEED: f32 = 30.0;
const AIM_ANGLE: f32 = 30.0;

/// Vanilla `SpearUseGoal.SpearUseState`.
struct SpearUseState {
    engage_time: i32,
    fleeing_time: i32,
    away_pos: Option<DVec3>,
    done: bool,
}

impl SpearUseState {
    const fn new() -> Self {
        Self {
            engage_time: -1,
            fleeing_time: -1,
            away_pos: None,
            done: false,
        }
    }

    /// Vanilla `SpearUseState.notEngagedYet`.
    const fn not_engaged_yet(&self) -> bool {
        self.engage_time < 0
    }

    /// Vanilla `SpearUseState.startEngagement`.
    const fn start_engagement(&mut self, spear_down_time: i32) {
        self.engage_time = spear_down_time;
    }

    /// Vanilla `SpearUseState.tickAndCheckEngagement`.
    const fn tick_and_check_engagement(&mut self) -> bool {
        if self.engage_time > 0 {
            self.engage_time -= 1;
            if self.engage_time == 0 {
                return true;
            }
        }
        false
    }

    /// Vanilla `SpearUseState.tickAndCheckFleeing`.
    const fn tick_and_check_fleeing(&mut self) -> bool {
        if self.fleeing_time > 0 {
            self.fleeing_time += 1;
            if self.fleeing_time > MAX_FLEEING_TIME {
                self.done = true;
                return true;
            }
        }
        false
    }
}

/// Vanilla `SpearUseGoal<T extends Monster>`.
pub struct SpearUseGoal {
    state: Option<SpearUseState>,
    speed_modifier_when_charging: f64,
    speed_modifier_when_repositioning: f64,
    approach_distance_sqr: f32,
    target_in_range_radius_sqr: f32,
}

impl SpearUseGoal {
    /// Vanilla `new SpearUseGoal(mob, speedWhenCharging, speedWhenRepositioning,
    /// approachDistance, targetInRangeRadius)`.
    #[must_use]
    pub const fn new(
        speed_modifier_when_charging: f64,
        speed_modifier_when_repositioning: f64,
        approach_distance: f32,
        target_in_range_radius: f32,
    ) -> Self {
        Self {
            state: None,
            speed_modifier_when_charging,
            speed_modifier_when_repositioning,
            approach_distance_sqr: approach_distance * approach_distance,
            target_in_range_radius_sqr: target_in_range_radius * target_in_range_radius,
        }
    }

    /// Vanilla `Mob.lookAt(target, 30.0F, 30.0F)`: rotate the body toward the target by at
    /// most the given angles this tick.
    fn look_at_target(mob: &dyn PathfinderMob, target: &SharedEntity) {
        let mob_pos = mob.position();
        let target_pos = target.position();
        let xd = target_pos.x - mob_pos.x;
        let zd = target_pos.z - mob_pos.z;
        let yd = target.get_eye_y() - mob.get_eye_y();
        let horizontal = (xd * xd + zd * zd).sqrt();
        let y_rot_d = (zd.atan2(xd).to_degrees() as f32) - 90.0;
        let x_rot_d = -(yd.atan2(horizontal).to_degrees() as f32);
        let (y_rot, x_rot) = mob.rotation();
        mob.set_rotation((
            rotate_towards(y_rot, y_rot_d, AIM_SPEED),
            rotate_towards(x_rot, x_rot_d, AIM_ANGLE),
        ));
    }

    /// Vanilla `SpearUseGoal.able_to_attack`: a target and a kinetic weapon in the main hand.
    fn able_to_attack(mob: &dyn PathfinderMob) -> bool {
        if mob.target().is_none() {
            return false;
        }

        let mut holds_kinetic_weapon = false;
        mob.with_equipment_slot(EquipmentSlot::MainHand, &mut |item_stack| {
            holds_kinetic_weapon = item_stack.has(KINETIC_WEAPON);
        });
        holds_kinetic_weapon
    }

    /// Vanilla `SpearUseGoal.getKineticWeaponUseDuration`.
    fn kinetic_weapon_use_duration(mob: &dyn PathfinderMob) -> i32 {
        let mut duration = 0;
        mob.with_equipment_slot(EquipmentSlot::MainHand, &mut |item_stack| {
            duration = item_stack
                .get(KINETIC_WEAPON)
                .map_or(0, KineticWeapon::compute_damage_use_duration);
        });
        reduced_tick_delay(duration)
    }
}

impl Goal for SpearUseGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::able_to_attack(mob) && !mob.is_using_item()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.state.as_ref().is_some_and(|state| !state.done) && Self::able_to_attack(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(true);
        self.state = Some(SpearUseState::new());
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
        mob.set_aggressive(false);
        self.state = None;
        mob.stop_using_item();
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };
        let Some(mut state) = self.state.take() else {
            return;
        };

        let target_distance_sqr = mob.position().distance_squared(target.position());
        let distance = target_distance_sqr.sqrt();
        let mount_distance = if mob.is_passenger() {
            MOUNT_DISTANCE
        } else {
            0
        };
        // Vanilla `getRootVehicle() instanceof Mob ? chargeSpeedModifier() : 1.0F`; the root
        // vehicle of an unridden mob is the mob itself.
        let root = mob
            .root_vehicle()
            .unwrap_or_else(|| SharedEntity::clone(entity));
        let speed_modifier = root
            .as_mob()
            .map_or(1.0, |vehicle| f64::from(vehicle.charge_speed_modifier()));

        Self::look_at_target(mob, &target);
        let target_position = target.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            DVec3::new(target_position.x, target.get_eye_y(), target_position.z),
            AIM_SPEED,
            AIM_ANGLE,
        );

        if state.not_engaged_yet() {
            if target_distance_sqr > f64::from(self.approach_distance_sqr) {
                mob.move_to_pos(
                    target.position(),
                    speed_modifier * self.speed_modifier_when_repositioning,
                );
                self.state = Some(state);
                return;
            }

            let use_duration = Self::kinetic_weapon_use_duration(mob);
            state.start_engagement(use_duration);
            mob.start_using_item(InteractionHand::MainHand);
        }

        if state.tick_and_check_engagement() {
            mob.stop_using_item();
            // Vanilla `LandRandomPos.getPosAway(mob, max(0, 9 + mount - dist),
            // max(1, 11 + mount - dist), 7, target)`. Steel only has the
            // `DefaultRandomPos`-shaped helper (no min/max horizontal pair and no
            // "move up out of solid, reject water/malus" land pass), so the max distance is
            // used as the sole horizontal bound.
            let cooldown_distance =
                (f64::from(MAX_COOLDOWN_DISTANCE) + f64::from(mount_distance) - distance).max(1.0);
            state.away_pos = default_random_pos_away(
                mob,
                cooldown_distance as i32,
                RETREAT_VERTICAL_DISTANCE,
                target.position(),
            );
            state.fleeing_time = 1;
        }

        if !state.tick_and_check_fleeing() {
            if let Some(away_pos) = state.away_pos {
                mob.move_to_pos(
                    away_pos,
                    speed_modifier * self.speed_modifier_when_repositioning,
                );
                if mob.mob_base().navigation().lock().is_done() {
                    if state.fleeing_time > 0 {
                        state.done = true;
                        self.state = Some(state);
                        return;
                    }
                    state.away_pos = None;
                }
            } else {
                mob.move_to_pos(
                    target.position(),
                    speed_modifier * self.speed_modifier_when_charging,
                );
                if target_distance_sqr < f64::from(self.target_in_range_radius_sqr)
                    || mob.mob_base().navigation().lock().is_done()
                {
                    // Vanilla `LandRandomPos.getPosAway(mob, 6 + mount - dist,
                    // 7 + mount - dist, 7, target)`; max distance only, as above.
                    let reposition_distance =
                        f64::from(MAX_REPOSITION_DISTANCE) + f64::from(mount_distance) - distance;
                    state.away_pos = default_random_pos_away(
                        mob,
                        reposition_distance as i32,
                        RETREAT_VERTICAL_DISTANCE,
                        target.position(),
                    );
                }
            }
        }

        self.state = Some(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spear_use_goal_uses_move_and_look_controls() {
        let goal = SpearUseGoal::new(1.0, 0.6, 4.0, 6.0);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::LOOK);
        assert_eq!(goal.approach_distance_sqr, 16.0);
        assert_eq!(goal.target_in_range_radius_sqr, 36.0);
    }

    #[test]
    fn spear_use_state_matches_vanilla_transitions() {
        let mut state = SpearUseState::new();

        assert!(state.not_engaged_yet());
        state.start_engagement(2);
        assert!(!state.not_engaged_yet());
        assert!(!state.tick_and_check_engagement());
        assert!(state.tick_and_check_engagement());

        state.fleeing_time = 1;
        assert!(!state.tick_and_check_fleeing());
        state.fleeing_time = MAX_FLEEING_TIME;
        assert!(state.tick_and_check_fleeing());
        assert!(state.done);
    }
}
