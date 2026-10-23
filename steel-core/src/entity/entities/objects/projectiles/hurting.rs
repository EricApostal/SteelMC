//! Shared behavior for the vanilla `AbstractHurtingProjectile` family.
//!
//! Mirrors vanilla `AbstractHurtingProjectile`: a `Projectile` whose velocity is
//! accelerated along its own direction each tick (`accelerationPower`) and then
//! scaled by an inertia multiplier. Unlike `ThrowableProjectile` these projectiles
//! have no gravity of their own.
//!
//! The explosion bridge at the bottom of this file is a contained substitute for
//! vanilla `ServerLevel.explode`/`Level.explode`. It is the single place the
//! hurting projectiles deliver an explosion; see [`explode`] for the exact
//! arguments each projectile passes.

use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{vanilla_damage_types, vanilla_game_events, vanilla_particle_types};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, WorldAabb, axis::Axis};

use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityReference, Projectile, ProjectileDeflection, RemovalReason, SharedEntity,
};
use crate::world::game_event::GameEventContext;
use crate::world::{ClipBlockShape, ClipFluid, World};

/// Vanilla `AbstractHurtingProjectile.INITAL_ACCELERATION_POWER`.
pub const INITIAL_ACCELERATION_POWER: f64 = 0.1;

/// Vanilla `AbstractHurtingProjectile.DEFLECTION_SCALE`.
const DEFLECTION_SCALE: f64 = 0.5;

/// Vanilla `AbstractHurtingProjectile.getInertia` (air).
const AIR_INERTIA: f32 = 0.95;

/// Vanilla `AbstractHurtingProjectile.getLiquidInertia`.
const LIQUID_INERTIA: f32 = 0.8;

/// `Entity.setSecondsOnFire(1.0F)` in 20-tick seconds, applied by `shouldBurn`.
const BURN_TICKS: i32 = 20;

/// Vanilla `AbstractHurtingProjectile.accelerationPower`, stored per instance.
pub struct HurtingProjectileState {
    acceleration_power: SyncMutex<f64>,
}

impl HurtingProjectileState {
    /// Creates state with the vanilla default acceleration power.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            acceleration_power: SyncMutex::new(INITIAL_ACCELERATION_POWER),
        }
    }

    /// Creates state with an explicit acceleration power (wind charges use 0.0).
    #[must_use]
    pub const fn with_acceleration_power(acceleration_power: f64) -> Self {
        Self {
            acceleration_power: SyncMutex::new(acceleration_power),
        }
    }

    /// Returns the current acceleration power.
    #[must_use]
    pub fn acceleration_power(&self) -> f64 {
        *self.acceleration_power.lock()
    }

    /// Replaces the current acceleration power.
    pub fn set_acceleration_power(&self, acceleration_power: f64) {
        *self.acceleration_power.lock() = acceleration_power;
    }
}

impl Default for HurtingProjectileState {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla-shaped behavior shared by entities that extend `AbstractHurtingProjectile`.
pub trait HurtingProjectile: Projectile {
    /// Returns this projectile's accelerated-movement state.
    fn hurting_state(&self) -> &HurtingProjectileState;

    /// Returns the per-tick velocity acceleration along the direction of travel.
    fn acceleration_power(&self) -> f64 {
        self.hurting_state().acceleration_power()
    }

    /// Replaces the per-tick velocity acceleration.
    fn set_acceleration_power(&self, acceleration_power: f64) {
        self.hurting_state()
            .set_acceleration_power(acceleration_power);
    }

    /// Vanilla `AbstractHurtingProjectile.getInertia`.
    fn get_inertia(&self) -> f32 {
        AIR_INERTIA
    }

    /// Vanilla `AbstractHurtingProjectile.getLiquidInertia`.
    fn get_liquid_inertia(&self) -> f32 {
        LIQUID_INERTIA
    }

    /// Vanilla `AbstractHurtingProjectile.shouldBurn`.
    fn should_burn(&self) -> bool {
        true
    }

    /// Vanilla `AbstractHurtingProjectile.getTrailParticle`.
    fn get_trail_particle(&self) -> Option<ParticleData> {
        Some(ParticleData::simple(&vanilla_particle_types::SMOKE))
    }

    /// Vanilla `AbstractHurtingProjectile.getClipType`.
    fn get_clip_type(&self) -> ClipBlockShape {
        ClipBlockShape::Collider
    }

