//! Shulker bullet projectile entity (`ShulkerBullet`).
//!
//! Mirrors vanilla `ShulkerBullet`: a physics-free homing projectile that steps
//! toward its target one axis at a time, dealing 4 mob-projectile damage and
//! 200 ticks of levitation. Without a live target it simply falls.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_entity_data::ShulkerBulletEntityData;
use steel_registry::{
    sound_events, vanilla_damage_types, vanilla_game_events, vanilla_mob_effects,
    vanilla_particle_types,
};
use steel_utils::UuidExt as _;
use steel_utils::locks::SyncMutex;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, Direction, DowncastType, DowncastTypeKey};
use uuid::Uuid;

use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityReference, EntitySyncedData, LivingEntity as _,
    MobEffectInstance, Projectile, ProjectileBase, ProjectileDeflection, ProjectileHit,
    RemovalReason, SharedEntity,
};
use crate::world::{ClipHitResult, LevelReader as _, World};

/// Vanilla `ShulkerBullet` homing speed.
const SPEED: f64 = 0.15;

/// Vanilla `ShulkerBullet` homing acceleration factor per tick.
const ACCELERATION: f64 = 0.025;

/// Vanilla `ShulkerBullet` velocity blend toward the target direction.
const BLEND: f64 = 0.2;

/// Vanilla `ShulkerBullet` impact damage.
const IMPACT_DAMAGE: f32 = 4.0;

/// Vanilla `ShulkerBullet` levitation duration (`new MobEffectInstance(LEVITATION, 200)`).
const LEVITATION_TICKS: i32 = 200;

/// Vanilla `ShulkerBullet.getDefaultGravity`.
const DEFAULT_GRAVITY: f64 = 0.04;

/// Vanilla `ShulkerBullet` first flight-step range (`10 + random.nextInt(5) * 10`).
const FLIGHT_STEPS_BASE: i32 = 10;
const FLIGHT_STEPS_RANGE: i32 = 5;
const FLIGHT_STEPS_SCALE: i32 = 10;

/// Vanilla `ShulkerBullet` homing state.
struct ShulkerBulletState {
    final_target: Option<EntityReference>,
    current_move_direction: Option<Direction>,
    flight_steps: i32,
    target_delta_x: f64,
    target_delta_y: f64,
    target_delta_z: f64,
}

/// Rotates `bullet` toward its movement vector (vanilla
/// `ProjectileUtil.rotateTowardsMovement`), interpolating with `rotation_speed`.
fn rotate_towards_movement<T: Projectile>(bullet: &T, rotation_speed: f32) {
    let movement = bullet.velocity();
    if movement.length_squared() == 0.0 {
        return;
    }
    let horizontal = (movement.x * movement.x + movement.z * movement.z).sqrt();
    let yaw = movement.x.atan2(movement.z).to_degrees() as f32;
    let pitch = movement.y.atan2(horizontal).to_degrees() as f32;
    let (yaw_old, pitch_old) = bullet.base().old_rotation();
    bullet.set_rotation((
        lerp_rotation(yaw_old, yaw, rotation_speed),
        lerp_rotation(pitch_old, pitch, rotation_speed),
    ));
}

