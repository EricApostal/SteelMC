//! Vanilla `Squid` (`net.minecraft.world.entity.animal.squid.Squid`).
//!
//! Vanilla extends `AgeableWaterCreature`/`WaterAnimal`; Steel has no water-creature base, so
//! the squid is an [`AgeableMob`] + [`PathfinderMob`] whose water behaviour is implemented here
//! (buoyancy drag, tentacle cycle, ink defense). Vanilla `Squid` does not override
//! `createNavigation`, so it keeps `GroundPathNavigation`; only the water pathfinding malus from
//! `AgeableWaterCreature` is configured here.

use std::f32::consts::{PI, TAU};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::particle_type::{ParticleData, ParticleTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SquidEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_blocks, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::Downcast as _;
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{Goal, GoalControls};
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::water::abstract_fish::check_water_animal_spawn_obstruction;
use crate::entity::{
    AgeableMob, AgeableMobBase, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity,
};

use crate::physics::{MoveResult, MoverType};
use crate::world::{LevelReader, World};

use super::glow_squid::GlowSquidEntity;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Squid.getDefaultGravity`.
const SQUID_GRAVITY: f64 = 0.08;
/// Vanilla `Squid.SquidFleeGoal.SQUID_FLEE_SPEED`.
const SQUID_FLEE_SPEED: f64 = 3.0;
/// Vanilla `Squid.SquidFleeGoal.SQUID_FLEE_MIN_DISTANCE`.
const SQUID_FLEE_MIN_DISTANCE: f64 = 5.0;
/// Vanilla `Squid.SquidFleeGoal` trigger range (squared).
const SQUID_FLEE_RANGE_SQR: f64 = 100.0;
/// Vanilla `Squid.SquidRandomMovementGoal` idle cutoff.
const SQUID_IDLE_NO_ACTION_TIME: i32 = 100;
/// Vanilla `Squid.aiStep` velocity decay while the tentacle cycle coasts in water.
const SQUID_WATER_DECAY: f64 = 0.9;
/// Vanilla `Squid.aiStep` rotation decay in air.
const SQUID_AIR_ROTATE_DECAY: f32 = 0.99;
/// Vanilla `Squid.aiStep` water rotation growth.
const SQUID_WATER_ROTATE_GROWTH: f32 = 0.8;
/// Vanilla `Squid` baby dimensions (`EntityDimensions.scalable(0.5, 0.5).withEyeHeight(0.37)`).
pub(super) const SQUID_BABY_WIDTH: f32 = 0.5;
pub(super) const SQUID_BABY_HEIGHT: f32 = 0.5;
pub(super) const SQUID_BABY_EYE_HEIGHT: f32 = 0.37;
/// Vanilla `Squid.spawnInk` particle count.
const INK_PARTICLE_COUNT: i32 = 30;

/// Vanilla `Squid`'s body state, shared by `Squid` and `GlowSquid`.
///
/// Vanilla stores these on `Squid` and `GlowSquid extends Squid`, so both run the same
/// tentacle cycle and jet movement; Steel keeps them in one struct that both entities own.
pub(super) struct SquidBody {
    /// Vanilla `Squid.movementVector`, written by both squid goals.
    movement_vector: SyncMutex<DVec3>,
    /// Vanilla `Squid.tentacleMovement`.
    tentacle_movement: SyncMutex<f32>,
    /// Vanilla `Squid.tentacleSpeed`.
    tentacle_speed: SyncMutex<f32>,
    /// Vanilla `Squid.tentacleAngle`.
    tentacle_angle: SyncMutex<f32>,
    /// Vanilla `Squid.rotateSpeed`.
    rotate_speed: SyncMutex<f32>,
    /// Vanilla `Squid.xBodyRot` (server-side pitch state).
    x_body_rot: SyncMutex<f32>,
    /// Vanilla `Squid.zBodyRot` (server-side roll state).
    z_body_rot: SyncMutex<f32>,
}

impl SquidBody {
    /// Vanilla `Squid` constructor field initialization, including
    /// `tentacleSpeed = 1 / (random.nextFloat() + 1) * 0.2`.
    pub(super) fn new() -> Self {
        Self {
            movement_vector: SyncMutex::new(DVec3::ZERO),
            tentacle_movement: SyncMutex::new(0.0),
            tentacle_speed: SyncMutex::new(1.0 / (rand::random::<f32>() + 1.0) * 0.2),
            tentacle_angle: SyncMutex::new(0.0),
            rotate_speed: SyncMutex::new(0.0),
            x_body_rot: SyncMutex::new(0.0),
            z_body_rot: SyncMutex::new(0.0),
        }
    }

    /// Vanilla `Squid.movementVector`.
    pub(super) fn movement_vector(&self) -> DVec3 {
        *self.movement_vector.lock()
    }

    /// Sets vanilla `Squid.movementVector`.
    pub(super) fn set_movement_vector(&self, movement: DVec3) {
        *self.movement_vector.lock() = movement;
    }

    /// Vanilla `Squid.hasMovementVector` (`lengthSqr() > 1.0E-5F`).
    pub(super) fn has_movement_vector(&self) -> bool {
        self.movement_vector().length_squared() > 1.0e-5
    }

    /// Vanilla `Squid.xBodyRot`.
    pub(super) fn x_body_rot(&self) -> f32 {
        *self.x_body_rot.lock()
    }

    /// Vanilla `Squid.zBodyRot`.
    pub(super) fn z_body_rot(&self) -> f32 {
        *self.z_body_rot.lock()
    }
}

/// Returns the shared body state of a squid-like entity, downcasting the concrete entity.
///
/// Vanilla hands both squid goals a `Squid`; Steel's goals only receive
/// `&dyn PathfinderMob`, so the state is resolved from the concrete `Squid`/`GlowSquid`.
pub(super) fn squid_body_of(entity: &dyn Entity) -> Option<&SquidBody> {
    let entity = entity.as_entity_event_source();
    if let Some(squid) = entity.downcast_ref::<SquidEntity>() {
        return Some(squid.body());
    }
    entity
        .downcast_ref::<GlowSquidEntity>()
        .map(GlowSquidEntity::body)
}

/// Vanilla `Squid.travel`: move along the raw velocity, with none of
/// `LivingEntity.travel`'s drag, gravity or input-driven movement. Both are applied by
/// [`tick_squid_body`] instead, which is what keeps the jet-and-glide cycle in step with
/// vanilla.
pub(super) fn travel(entity: &SharedEntity, velocity: DVec3) -> Option<MoveResult> {
    Arc::clone(entity).move_entity(MoverType::SelfMovement, velocity)
}

/// Vanilla `Squid.aiStep` body: tentacle cycle, next-tick velocity and body rotation.
///
/// Runs after this tick's `LivingEntity.aiStep`, so the squid moves with the velocity
/// produced by the previous body tick, exactly like vanilla.
pub(super) fn tick_squid_body(entity: &dyn LivingEntity, body: &SquidBody) {
    // Tentacle cycle.
    let should_broadcast = {
        let mut tentacle_movement = body.tentacle_movement.lock();
        *tentacle_movement += *body.tentacle_speed.lock();
        if *tentacle_movement > TAU {
            *tentacle_movement -= TAU;
            if rand::random_range(0..10) == 0 {
                *body.tentacle_speed.lock() = 1.0 / (rand::random::<f32>() + 1.0) * 0.2;
            }
            true
        } else {
            false
        }
    };
    if should_broadcast {
        entity.broadcast_entity_event(EntityStatus::SquidAnimSynch);
    }

    if entity.is_in_water() {
        let tentacle_movement = *body.tentacle_movement.lock();
        if tentacle_movement < PI {
            let tentacle_scale = tentacle_movement / PI;
            *body.tentacle_angle.lock() = (tentacle_scale * tentacle_scale * PI).sin() * PI * 0.25;
            if tentacle_scale > 0.75 {
                entity.set_velocity(body.movement_vector());
                *body.rotate_speed.lock() = 1.0;
            } else {
                *body.rotate_speed.lock() *= SQUID_WATER_ROTATE_GROWTH;
            }
        } else {
            *body.tentacle_angle.lock() = 0.0;
            let velocity = entity.velocity();
            entity.set_velocity(velocity * SQUID_WATER_DECAY);
            *body.rotate_speed.lock() *= SQUID_AIR_ROTATE_DECAY;
        }

        // Vanilla keeps the body rotation aligned with the movement vector.
        let velocity = entity.velocity();
        let horizontal = velocity.x.hypot(velocity.z);
        let target_yaw = -((velocity.x as f32).atan2(velocity.z as f32)) * (180.0 / PI);
        let body_rot = entity.y_body_rot();
        let new_body_rot = body_rot + (target_yaw - body_rot) * 0.1;
        entity.living_base().set_y_body_rot(new_body_rot);
        let (_, pitch) = entity.rotation();
        entity.set_rotation((new_body_rot, pitch));

        let rotate_speed = *body.rotate_speed.lock();
        *body.z_body_rot.lock() += PI * rotate_speed * 1.5;

        let target_pitch = -((horizontal as f32).atan2(velocity.y as f32)) * (180.0 / PI);
        let x_body_rot = *body.x_body_rot.lock();
        *body.x_body_rot.lock() = x_body_rot + (target_pitch - x_body_rot) * 0.1;
    } else {
        *body.tentacle_angle.lock() = body.tentacle_movement.lock().sin().abs() * PI * 0.25;

        let velocity = entity.velocity();
        let mut yd = velocity.y;
        if let Some(levitation) = entity.mob_effect(vanilla_mob_effects::LEVITATION) {
            yd = 0.05 * f64::from(levitation.amplifier() + 1);
        } else {
            yd -= SQUID_GRAVITY;
        }
        entity.set_velocity(DVec3::new(0.0, yd * entity.get_air_drag(), 0.0));

        let x_body_rot = *body.x_body_rot.lock();
        *body.x_body_rot.lock() = x_body_rot + (-90.0 - x_body_rot) * 0.02;
    }
}

/// Vanilla `Squid`.
#[entity_behavior(class = "Squid")]
pub struct SquidEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    entity_data: SyncMutex<SquidEntityData>,
    body: SquidBody,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SquidEntity`.
unsafe impl DowncastType for SquidEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/squid");
}

