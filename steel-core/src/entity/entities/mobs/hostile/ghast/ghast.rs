//! Vanilla `Ghast` (`net.minecraft.world.entity.monster.Ghast`).
//!
//! Vanilla ghasts fly with `FlyingPathNavigation` and `Ghast.GhastMoveControl`; Steel has no
//! flying navigation, so [`GhastMoveControl`] is reproduced here as the entity's own
//! acceleration step (documented deviation: pathfinding is straight-line flight).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::GhastEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::RandomSource;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::BLOCK_BEHAVIORS;
use crate::entity::ai::control::MoveControlOperation;
use crate::entity::ai::goal::{Goal, GoalControls, NearestAttackableTargetGoal};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::LargeFireballEntity;
use crate::entity::projectile::spawn_projectile_using_shoot;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    next_entity_id,
};
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Ghast.getSoundVolume`.
const GHAST_SOUND_VOLUME: f32 = 5.0;
/// Vanilla `Ghast.checkGhastSpawnRules`: `random.nextInt(20) == 0`.
const GHAST_SPAWN_CHANCE_BOUND: i32 = 20;
/// Vanilla `Ghast` default `explosionPower`.
const DEFAULT_EXPLOSION_POWER: i32 = 1;
/// Vanilla `Ghast.GhastShootFireballGoal` sight/range gate (64 blocks squared).
const FIREBALL_MAX_DISTANCE_SQR: f64 = 4096.0;
/// Vanilla `GhastShootFireballGoal` charge ticks.
const CHARGE_SOUND_TICK: i32 = 10;
const FIREBALL_RELEASE_TICK: i32 = 20;
const FIREBALL_COOLDOWN_TICKS: i32 = -40;
/// Vanilla `GhastShootFireballGoal` muzzle offset.
const FIREBALL_MUZZLE_OFFSET: f64 = 4.0;
/// Vanilla `Ghast.GhastMoveControl` flight constants.
const MOVE_FLOAT_DURATION_BASE: i32 = 2;
const MOVE_FLOAT_DURATION_RANGE: i32 = 5;
const FLYING_SPEED_SCALE: f64 = 5.0 / 3.0;
/// Vanilla `Ghast.RandomFloatAroundGoal` bounds.
const FLOAT_ATTEMPTS: i32 = 64;
const FLOAT_MIN_DISTANCE_SQR: f64 = 1.0;
const FLOAT_MAX_DISTANCE_SQR: f64 = 3600.0;
/// Vanilla `LevelEvent` ids for the ghast charge and shoot sounds.
const GHAST_CHARGE_LEVEL_EVENT: i32 = 1015;
const GHAST_SHOOT_LEVEL_EVENT: i32 = 1016;

/// Vanilla `Ghast`.
#[entity_behavior(class = "Ghast")]
pub struct GhastEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<GhastEntityData>,
    /// Vanilla `Ghast.explosionPower`.
    explosion_power: SyncMutex<i32>,
    /// Vanilla `Ghast.GhastMoveControl.floatDuration`.
    float_duration: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `GhastEntity`.
unsafe impl DowncastType for GhastEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ghast");
}

impl GhastEntity {
    /// Creates a new ghast entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved ghast entity.
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
        mob_base.set_xp_reward(XP_REWARD);
        let mut entity_data = GhastEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.is_charging.set(false);

        {
            // Vanilla `Ghast.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(5, RandomFloatAroundGoal::new());
            goal_selector.add_goal(7, GhastLookGoal::new());
            goal_selector.add_goal(7, GhastShootFireballGoal::new());

            // Vanilla target goal keeps targets within four blocks vertically.
            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(
                1,
                NearestAttackableTargetGoal::new_for_players_with_interval(
                    10,
                    true,
                    false,
                    |_, _| true,
                ),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            explosion_power: SyncMutex::new(DEFAULT_EXPLOSION_POWER),
            float_duration: SyncMutex::new(0),
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

    /// Vanilla `Ghast.isCharging`.
    #[must_use]
    pub fn is_charging(&self) -> bool {
        *self.entity_data.lock().is_charging.get()
    }

    /// Vanilla `Ghast.setCharging`.
    pub fn set_charging(&self, charging: bool) {
        self.entity_data.lock().is_charging.set(charging);
    }

    /// Vanilla `Ghast.getExplosionPower`.
    #[must_use]
    pub fn explosion_power(&self) -> i32 {
        *self.explosion_power.lock()
    }

    /// Vanilla `Ghast.faceMovementDirection`: face the target, else the velocity.
    fn face_movement_direction(&self) {
        let (_, pitch) = self.rotation();
        if let Some(target) = self.target() {
            if self.position().distance_squared(target.position()) < FIREBALL_MAX_DISTANCE_SQR {
                let delta = target.position() - self.position();
                let yaw = -((delta.x as f32).atan2(delta.z as f32)).to_degrees();
                self.set_rotation((yaw, pitch));
                self.set_y_body_rot(yaw);
            }
            return;
        }

        let movement = self.velocity();
        let yaw = -((movement.x as f32).atan2(movement.z as f32)).to_degrees();
        self.set_rotation((yaw, pitch));
        self.set_y_body_rot(yaw);
    }

    /// Vanilla `Ghast.GhastMoveControl.tick` flight acceleration.
    fn tick_move_control(&self) {
        let wanted = self
            .mob_base()
            .controls()
            .lock()
            .move_control
            .wanted_position();
        if self.mob_base().controls().lock().move_control.operation()
            != MoveControlOperation::MoveTo
        {
            return;
        }

        let mut float_duration = self.float_duration.lock();
        *float_duration -= 1;
        if *float_duration > 0 {
            return;
        }
        *float_duration = *float_duration
            + rand::random_range(0..MOVE_FLOAT_DURATION_RANGE)
            + MOVE_FLOAT_DURATION_BASE;

        let travel = wanted - self.position();
        let Some(direction) = travel.try_normalize() else {
            // Vanilla `GhastMoveControl.canReach` failure: drop the wanted position.
            self.mob_base().controls().lock().move_control.set_wait();
            return;
        };
        if travel.length_squared() < 1.0 {
            // Vanilla sets the operation back to `WAIT` once it arrives.
            self.mob_base().controls().lock().move_control.set_wait();
            return;
        }
        let flying_speed = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::FLYING_SPEED)
            .unwrap_or(0.0);
        self.set_velocity(self.velocity() + direction * (flying_speed * FLYING_SPEED_SCALE));
    }

    /// Vanilla `Ghast.checkGhastSpawnRules`, the `SpawnPlacements` predicate for
    /// `EntityType.GHAST`.
    #[must_use]
    pub fn check_ghast_spawn_rules(
        entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        random: &mut RandomSource,
    ) -> bool {
        level.difficulty() != Difficulty::Peaceful
            && random.next_i32_bounded(20) == 0
            && <Self as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
    }
}

