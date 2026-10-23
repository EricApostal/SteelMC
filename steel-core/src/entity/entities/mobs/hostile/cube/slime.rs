//! Vanilla `Slime` (`net.minecraft.world.entity.monster.cubemob.Slime`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SlimeEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_particle_types};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{Goal, GoalControls, NearestAttackableTargetGoal};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

use super::abstract_cube_mob::{
    CubeMoveState, CubeSquish, clamp_cube_size, cube_jump_delay, cube_max_health,
    cube_movement_speed, cube_sound_pitch, cube_sound_volume, cube_split_count,
};

const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `Slime`.
#[entity_behavior(class = "Slime")]
pub struct SlimeEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<SlimeEntityData>,
    squish: SyncMutex<CubeSquish>,
    was_on_ground: SyncMutex<bool>,
    move_state: SyncMutex<CubeMoveState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SlimeEntity`.
unsafe impl DowncastType for SlimeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/slime");
}

impl SlimeEntity {
    /// Creates a new slime entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved slime entity.
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
        let mut entity_data = SlimeEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.abstract_cube_mob_mut().id_size.set(1);

        {
            // Vanilla `AbstractCubeMob.registerGoals` plus `Slime.addBehaviourGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, CubeMobFloatGoal::new());
            goal_selector.add_goal(2, CubeMobAttackGoal::new());
            goal_selector.add_goal(4, CubeMobRandomDirectionGoal::new());
            goal_selector.add_goal(5, CubeMobKeepOnJumpingGoal::new());

            // Vanilla `Slime.addTargetingGoals`.
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
            target_selector.add_goal(
                3,
                NearestAttackableTargetGoal::new(true, |target, _world| {
                    target.entity_type() == &vanilla_entities::IRON_GOLEM
                }),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            squish: SyncMutex::new(CubeSquish::new()),
            was_on_ground: SyncMutex::new(false),
            move_state: SyncMutex::new(CubeMoveState::new()),
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

    /// Vanilla `AbstractCubeMob.getSize`.
    #[must_use]
    pub fn size(&self) -> i32 {
        *self.entity_data.lock().abstract_cube_mob().id_size.get()
    }

    /// Vanilla `AbstractCubeMob.isTiny`.
    #[must_use]
    pub fn is_tiny(&self) -> bool {
        self.size() <= 1
    }

    /// Vanilla `AbstractCubeMob.setSize`.
    pub fn set_size(&self, size: i32, update_health: bool) {
        let actual_size = clamp_cube_size(size);
        self.entity_data
            .lock()
            .abstract_cube_mob_mut()
            .id_size
            .set(actual_size);

        {
            let mut attributes = self.attributes().lock();
            attributes.set_base_value(vanilla_attributes::MAX_HEALTH, cube_max_health(actual_size));
            attributes.set_base_value(
                vanilla_attributes::MOVEMENT_SPEED,
                cube_movement_speed(actual_size),
            );
            // Vanilla `Slime.setSize`: attack damage is the raw size.
            attributes.set_base_value(vanilla_attributes::ATTACK_DAMAGE, f64::from(actual_size));
        }
        // Vanilla `Slime.setSize`: `xpReward = actualSize`.
        self.set_xp_reward(actual_size);

        if update_health {
            let max_health = self.get_max_health();
            self.set_health(max_health);
        }
    }

    /// Vanilla `AbstractCubeMob.getDefaultDimensions`.
    #[must_use]
    pub fn cube_dimensions(&self) -> EntityDimensions {
        self.entity_type.dimensions.scale(self.size() as f32)
    }

    /// Vanilla `AbstractCubeMob.canDealDamage`.
    #[must_use]
    pub fn can_deal_damage(&self) -> bool {
        !self.is_tiny() && self.is_effective_ai()
    }

    /// Vanilla `AbstractCubeMob.getSquishSound`.
    #[must_use]
    pub fn squish_sound(&self) -> SoundEventRef {
        if self.is_tiny() {
            &sound_events::ENTITY_SLIME_SQUISH_SMALL
        } else {
            &sound_events::ENTITY_SLIME_SQUISH
        }
    }

    /// Vanilla `AbstractCubeMob.getJumpSound`.
    #[must_use]
    pub fn jump_sound(&self) -> SoundEventRef {
        if self.is_tiny() {
            &sound_events::ENTITY_SLIME_JUMP_SMALL
        } else {
            &sound_events::ENTITY_SLIME_JUMP
        }
    }

    /// Vanilla `AbstractCubeMob.tick`: squish animation, landing squash and landing particles.
    fn tick_cube(&self) {
        self.squish.lock().tick();

        let on_ground = self.on_ground();
        let was_on_ground = *self.was_on_ground.lock();

        if on_ground && !was_on_ground {
            let width = self.cube_dimensions().width;
            if let Some(world) = self.level() {
                let size = width * 2.0;
                let radius = size / 2.0;
                let count = (size * 16.0) as i32;
                let position = self.position();
                for _ in 0..count {
                    let dir = rand::random::<f32>() * std::f32::consts::TAU;
                    let d = rand::random::<f32>() * 0.5 + 0.5;
                    let xd = f64::from(dir.sin() * radius * d);
                    let zd = f64::from(dir.cos() * radius * d);
                    world.send_particles(
                        ParticleData::simple(&vanilla_particle_types::ITEM_SLIME),
                        DVec3::new(position.x + xd, position.y, position.z + zd),
                        1,
                        DVec3::ZERO,
                        0.0,
                    );
                }
            }
            self.squish.lock().target = 1.0;
        }

        *self.was_on_ground.lock() = on_ground;
    }

    /// Vanilla `CubeMobMoveControl.tick`: turn, then jump on the vanilla delay.
    fn tick_cube_movement(&self) {
        let (yaw, _pitch) = self.rotation();
        let mut state = *self.move_state.lock();
        let target_yaw = state.y_rot;
        let new_yaw = crate::entity::ai::control::rotate_towards(yaw, target_yaw, 90.0);
        self.set_rotation((new_yaw, 0.0));
        self.set_y_head_rot(new_yaw);
        self.set_y_body_rot(new_yaw);

        let Some(wanted_speed) = state.wanted_speed else {
            self.set_mob_speed(0.0);
            return;
        };

        let movement_speed = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::MOVEMENT_SPEED)
            .unwrap_or(0.0);

        if self.on_ground() {
            self.set_mob_speed((wanted_speed * movement_speed) as f32);
            if state.tick_jump_delay(cube_jump_delay()) {
                self.jump_control_jump();
                if self.size() > 0 {
                    self.play_sound(
                        self.jump_sound(),
                        cube_sound_volume(self.size()),
                        cube_sound_pitch(self.is_tiny()),
                    );
                }
            } else {
                self.set_mob_speed(0.0);
            }
        } else {
            self.set_mob_speed((wanted_speed * movement_speed) as f32);
        }

        *self.move_state.lock() = state;
    }

    /// Vanilla `AbstractCubeMob.remove` split-on-death.
    fn split_on_death(&self, world: &Arc<World>) {
        let size = self.size();
        if size <= 1 || !LivingEntity::is_dead_or_dying(self) {
            return;
        }

        let width = self.cube_dimensions().width;
        let xz_cube_spawn_offset = width / 2.0;
        let half_size = size / 2;
        let count = cube_split_count();
        let entity_type = self.entity_type;
        let position = self.position();

        for i in 0..count {
            let xd = (i % 2) as f32 - 0.5;
            let zd = (i / 2) as f32 - 0.5;
            let child_position = DVec3::new(
                position.x + f64::from(xd * xz_cube_spawn_offset),
                position.y + 0.5,
                position.z + f64::from(zd * xz_cube_spawn_offset),
            );
            let Some(child) = crate::entity::ENTITIES.create(
                entity_type,
                crate::entity::next_entity_id(),
                child_position,
                Arc::downgrade(world),
            ) else {
                continue;
            };

            if let Some(slime) = child.as_entity_event_source().downcast_ref::<SlimeEntity>() {
                slime.set_size(half_size, true);
            }
            child.snap_to(child_position, rand::random::<f32>() * 360.0, 0.0);
            let _ = world.try_add_entity(child);
        }
    }

    /// Vanilla `AbstractCubeMob.dealDamage` (`push`/`playerTouch` contact damage).
    fn deal_damage(&self, entity: &SharedEntity, world: &World, target: &SharedEntity) {
        if !LivingEntity::is_alive(self) || !self.can_deal_damage() {
            return;
        }
        if target.as_living_entity().is_none() {
            return;
        }

        let width = self.cube_dimensions().width;
        if self.position().distance_squared(target.position()) > f64::from(width * width) {
            return;
        }
        if !self.has_line_of_sight_cached(target.as_ref()) {
            return;
        }

        if Mob::do_hurt_target(self, entity, world, target) {
            self.play_sound(&sound_events::ENTITY_SLIME_ATTACK, 1.0, 1.0);
        }
    }
}

