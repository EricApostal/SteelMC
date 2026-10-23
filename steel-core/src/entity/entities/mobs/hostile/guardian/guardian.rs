//! Vanilla `Guardian` (`net.minecraft.world.entity.monster.Guardian`).
//!
//! Vanilla swims with `WaterBoundPathNavigation` and `Guardian.GuardianMoveControl`; Steel
//! selects the water-bound navigation kind on its shared navigation and ticks the guardian
//! move control in [`GuardianEntity`]/[`ElderGuardianEntity`]'s `tick_move_control`.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::GuardianEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_damage_types, vanilla_entities};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::RandomSource;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::control::{MoveControlOperation, rotate_towards};
use crate::entity::ai::goal::{
    Goal, GoalControls, LookAtPlayerGoal, MoveTowardsRestrictionGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal, RandomStrollGoal,
};
use crate::entity::ai::navigation::NavigationKind;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 10;
/// Vanilla `Guardian.getAttackDuration`.
pub(crate) const GUARDIAN_ATTACK_DURATION: i32 = 80;
/// Vanilla `Guardian.getAmbientSoundInterval`.
const GUARDIAN_AMBIENT_INTERVAL: i32 = 160;
/// Vanilla `Guardian` thorns damage.
const GUARDIAN_THORNS_DAMAGE: f32 = 2.0;
/// Vanilla `Guardian.GuardianAttackSelector` minimum distance (squared).
const GUARDIAN_MIN_TARGET_DISTANCE_SQR: f64 = 9.0;
/// Vanilla `Guardian.checkGuardianSpawnRules` spawn roll bound.
const GUARDIAN_SPAWN_CHANCE_BOUND: i32 = 20;
/// Vanilla `Guardian.travelInWater` drag.
const GUARDIAN_WATER_DRAG: f64 = 0.9;
/// Vanilla `Guardian.travelInWater` sink acceleration when idle.
const GUARDIAN_IDLE_SINK: f64 = -0.005;
/// Vanilla `Guardian.GuardianAttackGoal` magic damage base and hard-difficulty bonus.
const GUARDIAN_MAGIC_DAMAGE: f32 = 1.0;
const GUARDIAN_MAGIC_DAMAGE_HARD: f32 = 2.0;
/// Vanilla `Guardian.GuardianAttackGoal` beam start tick offset.
const GUARDIAN_BEAM_START_TICK: i32 = -10;
/// Vanilla `Guardian.GuardianMoveControl`: `rotlerp(..., 90.0F)`.
const GUARDIAN_MOVE_CONTROL_MAX_TURN: f32 = 90.0;
/// Vanilla `Guardian.GuardianMoveControl`: `Mth.lerp(0.125F, ...)`.
const GUARDIAN_SPEED_LERP: f32 = 0.125;
/// Vanilla `Guardian.GuardianMoveControl`: the idle sine wobble amplitude.
const GUARDIAN_PUSH: f64 = 0.05;

/// Vanilla `Guardian`.
#[entity_behavior(class = "Guardian")]
pub struct GuardianEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<GuardianEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `GuardianEntity`.
unsafe impl DowncastType for GuardianEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/guardian");
}

impl GuardianEntity {
    /// Creates a new guardian entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved guardian entity.
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
        // Vanilla `Guardian`: `setPathfindingMalus(WATER, 0.0F)` and
        // `createNavigation` -> `WaterBoundPathNavigation`.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, 0.0);
        mob_base
            .navigation()
            .lock()
            .set_kind(NavigationKind::WaterBound);
        let mut entity_data = GuardianEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.id_moving.set(false);
        entity_data.id_attack_target.set(0);