impl SquidEntity {
    /// Creates a new squid entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved squid entity.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        let ageable_base = AgeableMobBase::new();
        // Vanilla `AgeableWaterCreature`: `setPathfindingMalus(WATER, 0.0F)`.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, 0.0);
        let mut entity_data = SquidEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `Squid.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, SquidRandomMovementGoal::new());
            goal_selector.add_goal(1, SquidFleeGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            entity_data: SyncMutex::new(entity_data),
            body: SquidBody::new(),
        }
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }

        let display = self.living_base.mob_effect_display_state();

        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }

        self.entity_data.set_base_invisible_flag(display.invisible);
    }

    /// Vanilla `Squid`'s body state (tentacle cycle, jet velocity, body rotation).
    #[must_use]
    pub(super) const fn body(&self) -> &SquidBody {
        &self.body
    }

    /// Vanilla `Squid.movementVector`.
    #[must_use]
    pub fn movement_vector(&self) -> DVec3 {
        self.body.movement_vector()
    }

    /// Sets vanilla `Squid.movementVector`.
    pub fn set_movement_vector(&self, movement: DVec3) {
        self.body.set_movement_vector(movement);
    }

    /// Vanilla `Squid.hasMovementVector` (`lengthSqr() > 1.0E-5F`).
    #[must_use]
    pub fn has_movement_vector(&self) -> bool {
        self.body.has_movement_vector()
    }

    /// Vanilla `Squid.xBodyRot` (server-side pitch state).
    #[must_use]
    pub fn x_body_rot(&self) -> f32 {
        self.body.x_body_rot()
    }

    /// Vanilla `Squid.zBodyRot` (server-side roll state).
    #[must_use]
    pub fn z_body_rot(&self) -> f32 {
        self.body.z_body_rot()
    }

    /// Vanilla `Squid.getSquirtSound`.
    #[must_use]
    pub fn squirt_sound() -> SoundEventRef {
        &sound_events::ENTITY_SQUID_SQUIRT
    }

    /// Vanilla `Squid.getInkParticle`.
    #[must_use]
    pub fn ink_particle() -> ParticleTypeRef {
        &vanilla_particle_types::SQUID_INK
    }

    /// Vanilla `Squid.spawnInk`: play the squirt sound and spray ink behind the squid.
    fn spawn_ink(&self) {
        self.play_sound(Self::squirt_sound(), self.sound_volume(), 1.0);
        let Some(world) = self.level() else {
            return;
        };

        let ink_scale = if AgeableMob::is_baby(self) { 0.1 } else { 0.3 };
        for _ in 0..INK_PARTICLE_COUNT {
            let dir = DVec3::new(
                f64::from(rand::random::<f32>() * 0.6 - 0.3),
                -1.0,
                f64::from(rand::random::<f32>() * 0.6 - 0.3),
            );
            let offset = dir * (ink_scale + f64::from(rand::random::<f32>()) * 2.0);
            world.send_particles(
                ParticleData::simple(Self::ink_particle()),
                self.position() + DVec3::new(0.0, 0.5, 0.0),
                0,
                offset,
                0.1,
            );
        }
    }

    /// Vanilla `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules`, the
    /// `SpawnPlacements` predicate for `EntityType.SQUID`.
    ///
    /// Vanilla ignores `entity_type` and `random`; the position must be within 13
    /// blocks below sea level, the block below must be water and the block above
    /// must be a full water block.
    #[must_use]
    pub fn check_surface_ageable_water_creature_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut RandomSource,
    ) -> bool {
        let min_spawn_level = level.sea_level() - 13;
        pos.y() >= min_spawn_level
            && pos.y() <= level.sea_level()
            && level
                .get_block_state(pos.below())
                .get_fluid_state()
                .is_water()
            && level.get_block_state(pos.above()).get_block() == &vanilla_blocks::WATER
    }
}

/// Vanilla `Squid.SquidRandomMovementGoal`.
pub(super) struct SquidRandomMovementGoal;

impl SquidRandomMovementGoal {
    pub(super) const fn new() -> Self {
        Self
    }

    /// Vanilla's goal holds a `Squid`; the shared body resolves the state for both the
    /// squid and the glow squid.
    fn body(mob: &dyn PathfinderMob) -> Option<&SquidBody> {
        squid_body_of(mob.as_entity_event_source())
    }
}

impl Goal for SquidRandomMovementGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(body) = Self::body(mob) else {
            return;
        };

        if mob.no_action_time() > SQUID_IDLE_NO_ACTION_TIME {
            body.set_movement_vector(DVec3::ZERO);
            return;
        }

        // Vanilla `reducedTickDelay(50)`.
        if rand::random_range(0..(50 + 1) / 2) == 0
            || !mob.is_in_water()
            || !body.has_movement_vector()
        {
            let angle = rand::random::<f32>() * TAU;
            body.set_movement_vector(DVec3::new(
                f64::from(angle.cos()) * 0.2,
                -0.1 + f64::from(rand::random::<f32>()) * 0.2,
                f64::from(angle.sin()) * 0.2,
            ));
        }
    }
}

/// Vanilla `Squid.SquidFleeGoal`.
pub(super) struct SquidFleeGoal {
    flee_ticks: i32,
}

impl SquidFleeGoal {
    pub(super) const fn new() -> Self {
        Self { flee_ticks: 0 }
    }

    /// Vanilla's goal holds a `Squid`; the shared body resolves the state for both the
    /// squid and the glow squid.
    fn body(mob: &dyn PathfinderMob) -> Option<&SquidBody> {
        squid_body_of(mob.as_entity_event_source())
    }
}

impl Goal for SquidFleeGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if Self::body(mob).is_none() {
            return false;
        }
        mob.is_in_water()
            && mob.last_hurt_by_mob().is_some_and(|attacker| {
                mob.position().distance_squared(attacker.position()) < SQUID_FLEE_RANGE_SQR
            })
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.flee_ticks = 0;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(body) = Self::body(mob) else {
            return;
        };
        self.flee_ticks += 1;

        let Some(attacker) = mob.last_hurt_by_mob() else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };

        let mut flee_to = mob.position() - attacker.position();
        let probe = BlockPos::new(
            steel_math::fast_floor(mob.position().x + flee_to.x),
            steel_math::fast_floor(mob.position().y + flee_to.y),
            steel_math::fast_floor(mob.position().z + flee_to.z),
        );
        let state = world.get_block_state(probe);
        let fluid = state.get_fluid_state();
        if fluid.is_water() || state.is_air() {
            let length = flee_to.length();
            if length > 0.0 {
                flee_to = flee_to.normalize();
                let mut avoid_speed = SQUID_FLEE_SPEED;
                if length > SQUID_FLEE_MIN_DISTANCE {
                    avoid_speed -= (length - SQUID_FLEE_MIN_DISTANCE) / SQUID_FLEE_MIN_DISTANCE;
                }
                if avoid_speed > 0.0 {
                    flee_to *= avoid_speed;
                }
            }
            if state.is_air() {
                flee_to = DVec3::new(flee_to.x, 0.0, flee_to.z);
            }
            body.set_movement_vector(flee_to / 20.0);
        }

        if self.flee_ticks % 10 == 5 {
            world.send_particles(
                ParticleData::simple(&vanilla_particle_types::BUBBLE),
                mob.position(),
                1,
                DVec3::ZERO,
                0.0,
            );
        }
    }
}

impl Entity for SquidEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            EntityDimensions::new(SQUID_BABY_WIDTH, SQUID_BABY_HEIGHT, SQUID_BABY_EYE_HEIGHT)
                .scale(scale)
        } else if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        (
            &sound_events::ENTITY_SQUID_SQUIRT,
            &sound_events::ENTITY_SQUID_SQUIRT,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_SQUID_SQUIRT
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
    }
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let hurt = LivingEntity::hurt_server(self, world, source, amount);
        if hurt && self.last_hurt_by_mob().is_some() {
            self.spawn_ink();
        }
        hurt
    }
}

impl LivingEntity for SquidEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SQUID_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SQUID_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    /// Vanilla `AgeableWaterCreature.getBaseExperienceReward`: `1 + random.nextInt(3)`,
    /// with no equipment bonus.
    fn base_experience_reward(&self) -> i32 {
        1 + rand::random_range(0..3)
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        // Vanilla `Squid.aiStep`: `super.aiStep()` (which travels with the velocity from
        // the previous tick) runs first, then the squid body updates the tentacle cycle,
        // the next tick's velocity and the body rotation.
        let result = self.mob_ai_step(entity);
        tick_squid_body(self, &self.body);
        result
    }

    /// Vanilla `Squid.travel`: the squid moves along its raw velocity, with none of
    /// `LivingEntity.travel`'s drag, gravity or input-driven movement. Both of those are
    /// applied by [`Self::ai_step_squid`] instead, which is what keeps the jet-and-glide
    /// cycle in step with vanilla.
    fn travel(&self, entity: &SharedEntity, _input: DVec3) -> Option<MoveResult> {
        travel(entity, self.velocity())
    }
}

impl Mob for SquidEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    /// Vanilla `AgeableWaterCreature.checkSpawnObstruction`: `level.isUnobstructed(this)`
    /// only tests other blocking entities, unlike `Mob`'s liquid-inclusive default.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        check_water_animal_spawn_obstruction(world, self)
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SQUID_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Squid.canBeLeashed` is `true`.
    fn mob_can_be_leashed(&self) -> bool {
        true
    }
}