    /// Vanilla `AbstractHurtingProjectile.canHitEntity`: the base projectile check
    /// plus "the entity must be affected by physics".
    fn hurting_can_hit_entity(&self, entity: &dyn Entity) -> bool {
        self.base_can_hit_entity(entity) && !entity.no_physics()
    }

    /// Vanilla `AbstractHurtingProjectile.assignDirectionalMovement`.
    fn assign_directional_movement(&self, direction: DVec3, speed: f64) {
        self.set_velocity(direction.normalize_or_zero() * speed);
        self.mark_velocity_sync();
    }

    /// Vanilla `AbstractHurtingProjectile.onDeflection`.
    fn hurting_on_deflection(&self, by_attack: bool) {
        if by_attack {
            self.set_acceleration_power(INITIAL_ACCELERATION_POWER);
        } else {
            self.set_acceleration_power(self.acceleration_power() * DEFLECTION_SCALE);
        }
    }

    /// Vanilla `AbstractHurtingProjectile.applyInertia`.
    fn apply_hurting_inertia(&self) {
        // VANILLA CLIENT-LOCAL: `applyInertia` creates the trailing water bubbles.
        let inertia = if self.is_in_water() {
            f64::from(self.get_liquid_inertia())
        } else {
            f64::from(self.get_inertia())
        };
        let movement = self.velocity();
        self.set_velocity(
            (movement + movement.normalize_or_zero() * self.acceleration_power()) * inertia,
        );
    }

    /// Vanilla `AbstractHurtingProjectile.tick`: accelerate, move, resolve the
    /// move-vector hit, then burn and create the trail.
    ///
    /// Called by concrete `tick` implementations as the vanilla `super.tick()`.
    fn hurting_projectile_tick(self: Arc<Self>) {
        // Vanilla `Entity.setOldPosAndRot()` runs before the tick; capture it here
        // so `updateRotation` lerps from the pre-move rotation.
        self.set_old_position_to_current();
        self.base().set_old_rotation_to_current();

        self.apply_hurting_inertia();

        let Some(world) = self.level() else {
            return;
        };
        let owner_removed = self.get_owner().is_some_and(|owner| owner.is_removed());
        if owner_removed || !world.is_position_entity_ticking(self.block_position()) {
            self.set_removed(RemovalReason::Discarded);
            return;
        }

        let hit = self.get_hit_result_on_move_vector();
        let new_position = match &hit {
            Some(result) => result.location(),
            None => self.position() + self.velocity(),
        };

        if let Err(error) = self.try_set_position(new_position) {
            log::debug!(
                "failed to advance hurting projectile {}: {error}",
                self.id()
            );
            self.set_removed(RemovalReason::Discarded);
            return;
        }

        self.update_rotation();
        self.apply_effects_from_blocks();
        self.projectile_base_tick();

        if self.should_burn() {
            self.ignite_for_ticks(BURN_TICKS);
        }

        if let Some(result) = hit
            && self.is_alive()
            && !self.is_world_change_pending()
        {
            Arc::clone(&self).hit_target_or_deflect_self(&result);
        }

        // VANILLA CLIENT-LOCAL: `createParticleTrail` uses `Level.addParticle`.
        let _ = self.get_trail_particle();
    }

    /// Saves vanilla `AbstractHurtingProjectile` fields.
    fn save_hurting_projectile(&self, nbt: &mut NbtCompound) {
        nbt.insert("acceleration_power", self.acceleration_power());
    }

