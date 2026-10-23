//! Vanilla `RangedCrossbowAttackGoal` (vanilla `RangedCrossbowAttackGoal` /
//! `CrossbowAttackMob.performCrossbowAttack`).
//!
//! **Dependency**: vanilla fires an `Arrow` through `CrossbowItem.performShooting`; Steel has
//! no `arrow` entity factory yet (`IllagerDependency::Arrow`), so the charge/aim state machine
//! below is complete and the projectile release is gated on that factory. When the factory
//! lands, `release_shot` only needs to create the arrow and set its motion.

use steel_utils::Downcast as _;

use crate::entity::{PathfinderMob, SharedEntity};

use super::selector::{Goal, GoalControls};

/// Vanilla `RangedCrossbowAttackGoal` charge ticks for a crossbow.
const CROSSBOW_CHARGE_TICKS: i32 = 25;
/// Vanilla `RangedCrossbowAttackGoal` "charge until shot" window.
const CROSSBOW_SHOT_TICKS: i32 = 5;
/// Vanilla `RangedCrossbowAttackGoal.SEE_TIME`.
const SEE_TIME: i32 = 5;

/// Vanilla `RangedCrossbowAttackGoal`.
pub struct RangedCrossbowAttackGoal {
    speed_modifier: f64,
    attack_radius_sqr: f64,
    attack_time: i32,
    see_time: i32,
    update_path_time: i32,
    can_attack: bool,
    charging: bool,
}

impl RangedCrossbowAttackGoal {
    /// Vanilla `RangedCrossbowAttackGoal(Mob, double, float)`.
    #[must_use]
    pub fn new(speed_modifier: f64, attack_radius: f32) -> Self {
        Self {
            speed_modifier,
            attack_radius_sqr: f64::from(attack_radius) * f64::from(attack_radius),
            attack_time: -1,
            see_time: -1,
            update_path_time: 0,
            can_attack: true,
            charging: false,
        }
    }

    /// Vanilla `RangedCrossbowAttackGoal.isHoldingCrossbow`.
    fn is_holding_crossbow(mob: &dyn PathfinderMob) -> bool {
        mob.is_holding(&mut |stack| {
            std::ptr::eq(stack.item(), &*steel_registry::vanilla_items::CROSSBOW)
        })
    }

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow` for the pillar of this goal: only
    /// pillagers are `CrossbowAttackMob`s, so the concrete capability is reached by key.
    fn set_charging_crossbow(mob: &dyn PathfinderMob, charging: bool) {
        if let Some(pillager) =
            mob.downcast_ref::<crate::entity::entities::mobs::illager::PillagerEntity>()
        {
            pillager.set_charging_crossbow(charging);
        }
    }

    /// Vanilla `RangedCrossbowAttackGoal.updatePath`.
    fn update_path(&mut self, mob: &dyn PathfinderMob, target: &SharedEntity) {
        let Some(world) = mob.level() else {
            return;
        };
        if self.update_path_time <= 0 {
            self.update_path_time = 20;
            mob.move_to_pos(target.position(), self.speed_modifier);
            let _ = world;
        } else {
            self.update_path_time -= 1;
        }
    }

    /// Vanilla `CrossbowAttackMob.performCrossbowAttack` projectile release.
    ///
    /// Gated on the missing `arrow` factory: without it there is nothing to shoot, so the
    /// goal keeps the vanilla charge/aim state instead of fabricating a projectile.
    fn release_shot(_mob: &dyn PathfinderMob, _target: &SharedEntity) -> bool {
        crate::entity::ENTITIES.has_factory(&steel_registry::vanilla_entities::ARROW)
    }
}

impl Goal for RangedCrossbowAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::is_holding_crossbow(mob) && mob.target().is_some()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        let in_range = mob.position().distance_squared(target.position()) <= self.attack_radius_sqr;
        Self::is_holding_crossbow(mob) && (self.can_attack || in_range)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(true);
        self.can_attack = false;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        mob.set_aggressive(false);
        Self::set_charging_crossbow(mob, false);
        self.attack_time = -1;
        self.see_time = -1;
        self.update_path_time = 0;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };

        let squared = mob.position().distance_squared(target.position());
        let can_see = mob.has_line_of_sight_cached(target.as_ref());
        let in_range = squared <= self.attack_radius_sqr;

        self.see_time = if can_see {
            self.see_time + 1
        } else {
            self.see_time - 1
        }
        .clamp(0, SEE_TIME);

        let aiming = self.see_time >= SEE_TIME;
        mob.set_aggressive(aiming);

        if in_range && self.see_time >= SEE_TIME {
            if self.attack_time < 0 {
                self.attack_time = CROSSBOW_CHARGE_TICKS + CROSSBOW_SHOT_TICKS;
                Self::set_charging_crossbow(mob, true);
            } else if self.attack_time > CROSSBOW_SHOT_TICKS {
                self.attack_time -= 1;
            } else if self.attack_time == CROSSBOW_SHOT_TICKS {
                if Self::release_shot(mob, &target) {
                    self.attack_time -= 1;
                } else {
                    // Dependency gate: keep charging until a projectile layer exists.
                    self.attack_time -= 1;
                }
            } else if self.attack_time > 0 {
                self.attack_time -= 1;
            } else {
                self.attack_time = CROSSBOW_CHARGE_TICKS + CROSSBOW_SHOT_TICKS;
            }
            Self::set_charging_crossbow(mob, self.attack_time > CROSSBOW_SHOT_TICKS);
        } else {
            self.charging = false;
            self.attack_time = -1;
            Self::set_charging_crossbow(mob, false);
        }

        if in_range {
            let _ = (can_see, entity);
        } else {
            self.update_path(mob, &target);
        }

        let target_eye =
            glam::DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
        mob.mob_base().controls().lock().look_control.set_look_at(
            target_eye,
            perfect_look_speed(),
            perfect_look_angle(),
        );
    }
}

/// Vanilla `RangedCrossbowAttackGoal` look speed while charging.
const fn perfect_look_speed() -> f32 {
    30.0
}

/// Vanilla `RangedCrossbowAttackGoal` pitch clamp while charging.
const fn perfect_look_angle() -> f32 {
    30.0
}