/// Extracts the concrete slime behind a goal context.
fn slime_of(mob: &dyn PathfinderMob) -> Option<&SlimeEntity> {
    mob.as_entity_event_source().downcast_ref::<SlimeEntity>()
}

/// Vanilla `AbstractCubeMob.CubeMobFloatGoal`.
struct CubeMobFloatGoal;

impl CubeMobFloatGoal {
    const fn new() -> Self {
        Self
    }
}

impl Goal for CubeMobFloatGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::JUMP | GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        slime_of(mob).is_some() && (mob.is_in_water() || mob.is_in_lava())
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(slime) = slime_of(mob) else {
            return;
        };
        if rand::random::<f32>() < 0.8 {
            mob.jump_control_jump();
        }
        slime.move_state.lock().set_wanted_movement(1.2);
    }
}

/// Vanilla `AbstractCubeMob.CubeMobKeepOnJumpingGoal`.
struct CubeMobKeepOnJumpingGoal;

impl CubeMobKeepOnJumpingGoal {
    const fn new() -> Self {
        Self
    }
}

impl Goal for CubeMobKeepOnJumpingGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::JUMP | GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.is_passenger()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(slime) = slime_of(mob) else {
            return;
        };
        slime.move_state.lock().set_wanted_movement(1.0);
    }
}