/// Vanilla `Ghast.GhastLookGoal`.
struct GhastLookGoal;

impl GhastLookGoal {
    const fn new() -> Self {
        Self
    }

    fn ghast(mob: &dyn PathfinderMob) -> Option<&GhastEntity> {
        mob.as_entity_event_source().downcast_ref::<GhastEntity>()
    }
}

impl Goal for GhastLookGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        true
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if let Some(ghast) = Self::ghast(mob) {
            ghast.face_movement_direction();
        }
    }
}

/// Vanilla `Ghast.GhastShootFireballGoal`.
struct GhastShootFireballGoal {
    charge_time: i32,
}

impl GhastShootFireballGoal {
    const fn new() -> Self {
        Self { charge_time: 0 }
    }

    fn ghast(mob: &dyn PathfinderMob) -> Option<&GhastEntity> {
        mob.as_entity_event_source().downcast_ref::<GhastEntity>()
    }
}

impl Goal for GhastShootFireballGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target().is_some()
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.charge_time = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(ghast) = Self::ghast(mob) {
            ghast.set_charging(false);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(ghast) = Self::ghast(mob) else {
            return;
        };
        let Some(target) = mob.target() else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };

        if mob.position().distance_squared(target.position()) < FIREBALL_MAX_DISTANCE_SQR
            && mob.has_line_of_sight_cached(target.as_ref())
        {
            self.charge_time += 1;
            if self.charge_time == CHARGE_SOUND_TICK && !ghast.is_silent() {
                world.level_event(GHAST_CHARGE_LEVEL_EVENT, ghast.block_position(), 0, None);
            }

            if self.charge_time == FIREBALL_RELEASE_TICK {
                let (yaw, pitch) = mob.rotation();
                let view = mob.calculate_view_vector(pitch, yaw);
                let origin = DVec3::new(
                    mob.position().x + view.x * FIREBALL_MUZZLE_OFFSET,
                    mob.position().y + 0.5,
                    mob.position().z + view.z * FIREBALL_MUZZLE_OFFSET,
                );
                let direction = DVec3::new(
                    target.position().x - (mob.position().x + view.x * FIREBALL_MUZZLE_OFFSET),
                    (target.position().y + 0.5) - (0.5 + mob.position().y + 0.5),
                    target.position().z - (mob.position().z + view.z * FIREBALL_MUZZLE_OFFSET),
                );

                if !ghast.is_silent() {
                    world.level_event(GHAST_SHOOT_LEVEL_EVENT, ghast.block_position(), 0, None);
                }

                if let Some(direction) = direction.try_normalize() {
                    let projectile = LargeFireballEntity::with_direction(
                        &vanilla_entities::FIREBALL,
                        next_entity_id(),
                        origin,
                        direction,
                        Arc::downgrade(&world),
                    );
                    projectile.set_explosion_power(ghast.explosion_power());
                    let mut item_stack = ItemStack::empty();
                    let _ = spawn_projectile_using_shoot(
                        &world,
                        projectile,
                        &mut item_stack,
                        target.position(),
                        1.0,
                        0.0,
                    );
                }
                self.charge_time = FIREBALL_COOLDOWN_TICKS;
            }
        } else if self.charge_time > 0 {
            self.charge_time -= 1;
        }

        ghast.set_charging(self.charge_time > CHARGE_SOUND_TICK);
    }
}