        {
            // Vanilla `Guardian.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(4, GuardianAttackGoal::new(false));
            goal_selector.add_goal(5, MoveTowardsRestrictionGoal::new(1.0));
            goal_selector.add_goal(7, RandomStrollGoal::with_interval(1.0, 80));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(
                8,
                LookAtPlayerGoal::new_for_living_entities(12.0, 0.01, |target, _world| {
                    target.entity_type() == &vanilla_entities::GUARDIAN
                        || target.entity_type() == &vanilla_entities::ELDER_GUARDIAN
                }),
            );
            goal_selector.add_goal(9, RandomLookAroundGoal::new());

            // Vanilla `Guardian` targets players, squid and axolotls beyond melee range.
            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(
                1,
                NearestAttackableTargetGoal::new_with_interval(
                    10,
                    true,
                    false,
                    |target, _world| {
                        let is_prey = target.entity_type() == &vanilla_entities::PLAYER
                            || target.entity_type() == &vanilla_entities::SQUID
                            || target.entity_type() == &vanilla_entities::GLOW_SQUID
                            || target.entity_type() == &vanilla_entities::AXOLOTL;
                        is_prey
                    },
                ),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
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

    /// Vanilla `Guardian.isMoving`.
    #[must_use]
    pub fn is_moving(&self) -> bool {
        *self.entity_data.lock().id_moving.get()
    }

    /// Vanilla `Guardian.setMoving`.
    pub fn set_moving(&self, moving: bool) {
        self.entity_data.lock().id_moving.set(moving);
    }

    /// Vanilla `Guardian.getAttackDuration`; elder guardians override it.
    #[must_use]
    pub fn attack_duration(&self) -> i32 {
        GUARDIAN_ATTACK_DURATION
    }

    /// Vanilla `Guardian.hasActiveAttackTarget`.
    #[must_use]
    pub fn has_active_attack_target(&self) -> bool {
        *self.entity_data.lock().id_attack_target.get() != 0
    }

    /// Vanilla `Guardian.setActiveAttackTarget`.
    pub fn set_active_attack_target(&self, entity_id: i32) {
        self.entity_data.lock().id_attack_target.set(entity_id);
    }

    /// Vanilla `Guardian.getFlopSound`.
    #[must_use]
    pub fn flop_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_GUARDIAN_FLOP
    }

    /// Vanilla `Guardian.travelInWater`: water drag plus idle sinking.
    fn tick_water_travel(&self) {
        if !self.is_in_water() {
            return;
        }
        let velocity = self.velocity() * GUARDIAN_WATER_DRAG;
        let velocity = if !self.is_moving() && self.target().is_none() {
            DVec3::new(velocity.x, velocity.y + GUARDIAN_IDLE_SINK, velocity.z)
        } else {
            velocity
        };
        self.set_velocity(velocity);
    }

    /// Vanilla `Guardian.checkGuardianSpawnRules`, the `SpawnPlacements` predicate for
    /// `EntityType.GUARDIAN` and `EntityType.ELDER_GUARDIAN`.
    #[must_use]
    pub fn check_guardian_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        random: &mut RandomSource,
    ) -> bool {
        (random.next_i32_bounded(20) == 0 || !can_see_sky_from_below_water(level, pos))
            && level.difficulty() != Difficulty::Peaceful
            && (spawn_reason.is_spawner()
                || level.get_block_state(pos).get_fluid_state().is_water())
            && level
                .get_block_state(pos.below())
                .get_fluid_state()
                .is_water()
    }
}

/// Vanilla `LevelReader.canSeeSkyFromBelowWater`.
///
/// Deliberate difference: Steel has no per-block `liquid` property, so a
/// non-empty fluid state stands in for vanilla's `BlockState.liquid()`.
fn can_see_sky_from_below_water(level: &World, pos: BlockPos) -> bool {
    if pos.y() >= level.sea_level() {
        return level.can_see_sky(pos);
    }
    let scan_point = BlockPos::new(pos.x(), level.sea_level(), pos.z());
    if !level.can_see_sky(scan_point) {
        return false;
    }
    let mut y = scan_point.y() - 1;
    while y > pos.y() {
        let state = level.get_block_state(BlockPos::new(pos.x(), y, pos.z()));
        if state.get_light_dampening() > 0 && state.get_fluid_state().is_empty() {
            return false;
        }
        y -= 1;
    }
    true
}

/// Vanilla `Guardian.GuardianAttackGoal` (the beam attack).
pub(crate) struct GuardianAttackGoal {
    attack_time: i32,
    elder: bool,
}

impl GuardianAttackGoal {
    pub(crate) const fn new(elder: bool) -> Self {
        Self {
            attack_time: 0,
            elder,
        }
    }

    fn guardian(mob: &dyn PathfinderMob) -> Option<&GuardianEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<GuardianEntity>()
    }
}