/// Vanilla `AbstractCubeMob.CubeMobRandomDirectionGoal`.
struct CubeMobRandomDirectionGoal {
    chosen_degrees: f32,
    next_randomize_time: i32,
}

impl CubeMobRandomDirectionGoal {
    const fn new() -> Self {
        Self {
            chosen_degrees: 0.0,
            next_randomize_time: 0,
        }
    }
}

impl Goal for CubeMobRandomDirectionGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target().is_none()
            && (mob.on_ground()
                || mob.is_in_water()
                || mob.is_in_lava()
                || mob.has_mob_effect(&steel_registry::vanilla_mob_effects::LEVITATION))
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        self.next_randomize_time -= 1;
        if self.next_randomize_time <= 0 {
            // Vanilla `adjustedTickDelay(40 + random.nextInt(60))`.
            self.next_randomize_time = 20 + rand::random_range(0..30);
            self.chosen_degrees = rand::random_range(0..360) as f32;
        }
        if let Some(slime) = slime_of(mob) {
            slime
                .move_state
                .lock()
                .set_direction(self.chosen_degrees, false);
        }
    }
}

/// Vanilla `AbstractCubeMob.CubeMobAttackGoal`.
struct CubeMobAttackGoal {
    grow_tired_timer: i32,
}

impl CubeMobAttackGoal {
    const fn new() -> Self {
        Self {
            grow_tired_timer: 0,
        }
    }
}