/// Vanilla `Ghast.RandomFloatAroundGoal`.
struct RandomFloatAroundGoal;

impl RandomFloatAroundGoal {
    const fn new() -> Self {
        Self
    }

    fn ghast(mob: &dyn PathfinderMob) -> Option<&GhastEntity> {
        mob.as_entity_event_source().downcast_ref::<GhastEntity>()
    }
}

impl Goal for RandomFloatAroundGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let wanted = mob
            .mob_base()
            .controls()
            .lock()
            .move_control
            .wanted_position();
        let distance_sqr = mob.position().distance_squared(wanted);
        distance_sqr < FLOAT_MIN_DISTANCE_SQR || distance_sqr > FLOAT_MAX_DISTANCE_SQR
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if Self::ghast(mob).is_none() {
            return;
        }

        let center = mob.position();
        let mut result = center;
        for _ in 0..FLOAT_ATTEMPTS {
            let candidate = center
                + DVec3::new(
                    (rand::random::<f64>() * 2.0 - 1.0) * 16.0,
                    (rand::random::<f64>() * 2.0 - 1.0) * 16.0,
                    (rand::random::<f64>() * 2.0 - 1.0) * 16.0,
                );
            result = candidate;
            if let Some(world) = mob.level() {
                // Vanilla `isGoodTarget` with `distanceToBlocks == 0` accepts any target; the
                // motion-blocking height clamp below keeps the ghast above the ground.
                let pos = BlockPos::containing(candidate.x, candidate.y, candidate.z);
                if candidate.y > f64::from(world.get_min_y()) && world.get_block_state(pos).is_air()
                {
                    break;
                }
            }
        }

        mob.mob_base()
            .controls()
            .lock()
            .move_control
            .set_wanted_position(result, 1.0);
    }
}

impl Entity for GhastEntity {
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
        self.tick_move_control();
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
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
        SoundSource::Hostile
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    /// Vanilla `Ghast.onClimbable` is `false` and fall damage is skipped entirely.
    fn on_climbable(&self) -> bool {
        false
    }

    fn cause_fall_damage(
        self: Arc<Self>,
        _fall_distance: f64,
        _damage_modifier: f32,
        _source: &DamageSource,
    ) -> bool {
        false
    }

    /// Vanilla `Ghast.hurtServer`: a reflected fireball deals 1000 damage.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let reflected_fireball = source
            .direct_entity()
            .is_some_and(|direct| direct.entity_type() == &vanilla_entities::FIREBALL)
            && source
                .causing_entity()
                .is_some_and(|causing| causing.entity_type() == &vanilla_entities::PLAYER);

        if reflected_fireball {
            return LivingEntity::hurt_server(self, world, source, 1000.0);
        }
        if LivingEntity::is_invulnerable_to(self, world, source) {
            return false;
        }
        LivingEntity::hurt_server(self, world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("ExplosionPower", *self.explosion_power.lock() as i8);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        *self.explosion_power.lock() = i32::from(
            nbt.byte("ExplosionPower")
                .unwrap_or(DEFAULT_EXPLOSION_POWER as i8),
        );
    }
}

impl LivingEntity for GhastEntity {
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
        GHAST_SOUND_VOLUME
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_GHAST_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_GHAST_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for GhastEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    /// Vanilla `Ghast.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        1
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_GHAST_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Ghast.checkGhastSpawnRules`: non-peaceful, a 1-in-20 roll, then the shared rules.
    fn check_mob_spawn_rules(
        entity_type: EntityTypeRef,
        level: &dyn LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        if rand::random_range(0..GHAST_SPAWN_CHANCE_BOUND) != 0 {
            return false;
        }
        // Inlined `Mob::check_mob_spawn_rules` default, which vanilla
        // `Ghast.checkGhastSpawnRules` reaches through `checkMobSpawnRules`; Rust has no
        // super-call, so the shared body is repeated here. The non-peaceful gate is
        // applied by `Monster::check_monster_spawn_rules` via
        // `is_spawn_allowed_on_difficulty`.
        if spawn_reason.is_spawner() {
            return true;
        }
        let below = pos.below();
        let state = level.get_block_state(below);
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .is_valid_spawn(state, level, below, entity_type)
    }
}

impl Monster for GhastEntity {}

impl PathfinderMob for GhastEntity {}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::GhastEntity;
    use crate::entity::Entity;

    #[test]
    fn ghast_charge_and_explosion_power_round_trip() {
        init_vanilla_registry();
        let ghast = GhastEntity::new(&vanilla_entities::GHAST, 1, DVec3::ZERO, Weak::new());

        assert!(!ghast.is_charging());
        assert_eq!(ghast.explosion_power(), 1);

        ghast.set_charging(true);
        assert!(ghast.is_charging());

        let mut nbt = NbtCompound::new();
        ghast.save_additional(&mut nbt);
        assert_eq!(nbt.byte("ExplosionPower"), Some(1));
        assert_eq!(ghast.entity_type(), &vanilla_entities::GHAST);
    }
}