impl Goal for GuardianAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target()
            .and_then(|target| target.as_living_entity().map(LivingEntity::is_alive))
            .unwrap_or(false)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.elder {
            return true;
        }
        mob.target().is_some_and(|target| {
            mob.position().distance_squared(target.position()) > GUARDIAN_MIN_TARGET_DISTANCE_SQR
        })
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.attack_time = GUARDIAN_BEAM_START_TICK;
        mob.mob_base().navigation().lock().stop();
        if let Some(target) = mob.target() {
            mob.mob_base().controls().lock().look_control.set_look_at(
                target.position(),
                90.0,
                90.0,
            );
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(guardian) = Self::guardian(mob) {
            guardian.set_active_attack_target(0);
        }
        mob.set_target(None);
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(guardian) = Self::guardian(mob) else {
            return;
        };
        let Some(target) = mob.target() else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };

        mob.mob_base().navigation().lock().stop();
        mob.mob_base()
            .controls()
            .lock()
            .look_control
            .set_look_at(target.position(), 90.0, 90.0);

        if !mob.has_line_of_sight_cached(target.as_ref()) {
            mob.set_target(None);
            return;
        }

        self.attack_time += 1;
        if self.attack_time == 0 {
            guardian.set_active_attack_target(target.id());
            if !guardian.is_silent() {
                guardian.broadcast_entity_event(EntityStatus::GuardianAttackSound);
            }
        } else if self.attack_time >= guardian.attack_duration() {
            let mut magic_damage = GUARDIAN_MAGIC_DAMAGE;
            if world.difficulty() == Difficulty::Hard {
                magic_damage += GUARDIAN_MAGIC_DAMAGE_HARD;
            }
            if self.elder {
                magic_damage += GUARDIAN_MAGIC_DAMAGE_HARD;
            }

            let magic_source =
                DamageSource::direct(&vanilla_damage_types::INDIRECT_MAGIC, entity.clone());
            let _ = target.hurt(&world, &magic_source, magic_damage);
            Mob::do_hurt_target(guardian, entity, &world, &target);
            mob.set_target(None);
        }
    }
}

impl Entity for GuardianEntity {
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
        self.tick_water_travel();
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

    /// Vanilla `Guardian.hurtServer`: spiky guardians damage melee attackers.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let avoids_thorns = source.is(&steel_utils::Identifier::vanilla_static(
            "avoids_guardian_thorns",
        )) || source.damage_type == &vanilla_damage_types::THORNS;
        if !self.is_moving()
            && !avoids_thorns
            && let Some(direct) = source.direct_entity()
            && let Some(attacker) = direct.as_living_entity()
        {
            if let Some(self_entity) = world.get_entity_by_id(self.id()) {
                let thorns_source =
                    DamageSource::direct(&vanilla_damage_types::THORNS, self_entity);
                attacker.hurt(world, &thorns_source, GUARDIAN_THORNS_DAMAGE);
            }
        }

        LivingEntity::hurt_server(self, world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
    }
}

impl LivingEntity for GuardianEntity {
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
        Some(if self.is_in_water() {
            &sound_events::ENTITY_GUARDIAN_HURT
        } else {
            &sound_events::ENTITY_GUARDIAN_HURT_LAND
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_in_water() {
            &sound_events::ENTITY_GUARDIAN_DEATH
        } else {
            &sound_events::ENTITY_GUARDIAN_DEATH_LAND
        })
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

impl Mob for GuardianEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Guardian.GuardianMoveControl.tick`.
    fn tick_move_control(&self) {
        let moving = tick_guardian_move_control(self);
        self.set_moving(moving);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_in_water() {
            &sound_events::ENTITY_GUARDIAN_AMBIENT
        } else {
            &sound_events::ENTITY_GUARDIAN_AMBIENT_LAND
        })
    }

    fn ambient_sound_interval(&self) -> i32 {
        GUARDIAN_AMBIENT_INTERVAL
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Guardian.getMaxHeadXRot` is 180.
    fn max_head_x_rot(&self) -> f32 {
        180.0
    }

    /// Vanilla `Guardian.checkGuardianSpawnRules`.
    fn check_mob_spawn_rules(
        entity_type: EntityTypeRef,
        level: &dyn LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        // The non-peaceful gate is applied by `<Self as Monster>::check_mob_spawn_rules`'s
        // `is_spawn_allowed_on_difficulty`; `LevelReader` exposes no difficulty accessor.
        if !spawn_reason.is_spawner() && !level.get_block_state(pos).get_fluid_state().is_water() {
            return false;
        }
        if !level
            .get_block_state(pos.offset(0, -1, 0))
            .get_fluid_state()
            .is_water()
        {
            return false;
        }
        <Self as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos)
    }
}

impl Monster for GuardianEntity {}