    /// Loads vanilla `AbstractHurtingProjectile` fields.
    fn load_hurting_projectile(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.set_acceleration_power(
            nbt.double("acceleration_power")
                .unwrap_or(INITIAL_ACCELERATION_POWER),
        );
    }
}

/// Vanilla `Projectile.deflect`'s default body, mirrored for overrides that must
/// gate deflection without recursing into their own `deflect` implementation.
///
/// Used by `WindCharge`, whose first five ticks refuse deflection. Remove once
/// `Projectile` exposes the default body (e.g. `Projectile::base_deflect`) or a
/// public `ProjectileDeflection::apply`.
pub(crate) fn base_deflect(
    projectile: &dyn Projectile,
    deflection: ProjectileDeflection,
    deflecting_entity: Option<&dyn Entity>,
    new_owner: Option<EntityReference>,
    by_attack: bool,
) -> bool {
    match deflection {
        ProjectileDeflection::None => {}
        ProjectileDeflection::Reverse => {
            let rotation = rand::random_range(170.0..190.0);
            projectile.set_velocity(projectile.velocity() * -0.5);
            let (yaw, pitch) = projectile.rotation();
            projectile.set_rotation((yaw + rotation, pitch));
            let (old_yaw, old_pitch) = projectile.base().old_rotation();
            projectile
                .base()
                .set_old_rotation((old_yaw + rotation, old_pitch));
            projectile.mark_velocity_sync();
        }
        ProjectileDeflection::AimDeflect => {
            let Some(entity) = deflecting_entity else {
                return false;
            };
            projectile.set_velocity(entity.look_angle());
            projectile.mark_velocity_sync();
        }
        ProjectileDeflection::MomentumDeflect => {
            let Some(entity) = deflecting_entity else {
                return false;
            };
            let movement = entity.velocity();
            let length = movement.length();
            let normalized = if length < 1.0e-5 {
                DVec3::ZERO
            } else {
                movement / length
            };
            projectile.set_velocity(normalized);
            projectile.mark_velocity_sync();
        }
    }

    projectile.set_owner_uuid(new_owner.map(|reference| reference.uuid()));
    projectile.on_deflection(by_attack);
    true
}

/// Runs vanilla `EnchantmentHelper.doPostAttackEffects(world, target, source)`.
///
/// Only the signature shape matters here; the real per-item effect application
/// belongs to the enchantment lane. This placeholder must be replaced when that
/// lane adds a supported entry point (see `Mob::hurt`'s current inline use of
/// `EnchantmentPostAttackContext`).
pub fn do_post_attack_effects(world: &World, target: &SharedEntity, source: &DamageSource) {
    // Gated so the imports and parameters stay live until the enchantment lane
    // wires the real effect application.
    let _ = (world, target, source);
}

/// Vanilla `Level.ExplosionInteraction`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExplosionInteraction {
    /// Blocks and entities are unaffected.
    None,
    /// Blocks are destroyed per the block-explosion game rules.
    Block,
    /// Blocks are destroyed per the mob-griefing rule.
    Mob,
    /// Blocks are destroyed per the TNT-explosion game rules.
    Tnt,
    /// Blocks are not destroyed, but triggers (e.g. buttons) still fire.
    Trigger,
}

/// The arguments of `ServerLevel.explode(...)` that the hurting projectiles use.
///
/// Each field holds the exact value vanilla passes at the projectile's call site,
/// so the explosion lane can consume them once `World::explode` exists.
pub struct ExplosionSpec {
    /// Vanilla `source`: the exploding projectile, excluded from the damage sweep.
    pub source: SharedEntity,
    /// Vanilla `damageSource`; `None` derives the explosion damage source from
    /// `source` and its owner, like vanilla's `damageSources().explosion(...)`.
    pub damage_source: Option<DamageSource>,
    /// Vanilla `center`, already snapped by the caller where vanilla does so.
    pub center: DVec3,
    /// Vanilla `radius`.
    pub radius: f32,
    /// Vanilla `fire`: whether the explosion can place fire.
    pub fire: bool,
    /// Vanilla `interactionType`.
    pub interaction: ExplosionInteraction,
    /// Vanilla `SimpleExplosionDamageCalculator.shouldDamageEntity`.
    pub damages_entities: bool,
    /// Vanilla `SimpleExplosionDamageCalculator.getKnockbackMultiplier`; `None`
    /// means the calculator default of 1.0.
    pub knockback_multiplier: Option<f32>,
    /// Vanilla `smallExplosionParticles`.
    pub small_particles: ParticleData,
    /// Vanilla `largeExplosionParticles`.
    pub large_particles: ParticleData,
    /// Vanilla `explosionSound`.
    pub sound: SoundEventRef,
}

