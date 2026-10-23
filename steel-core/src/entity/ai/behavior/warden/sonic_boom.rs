//! Vanilla `SonicBoom`: charge, then fire a ranged sonic attack at the attack
//! target.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::particle_type::ParticleData;
use steel_registry::{sound_events, vanilla_damage_types, vanilla_particle_types};

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};
use crate::entity::ai::control::{DEFAULT_LOOK_X_MAX_ROT_ANGLE, DEFAULT_LOOK_Y_MAX_ROT_SPEED};
use crate::entity::damage::DamageSource;
use crate::entity::{LivingEntity, SharedEntity};

/// Vanilla `SonicBoom.DISTANCE_XZ`.
const DISTANCE_XZ: f64 = 15.0;

/// Vanilla `SonicBoom.DISTANCE_Y`.
const DISTANCE_Y: f64 = 20.0;

/// Vanilla `SonicBoom.KNOCKBACK_VERTICAL`.
const KNOCKBACK_VERTICAL: f64 = 0.5;

/// Vanilla `SonicBoom.KNOCKBACK_HORIZONTAL`.
const KNOCKBACK_HORIZONTAL: f64 = 2.5;

/// Vanilla `SonicBoom.COOLDOWN`.
pub const COOLDOWN: i64 = 40;

/// Vanilla `SonicBoom.TICKS_BEFORE_PLAYING_SOUND`, `Mth.ceil(34.0)`.
const TICKS_BEFORE_PLAYING_SOUND: i64 = 34;

/// Vanilla `SonicBoom.DURATION`, `Mth.ceil(60.0F)`.
const DURATION: i32 = 60;

/// Vanilla `SonicBoom`: cool the attack, wait out the sound delay, then damage
/// and knock back the attack target along the chest-to-eye line.
pub struct SonicBoom {
    state: BehaviorState,
}

impl SonicBoom {
    /// Vanilla `new SonicBoom()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::ATTACK_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                    (
                        &mem::SONIC_BOOM_COOLDOWN as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::SONIC_BOOM_SOUND_COOLDOWN as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                    (
                        &mem::SONIC_BOOM_SOUND_DELAY as MemoryModuleTypeRef,
                        MemoryStatus::Registered,
                    ),
                ],
                DURATION,
                DURATION,
            ),
        }
    }

    /// Vanilla `SonicBoom.setCooldown`.
    pub fn set_cooldown(ctx: &mut BehaviorContext<'_>, cooldown: i64) {
        ctx.set_memory_with_expiry(&mem::SONIC_BOOM_COOLDOWN, (), cooldown);
    }

    /// The `ifPresent` body of vanilla `tick`: validate the remembered target,
    /// then hurt and knock it back.
    fn fire(ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return;
        };

        if !super::can_target_entity(ctx, &target)
            || !super::closer_than_with_y(ctx.body, &target, DISTANCE_XZ, DISTANCE_Y)
        {
            return;
        }

        // Vanilla `target.getEyePosition().subtract(source)`, where `source`
        // is the body's position plus its `WARDEN_CHEST` attachment offset.
        let source = super::eye_position(ctx.body) + super::warden_chest_attachment(ctx);
        let delta = super::eye_position(&target) - source;
        // Vanilla `Vec3.normalize()` yields `Vec3.ZERO` for a degenerate delta.
        let direction = delta.normalize_or_zero();

        // Vanilla sends one `SONIC_BOOM` particle per block of the chest-to-eye line.
        let steps = delta.length().floor() as i32 + 7;
        for step in 1..steps {
            let particle_pos = source + direction * f64::from(step);
            ctx.level.send_particles(
                ParticleData::simple(&vanilla_particle_types::SONIC_BOOM),
                particle_pos,
                1,
                DVec3::ZERO,
                0.0,
            );
        }

        ctx.body
            .play_sound(&sound_events::ENTITY_WARDEN_SONIC_BOOM, 3.0, 1.0);

        // Vanilla `level.damageSources().sonicBoom(body)`.
        let damage_source =
            DamageSource::direct(&vanilla_damage_types::SONIC_BOOM, Arc::clone(ctx.body));

        if target.hurt(ctx.level, &damage_source, 10.0) {
            knock_back(&target, direction);
        }
    }
}

impl Default for SonicBoom {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for SonicBoom {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.get_memory(&mem::ATTACK_TARGET).is_some_and(|target| {
            super::closer_than_with_y(ctx.body, &target, DISTANCE_XZ, DISTANCE_Y)
        })
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory_with_expiry(&mem::ATTACK_COOLING_DOWN, true, i64::from(DURATION));
        ctx.set_memory_with_expiry(&mem::SONIC_BOOM_SOUND_DELAY, (), TICKS_BEFORE_PLAYING_SOUND);
        // Vanilla plays `SoundEvents.WARDEN_SONIC_CHARGE` here.
        ctx.body
            .play_sound(&sound_events::ENTITY_WARDEN_SONIC_CHARGE, 3.0, 1.0);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) {
            look_at(ctx, &target);
        }

        if ctx.has_memory_value(&mem::SONIC_BOOM_SOUND_DELAY)
            || ctx.has_memory_value(&mem::SONIC_BOOM_SOUND_COOLDOWN)
        {
            return;
        }

        // Vanilla plays `SoundEvents.WARDEN_SONIC_BOOM` inside `fire`.
        ctx.set_memory_with_expiry(
            &mem::SONIC_BOOM_SOUND_COOLDOWN,
            (),
            i64::from(DURATION) - TICKS_BEFORE_PLAYING_SOUND,
        );
        Self::fire(ctx);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        Self::set_cooldown(ctx, COOLDOWN);
    }
}

/// Vanilla `body.getLookControl().setLookAt(target.position())`, reached through
/// the mob controls the same way `LookAtTargetSink` does.
fn look_at(ctx: &mut BehaviorContext<'_>, target: &SharedEntity) {
    let Some(mob) = ctx.body.as_mob() else {
        return;
    };

    mob.mob_base().controls().lock().look_control.set_look_at(
        target.position(),
        DEFAULT_LOOK_Y_MAX_ROT_SPEED,
        DEFAULT_LOOK_X_MAX_ROT_ANGLE,
    );
}

/// Vanilla `target.push(delta.x * knockbackHorizontal, delta.y *
/// knockbackVertical, delta.z * knockbackHorizontal)`, where `delta` is the
/// normalized source-to-eye direction.
///
/// Both axes are scaled by `1.0 - KNOCKBACK_RESISTANCE`. The generic
/// `LivingEntity::knockback` only takes a horizontal impulse, so the vertical
/// component vanilla applies here is kept by pushing the combined impulse.
fn knock_back(target: &SharedEntity, direction: DVec3) {
    let Some(living) = target.as_living_entity() else {
        return;
    };

    let resistance = 1.0 - LivingEntity::knockback_resistance(living);
    target.push_impulse(DVec3::new(
        direction.x * KNOCKBACK_HORIZONTAL * resistance,
        direction.y * KNOCKBACK_VERTICAL * resistance,
        direction.z * KNOCKBACK_HORIZONTAL * resistance,
    ));
}