impl PathfinderMob for GuardianEntity {
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        if let Some(world) = self.level() {
            if world.get_block_state(pos).get_fluid_state().is_water() {
                return 10.0 + LevelReader::pathfinding_cost_from_light_levels(world.as_ref(), pos);
            }
        }
        Monster::get_walk_target_value_monster(self, pos)
    }
}

/// Vanilla `Guardian.GuardianMoveControl.tick`.
///
/// Shared by `Guardian` and `ElderGuardian` (`ElderGuardian` inherits the same
/// inner control class in vanilla). Lives here because Steel's `Mob` trait ticks
/// controls through `tick_move_control` rather than a per-mob control object.
pub(super) fn tick_guardian_move_control<M: Mob + ?Sized>(mob: &M) -> bool {
    let move_control = {
        let mut controls = mob.mob_base().controls().lock();
        let move_control = controls.move_control;
        if matches!(move_control.operation(), MoveControlOperation::MoveTo) {
            controls.move_control.set_wait();
        }
        move_control
    };

    if move_control.operation() != MoveControlOperation::MoveTo
        || mob.mob_base().navigation().lock().is_done()
    {
        mob.set_mob_speed(0.0);
        return false;
    }

    let delta = move_control.wanted_position() - mob.position();
    let length = delta.length();
    if length <= 0.0 {
        // Vanilla divides by `length` unguarded; a zero-length delta means the
        // wanted position is the current position, so idle instead of emitting NaN.
        mob.set_mob_speed(0.0);
        return false;
    }
    let (xd, yd, zd) = (delta.x / length, delta.y / length, delta.z / length);

    let y_rot_d = (delta.z.atan2(delta.x).to_degrees() as f32) - 90.0;
    let (yaw, pitch) = mob.rotation();
    let new_yaw = rotate_towards(yaw, y_rot_d, GUARDIAN_MOVE_CONTROL_MAX_TURN);
    mob.set_rotation((new_yaw, pitch));
    mob.living_base().set_y_body_rot(new_yaw);

    let movement_speed = mob
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MOVEMENT_SPEED);
    let target_speed = (move_control.speed_modifier() * movement_speed) as f32;
    let new_speed = mob.get_speed() + (target_speed - mob.get_speed()) * GUARDIAN_SPEED_LERP;
    mob.set_mob_speed(new_speed);

    let tick_phase = (mob.tick_count() + mob.id()) as f64 * 0.5;
    let push = tick_phase.sin() * GUARDIAN_PUSH;
    let yaw_radians = f64::from(new_yaw) * (std::f64::consts::PI / 180.0);
    let (sin_yaw, cos_yaw) = yaw_radians.sin_cos();
    let y_push = ((mob.tick_count() + mob.id()) as f64 * 0.75).sin() * GUARDIAN_PUSH;
    mob.set_velocity(
        mob.velocity()
            + DVec3::new(
                push * cos_yaw,
                y_push * (sin_yaw + cos_yaw) * 0.25 + f64::from(new_speed) * yd * 0.1,
                push * sin_yaw,
            ),
    );

    // Vanilla steers the look control toward a point ahead of the swim direction.
    let eye_y = mob.get_eye_y();
    let new_look = DVec3::new(
        mob.position().x + xd * 2.0,
        eye_y + yd / length,
        mob.position().z + zd * 2.0,
    );
    let (old_look, y_max_rot_speed, x_max_rot_angle) = {
        let controls = mob.mob_base().controls().lock();
        let control = controls.look_control;
        let old = if control.is_looking_at_target() {
            control.wanted_position()
        } else {
            new_look
        };
        (old, control.y_max_rot_speed(), control.x_max_rot_angle())
    };
    mob.mob_base().controls().lock().look_control.set_look_at(
        old_look.lerp(new_look, GUARDIAN_SPEED_LERP),
        y_max_rot_speed,
        x_max_rot_angle,
    );
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::GuardianEntity;
    use crate::entity::Entity;

    #[test]
    fn guardian_synced_attack_state_round_trips() {
        init_vanilla_registry();
        let guardian =
            GuardianEntity::new(&vanilla_entities::GUARDIAN, 1, DVec3::ZERO, Weak::new());

        assert!(!guardian.is_moving());
        assert!(!guardian.has_active_attack_target());
        assert_eq!(guardian.attack_duration(), 80);

        guardian.set_moving(true);
        guardian.set_active_attack_target(42);
        assert!(guardian.is_moving());
        assert!(guardian.has_active_attack_target());
        assert_eq!(guardian.entity_type(), &vanilla_entities::GUARDIAN);
    }
}