impl Goal for CubeMobAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };
        slime_of(mob).is_some() && Mob::can_attack(mob, target_living)
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        // Vanilla `reducedTickDelay(300)`.
        self.grow_tired_timer = 150;
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };
        if !Mob::can_attack(mob, target_living) {
            return false;
        }
        self.grow_tired_timer -= 1;
        self.grow_tired_timer > 0
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };
        mob.mob_base()
            .controls()
            .lock()
            .look_control
            .set_look_at(target.position(), 10.0, 10.0);

        if let Some(slime) = slime_of(mob) {
            let (yaw, _pitch) = mob.rotation();
            slime
                .move_state
                .lock()
                .set_direction(yaw, slime.can_deal_damage());
        }
    }
}

impl Entity for SlimeEntity {
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
        self.tick_cube();
        self.tick_cube_movement();
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        self.cube_dimensions()
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
        Monster::sound_source_monster(self)
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    /// Vanilla `AbstractCubeMob.addAdditionalSaveData`.
    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("Size", self.size() - 1);
        nbt.insert("wasOnGround", *self.was_on_ground.lock());
    }

    /// Vanilla `AbstractCubeMob.readAdditionalSaveData`.
    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.set_size(nbt.int("Size").unwrap_or(0) + 1, false);
        self.load_mob(nbt);
        *self.was_on_ground.lock() = nbt.byte("wasOnGround").is_some_and(|value| value != 0);
    }
    /// Vanilla `AbstractCubeMob.playerTouch`: contact damage when a player walks into the cube.
    fn player_touch(self: Arc<Self>, player: &Arc<crate::player::Player>) {
        if !self.can_deal_damage() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        let target: SharedEntity = Arc::<crate::player::Player>::clone(player);
        self.deal_damage(&entity, &world, &target);
    }
}

impl LivingEntity for SlimeEntity {
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
        cube_sound_volume(self.size())
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(if self.is_tiny() {
            &sound_events::ENTITY_SLIME_HURT_SMALL
        } else {
            &sound_events::ENTITY_SLIME_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_tiny() {
            &sound_events::ENTITY_SLIME_DEATH_SMALL
        } else {
            &sound_events::ENTITY_SLIME_DEATH
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

    /// Vanilla `AbstractCubeMob.remove` splits a dying slime before removal.
    fn on_death(&self, _source: &DamageSource) {
        if let Some(world) = self.level() {
            self.split_on_death(&world);
        }
    }
}

impl Mob for SlimeEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        None
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `AbstractCubeMob.getMaxHeadXRot` returns 0.
    fn max_head_x_rot(&self) -> f32 {
        0.0
    }

    /// Vanilla `Slime.checkSlimeSpawnRules`: non-peaceful only, spawners delegate to the shared
    /// mob rules.
    ///
    /// BLOCKED(slime spawn): the swamp surface-spawn chance and the slime-chunk seed roll need
    /// biome-tag and chunk-seed queries this static rule cannot reach, so only the peaceful gate
    /// and the spawner path are exact here.
    fn check_mob_spawn_rules(
        entity_type: EntityTypeRef,
        level: &dyn crate::world::LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        if spawn_reason.is_spawner() {
            return <Self as Mob>::check_mob_spawn_rules(entity_type, level, spawn_reason, pos);
        }
        false
    }
}

impl Monster for SlimeEntity {}

impl PathfinderMob for SlimeEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::SlimeEntity;
    use crate::entity::{Entity, LivingEntity};

    #[test]
    fn slime_size_drives_health_damage_and_nbt() {
        init_vanilla_registry();
        let slime = SlimeEntity::new(&vanilla_entities::SLIME, 1, DVec3::ZERO, Weak::new());

        assert_eq!(slime.size(), 1);
        assert!(slime.is_tiny());

        slime.set_size(3, true);
        assert_eq!(slime.size(), 3);
        assert!(!slime.is_tiny());
        assert_eq!(slime.get_max_health(), 9.0);
        assert_eq!(slime.get_health(), 9.0);

        let mut nbt = simdnbt::owned::NbtCompound::new();
        slime.save_additional(&mut nbt);
        assert_eq!(nbt.int("Size"), Some(2));
        assert_eq!(slime.entity_type(), &vanilla_entities::SLIME);
    }
}