/// A shulker bullet.
#[entity_behavior(class = "ShulkerBullet")]
pub struct ShulkerBulletEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced data layer declared by this entity (no fields of its own).
    entity_data: SyncMutex<ShulkerBulletEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Homing state (target, movement direction, deltas, remaining flight steps).
    state: SyncMutex<ShulkerBulletState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ShulkerBulletEntity`.
unsafe impl DowncastType for ShulkerBulletEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/shulker_bullet");
}

impl ShulkerBulletEntity {
    /// Creates a shulker bullet with no owner or target.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ShulkerBulletEntityData::new()),
            projectile_base: ProjectileBase::new(),
            state: SyncMutex::new(ShulkerBulletState {
                final_target: None,
                current_move_direction: None,
                flight_steps: 0,
                target_delta_x: 0.0,
                target_delta_y: 0.0,
                target_delta_z: 0.0,
            }),
        }
    }

    /// Creates a shulker bullet at `owner`'s bounding-box center, homing on
    /// `target` (vanilla `ShulkerBullet(Level, LivingEntity, Entity, Axis)`).
    #[must_use]
    pub fn with_target(
        entity_type: EntityTypeRef,
        id: i32,
        owner: &SharedEntity,
        target: &SharedEntity,
        invalid_start_axis: Option<steel_utils::axis::Axis>,
        world: Weak<World>,
    ) -> Self {
        let bounds = owner.bounding_box();
        let min = bounds.min_corner();
        let max = bounds.max_corner();
        let position = DVec3::new(
            (min.x + max.x) / 2.0,
            (min.y + max.y) / 2.0,
            (min.z + max.z) / 2.0,
        );
        let bullet = Self::new(entity_type, id, position, world);
        bullet.set_owner_entity(Some(owner));
        bullet.set_target_entity(Some(target));
        bullet.state.lock().current_move_direction = Some(Direction::Up);
        bullet.select_next_move_direction(invalid_start_axis, Some(target));
        bullet
    }

    /// Creates a shulker bullet from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(ShulkerBulletEntityData::new()),
            projectile_base: ProjectileBase::new(),
            state: SyncMutex::new(ShulkerBulletState {
                final_target: None,
                current_move_direction: None,
                flight_steps: 0,
                target_delta_x: 0.0,
                target_delta_y: 0.0,
                target_delta_z: 0.0,
            }),
        }
    }

    /// Returns the persisted target UUID (vanilla `ShulkerBullet.finalTarget`).
    #[must_use]
    pub fn target_uuid(&self) -> Option<Uuid> {
        self.state
            .lock()
            .final_target
            .as_ref()
            .map(EntityReference::uuid)
    }

    /// Sets the persisted target UUID without a live cache
    /// (vanilla `ShulkerBullet.setTarget` storing the target reference).
    pub fn set_target(&self, target: Option<Uuid>) {
        self.state.lock().final_target = target.map(EntityReference::from_uuid);
    }

    /// Sets the target from a live entity, caching it for immediate resolution.
    pub fn set_target_entity(&self, target: Option<&SharedEntity>) {
        self.state.lock().final_target = target.map(EntityReference::from_entity);
    }

    /// Resolves the live target entity, if it is still in this domain.
    fn live_target(&self) -> Option<SharedEntity> {
        let world = self.level()?;
        let reference = self.state.lock().final_target.clone()?;
        reference.get_entity(&world)
    }

    /// Vanilla `ShulkerBullet.selectNextMoveDirection`.
    fn select_next_move_direction(
        &self,
        avoid_axis: Option<steel_utils::axis::Axis>,
        target: Option<&SharedEntity>,
    ) {
        let Some(world) = self.level() else {
            return;
        };

        let (target_pos, y_offset) = match target {
            None => (self.block_position().below(), 0.5),
            Some(target) => {
                let y_offset = f64::from(target.base().dimensions().height) * 0.5;
                let position = target.position();
                (
                    BlockPos::containing(position.x, position.y + y_offset, position.z),
                    y_offset,
                )
            }
        };

        let mut target_x = f64::from(target_pos.x()) + 0.5;
        let mut target_y = f64::from(target_pos.y()) + y_offset;
        let mut target_z = f64::from(target_pos.z()) + 0.5;
        let mut selection = None;

        let own_position = self.position();
        let close_to_center = (own_position.x - (f64::from(target_pos.x()) + 0.5)).abs() < 2.0
            && (own_position.y - (f64::from(target_pos.y()) + 0.5)).abs() < 2.0
            && (own_position.z - (f64::from(target_pos.z()) + 0.5)).abs() < 2.0;

        if !close_to_center {
            let current = self.block_position();
            let mut options: Vec<Direction> = Vec::new();
            let is_empty = |pos: BlockPos| world.get_block_state(pos).is_air();

            if avoid_axis != Some(steel_utils::axis::Axis::X) {
                if current.x() < target_pos.x() && is_empty(current.east()) {
                    options.push(Direction::East);
                } else if current.x() > target_pos.x() && is_empty(current.west()) {
                    options.push(Direction::West);
                }
            }
            if avoid_axis != Some(steel_utils::axis::Axis::Y) {
                if current.y() < target_pos.y() && is_empty(current.above()) {
                    options.push(Direction::Up);
                } else if current.y() > target_pos.y() && is_empty(current.below()) {
                    options.push(Direction::Down);
                }
            }
            if avoid_axis != Some(steel_utils::axis::Axis::Z) {
                if current.z() < target_pos.z() && is_empty(current.south()) {
                    options.push(Direction::South);
                } else if current.z() > target_pos.z() && is_empty(current.north()) {
                    options.push(Direction::North);
                }
            }

            let mut candidate = Direction::random();
            if options.is_empty() {
                let mut attempts = 5;
                while !is_empty(candidate.relative(current)) && attempts > 0 {
                    candidate = Direction::random();
                    attempts -= 1;
                }
            } else {
                candidate = options[rand::random_range(0..options.len())];
            }
            selection = Some(candidate);

            let (dx, dy, dz) = candidate.offset();
            target_x = own_position.x + f64::from(dx);
            target_y = own_position.y + f64::from(dy);
            target_z = own_position.z + f64::from(dz);
        }

        let xa = target_x - own_position.x;
        let ya = target_y - own_position.y;
        let za = target_z - own_position.z;
        let distance = (xa * xa + ya * ya + za * za).sqrt();

        let mut state = self.state.lock();
        state.current_move_direction = selection;
        if distance == 0.0 {
            state.target_delta_x = 0.0;
            state.target_delta_y = 0.0;
            state.target_delta_z = 0.0;
        } else {
            state.target_delta_x = xa / distance * SPEED;
            state.target_delta_y = ya / distance * SPEED;
            state.target_delta_z = za / distance * SPEED;
        }
        state.flight_steps =
            FLIGHT_STEPS_BASE + rand::random_range(0..FLIGHT_STEPS_RANGE) * FLIGHT_STEPS_SCALE;
        drop(state);

        self.mark_velocity_sync();
    }

    /// Vanilla `ShulkerBullet.destroy`.
    fn destroy(&self) {
        self.set_removed(RemovalReason::Discarded);
        self.game_event(&vanilla_game_events::ENTITY_DAMAGE);
    }
}

impl Entity for ShulkerBulletEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        self.default_tick();

        let target = self.live_target();
        let target = target.filter(|target| {
            target.is_alive() && !(target.as_player().is_some() && target.is_spectator())
        });

        match &target {
            None => {
                self.state.lock().final_target = None;
                self.apply_gravity();
            }
            Some(target) => {
                {
                    let mut state = self.state.lock();
                    state.target_delta_x =
                        (state.target_delta_x * (1.0 + ACCELERATION)).clamp(-1.0, 1.0);
                    state.target_delta_y =
                        (state.target_delta_y * (1.0 + ACCELERATION)).clamp(-1.0, 1.0);
                    state.target_delta_z =
                        (state.target_delta_z * (1.0 + ACCELERATION)).clamp(-1.0, 1.0);
                }
                let (dx, dy, dz) = {
                    let state = self.state.lock();
                    (
                        state.target_delta_x,
                        state.target_delta_y,
                        state.target_delta_z,
                    )
                };
                let movement = self.velocity();
                self.set_velocity(
                    movement
                        + DVec3::new(
                            (dx - movement.x) * BLEND,
                            (dy - movement.y) * BLEND,
                            (dz - movement.z) * BLEND,
                        ),
                );
            }
        }

        let hit = self.get_hit_result_on_move_vector();
        let movement = self.velocity();
        if let Err(error) = self.try_set_position(self.position() + movement) {
            log::debug!("failed to advance shulker bullet {}: {error}", self.id());
            self.set_removed(RemovalReason::Discarded);
            return;
        }
        self.apply_effects_from_blocks();

        if let Some(result) = &hit
            && self.is_alive()
        {
            Arc::clone(&self).hit_target_or_deflect_self(result);
        }

        rotate_towards_movement(&*self, 0.5);

        // VANILLA CLIENT-LOCAL: the end-rod trail particle.

        let Some(target) = target else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        let mut reselect = false;
        let move_direction = self.state.lock().current_move_direction;
        {
            let mut state = self.state.lock();
            if state.flight_steps > 0 {
                state.flight_steps -= 1;
                if state.flight_steps == 0 {
                    reselect = true;
                }
            }
        }
        if reselect {
            let axis = move_direction.map(Direction::get_axis);
            self.select_next_move_direction(axis, Some(&target));
            return;
        }

        let Some(move_direction) = move_direction else {
            return;
        };
        let current = self.block_position();
        let axis = move_direction.get_axis();
        let next = move_direction.relative(current);
        if world.is_position_entity_ticking(next)
            && world.is_face_sturdy(world.get_block_state(next), next, Direction::Up)
        {
            self.select_next_move_direction(Some(axis), Some(&target));
            return;
        }

        let target_pos = target.block_position();
        let aligned = match axis {
            steel_utils::axis::Axis::X => current.x() == target_pos.x(),
            steel_utils::axis::Axis::Z => current.z() == target_pos.z(),
            steel_utils::axis::Axis::Y => current.y() == target_pos.y(),
        };
        if aligned {
            self.select_next_move_direction(Some(axis), Some(&target));
        }
    }

    fn check_despawn(&self) {
        // Vanilla `ShulkerBullet.checkDespawn` discards on peaceful.
        if let Some(world) = self.level()
            && world.difficulty() == Difficulty::Peaceful
        {
            self.set_removed(RemovalReason::Discarded);
        }
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Hostile
    }

    fn spawn_data(&self) -> i32 {
        self.get_owner().map_or(0, |owner| owner.id())
    }

    fn restore_owner_reference(&self, owner: &SharedEntity) {
        self.cache_owner_entity(owner);
    }

    fn projectile_owner_uuid(&self) -> Option<uuid::Uuid> {
        self.owner_uuid()
    }

    fn projectile_owner(&self) -> Option<SharedEntity> {
        self.get_owner()
    }

    fn no_physics(&self) -> bool {
        // Vanilla sets `noPhysics = true` in the constructor.
        true
    }

    fn is_affected_by_blocks(&self) -> bool {
        // Vanilla `ShulkerBullet.isAffectedByBlocks` tracks removal.
        !self.is_removed()
    }

    fn is_on_fire(&self) -> bool {
        // Vanilla `ShulkerBullet.isOnFire` never renders fire.
        false
    }

    fn is_pickable(&self) -> bool {
        true
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        let state = self.state.lock();
        if let Some(target) = &state.final_target {
            nbt.insert(
                "Target",
                simdnbt::owned::NbtTag::IntArray(target.uuid().to_int_array().to_vec()),
            );
        }
        if let Some(direction) = state.current_move_direction {
            nbt.insert("Dir", direction.get_3d_data_value() as i8);
        }
        nbt.insert("Steps", state.flight_steps);
        nbt.insert("TXD", state.target_delta_x);
        nbt.insert("TYD", state.target_delta_y);
        nbt.insert("TZD", state.target_delta_z);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        let mut state = self.state.lock();
        state.flight_steps = nbt.int("Steps").unwrap_or(0);
        state.target_delta_x = nbt.double("TXD").unwrap_or(0.0);
        state.target_delta_y = nbt.double("TYD").unwrap_or(0.0);
        state.target_delta_z = nbt.double("TZD").unwrap_or(0.0);
        state.current_move_direction = nbt
            .byte("Dir")
            .map(|value| Direction::from_3d_data_value(i32::from(value)));
        state.final_target = EntityReference::read(&nbt, "Target");
    }

    fn deflection(&self, _projectile: &dyn Projectile) -> ProjectileDeflection {
        // Vanilla `ShulkerBullet` is not in `DEFLECTS_PROJECTILES`; its redirect
        // behavior is implemented through `onRedirectProjectile`.
        ProjectileDeflection::None
    }
}

impl Projectile for ShulkerBulletEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        // Vanilla `ShulkerBullet.canHitEntity` adds the physics requirement.
        self.base_can_hit_entity(entity) && !entity.no_physics()
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, _location: DVec3) {
        // Vanilla `ShulkerBullet.onHitEntity`: 4 mob-projectile damage plus levitation.
        let owner = self.get_owner();
        let mut damage = DamageSource::environment(&vanilla_damage_types::MOB_PROJECTILE)
            .with_direct_entity(Arc::clone(&self) as SharedEntity);
        if let Some(owner) = &owner {
            damage = damage.with_causing_entity(Arc::clone(owner));
        }
        let Some(world) = entity.level() else {
            return;
        };
        if !entity.hurt(&world, &damage, IMPACT_DAMAGE) {
            return;
        }
        if let Some(living) = entity.as_living_entity() {
            living.add_mob_effect(MobEffectInstance::with_duration(
                &vanilla_mob_effects::LEVITATION,
                LEVITATION_TICKS,
                0,
            ));
        }
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        Arc::clone(&self).projectile_on_hit_block(hit);
        if let Some(world) = self.level() {
            world.send_particles(
                ParticleData::simple(&vanilla_particle_types::EXPLOSION),
                self.position(),
                2,
                DVec3::splat(0.2),
                0.0,
            );
            world.play_sound_at(
                &sound_events::ENTITY_SHULKER_BULLET_HIT,
                self.sound_source(),
                self.position(),
                1.0,
                1.0,
                None,
            );
        }
    }

    fn on_hit(self: Arc<Self>, hit: &ProjectileHit) {
        Arc::clone(&self).projectile_on_hit(hit);
        self.destroy();
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        // Vanilla `ShulkerBullet.hurtServer` always destroys the bullet.
        if let Some(world) = self.level() {
            world.play_sound_at(
                &sound_events::ENTITY_SHULKER_BULLET_HURT,
                self.sound_source(),
                self.position(),
                1.0,
                1.0,
                None,
            );
            world.send_particles(
                ParticleData::simple(&vanilla_particle_types::CRIT),
                self.position(),
                15,
                DVec3::splat(0.2),
                0.0,
            );
        }
        self.destroy();
        true
    }

    fn on_deflection(&self, _by_attack: bool) {
        // Vanilla `ShulkerBullet.onRedirectProjectile` is invoked by the redirected
        // projectile; the bullet itself keeps its own homing state.
    }
}

/// Vanilla `Mth.lerp(speed, old, new)` angle step used by rotation helpers.
fn lerp_rotation(old: f32, new: f32, speed: f32) -> f32 {
    let mut old = old;
    while new - old < -180.0 {
        old -= 360.0;
    }
    while new - old >= 180.0 {
        old += 360.0;
    }
    old + speed * (new - old)
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::world::World;

    use super::ShulkerBulletEntity;

    #[test]
    fn target_uuid_round_trips() {
        init_vanilla_registry();

        let bullet = ShulkerBulletEntity::new(
            &vanilla_entities::SHULKER_BULLET,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        let uuid = uuid::Uuid::from_u128(0x1234_5678_9abc_def0_1234_5678_9abc_def0);

        bullet.set_target(Some(uuid));
        assert_eq!(bullet.target_uuid(), Some(uuid));

        bullet.set_target(None);
        assert_eq!(bullet.target_uuid(), None);
    }
}