impl AgeableMob for SquidEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        &self.ageable_base
    }

    fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().ageable_mob().age_locked.get()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.entity_data
            .lock()
            .ageable_mob_mut()
            .age_locked
            .set(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.entity_data.lock().ageable_mob_mut().baby.set(baby);
    }
}

impl PathfinderMob for SquidEntity {
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        // Vanilla water creatures favor water.
        if let Some(world) = self.level()
            && world.get_block_state(pos).get_fluid_state().is_water()
        {
            return 10.0;
        }
        // `PathfinderMob::get_walk_target_value` cannot be reached from an override (a UFCS
        // call on `self` re-enters this method), so its default body is inlined verbatim: it
        // dispatches to the monster/animal helpers, falling back to `0.0` (the vanilla
        // `PathfinderMob.getWalkTargetValue`) for a creature that is neither.
        if let Some(monster) = self.as_monster() {
            monster.get_walk_target_value_monster(pos)
        } else if let Some(animal) = self.as_animal() {
            animal.animal_walk_target_value(pos)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::SquidEntity;
    use crate::entity::{AgeableMob, Entity, PathfinderMob};

    #[test]
    fn squid_starts_without_a_movement_vector() {
        init_vanilla_registry();
        let squid = SquidEntity::new(&vanilla_entities::SQUID, 1, DVec3::ZERO, Weak::new());
        assert!(!squid.has_movement_vector());
        assert_eq!(squid.entity_type(), &vanilla_entities::SQUID);
        assert!(!AgeableMob::is_baby(&squid));
    }

    #[test]
    fn squid_walk_target_falls_back_to_the_pathfinder_default_without_recursing() {
        init_vanilla_registry();
        let squid = SquidEntity::new(&vanilla_entities::SQUID, 1, DVec3::ZERO, Weak::new());
        // `PathfinderMob::get_walk_target_value` — `0.0` for a creature that is neither a
        // monster nor an animal — must be reached without re-entering this override.
        assert_eq!(
            squid.get_walk_target_value(steel_utils::BlockPos::ZERO),
            0.0
        );
    }
}