/// Delivers a projectile explosion.
///
/// TODO(explosions-lane): replace this contained substitute with `World::explode`.
/// The substitute performs the vanilla entity sweep (radius falloff damage through
/// `Entity::hurt` plus explosion knockback) but intentionally omits block
/// destruction, block drops, fire placement, and the `ClientboundExplodePacket`
/// particle/sound delivery, all of which belong to the explosion foundation.
/// Nothing outside this function needs to change when that lands.
pub fn explode(world: &Arc<World>, spec: &ExplosionSpec) {
    if spec.radius < 1.0e-5 {
        return;
    }

    world.game_event(
        &vanilla_game_events::EXPLODE,
        BlockPos::from(spec.center),
        &GameEventContext::new(Some(spec.source.as_ref()), None),
    );

    let double_radius = f64::from(spec.radius) * 2.0;
    let sweep = WorldAabb::new(
        (spec.center.x - double_radius - 1.0).floor(),
        (spec.center.y - double_radius - 1.0).floor(),
        (spec.center.z - double_radius - 1.0).floor(),
        (spec.center.x + double_radius + 1.0).floor(),
        (spec.center.y + double_radius + 1.0).floor(),
        (spec.center.z + double_radius + 1.0).floor(),
    );
    let damage_source = spec.damage_source.clone().unwrap_or_else(|| {
        let mut source = DamageSource::environment(&vanilla_damage_types::EXPLOSION)
            .with_direct_entity(Arc::clone(&spec.source));
        // Vanilla `Explosion.getIndirectSourceEntity`: a living source is its own
        // indirect source, a projectile contributes its living owner.
        let indirect = match spec.source.as_living_entity() {
            Some(_) => Some(Arc::clone(&spec.source)),
            None => spec.source.projectile_owner(),
        };
        if let Some(indirect) = indirect {
            source = source.with_causing_entity(indirect);
        }
        source
    });
    let knockback_multiplier = spec.knockback_multiplier.unwrap_or(1.0);

    for entity in world.get_entities_in_aabb(&sweep) {
        if Arc::ptr_eq(&entity, &spec.source) || entity.is_spectator() {
            continue;
        }

        let distance = entity.position().distance(spec.center) / double_radius;
        if distance > 1.0 {
            continue;
        }

        let position = entity.position();
        let mut direction = DVec3::new(position.x, entity.get_eye_y(), position.z) - spec.center;
        if direction.length_squared() < 1.0e-8 {
            direction = DVec3::Y;
        }
        let direction = direction.normalize();
        let exposure = if !spec.damages_entities && knockback_multiplier == 0.0 {
            0.0
        } else {
            seen_percent(world, spec.center, &entity)
        };

        if spec.damages_entities {
            // Vanilla also gates on `source.doTeamsAllowDamage(entity)`.
            entity.hurt(
                world,
                &damage_source,
                entity_damage_amount(spec.radius, distance, exposure),
            );
        }

        let knockback_power =
            (1.0 - distance) * f64::from(exposure) * f64::from(knockback_multiplier);
        entity.push_impulse(direction * knockback_power);
    }
}

/// Vanilla `ExplosionDamageCalculator.getEntityDamageAmount`.
fn entity_damage_amount(radius: f32, distance: f64, exposure: f32) -> f32 {
    let double_radius = f64::from(radius) * 2.0;
    let power = (1.0 - distance) * f64::from(exposure);
    ((power * power + power) / 2.0 * 7.0 * double_radius + 1.0) as f32
}

/// Vanilla `ServerExplosion.getSeenPercent`: the fraction of sampled points on the
/// entity's bounding box that have an unobstructed line to the explosion center.
fn seen_percent(world: &World, center: DVec3, entity: &SharedEntity) -> f32 {
    let bb = entity.bounding_box();
    let x_size = 1.0 / ((bb.max(Axis::X) - bb.min(Axis::X)) * 2.0 + 1.0);
    let y_size = 1.0 / ((bb.max(Axis::Y) - bb.min(Axis::Y)) * 2.0 + 1.0);
    let z_size = 1.0 / ((bb.max(Axis::Z) - bb.min(Axis::Z)) * 2.0 + 1.0);
    if x_size < 0.0 || y_size < 0.0 || z_size < 0.0 {
        return 0.0;
    }

    let x_offset = (1.0 - (1.0 / x_size).floor() * x_size) / 2.0;
    let z_offset = (1.0 - (1.0 / z_size).floor() * z_size) / 2.0;

    let mut hits = 0;
    let mut count = 0;
    let mut xx = 0.0;
    while xx <= 1.0 {
        let mut yy = 0.0;
        while yy <= 1.0 {
            let mut zz = 0.0;
            while zz <= 1.0 {
                let from = DVec3::new(
                    lerp(xx, bb.min(Axis::X), bb.max(Axis::X)) + x_offset,
                    lerp(yy, bb.min(Axis::Y), bb.max(Axis::Y)),
                    lerp(zz, bb.min(Axis::Z), bb.max(Axis::Z)) + z_offset,
                );
                if world
                    .clip(from, center, ClipBlockShape::Collider, ClipFluid::None)
                    .is_miss()
                {
                    hits += 1;
                }
                count += 1;
                zz += z_size;
            }
            yy += y_size;
        }
        xx += x_size;
    }

    hits as f32 / count as f32
}

const fn lerp(delta: f64, start: f64, end: f64) -> f64 {
    start + delta * (end - start)
}
