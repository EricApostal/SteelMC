//! Vanilla `ChargeAttack`: rush the attack target and knock back whatever it hits.

use glam::DVec3;
use steel_registry::vanilla_attributes;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::{EntityAnchor, SharedEntity};

/// Vanilla `ChargeAttack` (no fixed duration; the default 60-tick window is
/// irrelevant because `canStillUse` gates every tick).
const DEFAULT_DURATION: i32 = 60;

/// Vanilla `ChargeAttack<Animal>`.
pub struct ChargeAttack {
    state: BehaviorState,
    time_between_attacks: i32,
    charge_targeting: TargetingConditions,
    speed: f32,
    knockback_force: f32,
    max_target_detection_distance: f64,
    max_charge_distance: f64,
    charge_velocity_vector: DVec3,
    start_position: DVec3,
}

impl ChargeAttack {
    /// Vanilla `ChargeAttack(timeBetweenAttacks, chargeTargeting, speed, knockbackForce,
    /// maxChargeDistance, maxTargetDetectionDistance, chargeSound)`.
    ///
    /// Steel carries no sound handle on the behavior layer, so `chargeSound` is not
    /// taken here; the charge/hit half is complete.
    #[must_use]
    pub fn new(
        time_between_attacks: i32,
        charge_targeting: TargetingConditions,
        speed: f32,
        knockback_force: f32,
        max_charge_distance: f64,
        max_target_detection_distance: f64,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::CHARGE_COOLDOWN_TICKS as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::ATTACK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                ],
                DEFAULT_DURATION,
                DEFAULT_DURATION,
            ),
            time_between_attacks,
            charge_targeting,
            speed,
            knockback_force,
            max_target_detection_distance,
            max_charge_distance,
            charge_velocity_vector: DVec3::ZERO,
            start_position: DVec3::ZERO,
        }
    }

    /// Vanilla `dealDamageToTarget`.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror the vanilla `ChargeAttack` instance call site"
    )]
    fn deal_damage_to_target(&self, ctx: &mut BehaviorContext<'_>, target: &SharedEntity) {
        if let Some(mob) = ctx.body.as_mob() {
            let _ = mob.do_hurt_target(ctx.body, ctx.level, target);
        }
    }

    /// Vanilla `dealKnockBack`.
    fn deal_knockback(&self, ctx: &BehaviorContext<'_>, target: &SharedEntity) {
        // Vanilla adds `0.25F * (speedAmplifier - slownessAmplifier)`; Steel's
        // effect surface has no amplifier accessor here, so the unmodified factor
        // is used (equivalent for mobs without Speed/Slowness).
        let speed_boost_power = 0.0;
        let movement_speed = ctx.body.as_living_entity().map_or(1.0, |living| {
            living
                .attributes()
                .lock()
                .required_value(vanilla_attributes::MOVEMENT_SPEED) as f32
        });
        let speed_factor = (self.speed * movement_speed).clamp(0.2, 2.0) + speed_boost_power;
        if let Some(living) = target.as_living_entity() {
            living.knockback(
                f64::from(speed_factor * self.knockback_force),
                ctx.body.velocity().x,
                ctx.body.velocity().z,
            );
        }
    }
}

impl Behavior for ChargeAttack {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::ATTACK_TARGET)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(attack_target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };
        if ctx.body.position().distance_squared(self.start_position)
            >= self.max_charge_distance * self.max_charge_distance
        {
            return false;
        }
        if attack_target
            .position()
            .distance_squared(ctx.body.position())
            >= self.max_target_detection_distance * self.max_target_detection_distance
        {
            return false;
        }

        let has_line_of_sight = ctx
            .body
            .as_living_entity()
            .is_some_and(|living| living.has_line_of_sight(attack_target.as_ref()));
        has_line_of_sight && !ctx.has_memory_value(&mem::CHARGE_COOLDOWN_TICKS)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.start_position = ctx.body.position();
        let Some(attack_target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return;
        };
        let direction = (attack_target.position() - ctx.body.position()).normalize_or_zero();
        self.charge_velocity_vector = direction * f64::from(self.speed);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(attack_target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return;
        };

        ctx.body.look_at_entity(
            EntityAnchor::Eyes,
            attack_target.as_ref(),
            EntityAnchor::Eyes,
        );
        ctx.body.set_velocity(self.charge_velocity_vector);

        let body_box = ctx.body.bounding_box();
        let body = ctx.body;
        let charge_targeting = &self.charge_targeting;
        let level = ctx.level;
        let colliding_entities = level.get_entities_in_aabb_matching(&body_box, |entity| {
            entity
                .as_living_entity()
                .is_some_and(|living| charge_targeting.test(level, body.as_living_entity(), living))
        });

        let Some(closest_attack_target) = colliding_entities.first().cloned() else {
            return;
        };
        if ctx.body.has_passenger(closest_attack_target.as_ref()) {
            return;
        }

        self.deal_damage_to_target(ctx, &closest_attack_target);
        self.deal_knockback(ctx, &closest_attack_target);
        self.stop(ctx);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory(&mem::CHARGE_COOLDOWN_TICKS, self.time_between_attacks);
        ctx.erase_memory(&mem::ATTACK_TARGET);
    }
}
