//! Vanilla `Breeze` (`net.minecraft.world.entity.monster.breeze.Breeze`).
//!
//! Brain-driven in vanilla (`BreezeAi`, owned by the brain slice); this file owns the entity
//! state the brain and the breeze behaviors read: pose, whirl sound, jump trail, projectile
//! deflection and the brain hook.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::particle_type::{BlockParticleOption, ParticleData};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::BreezeEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_blocks, vanilla_entities, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::brain::Brain;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

use super::brain;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 10;
/// Vanilla `Breeze` jump-trail particle limit.
const JUMP_TRAIL_PARTICLE_TICKS: i32 = 5;
/// Vanilla `Breeze.emitJumpTrailParticles` particle count.
const JUMP_TRAIL_PARTICLES: i32 = 3;
/// Vanilla `Breeze.causeFallDamage` landing-sound threshold.
const LAND_SOUND_FALL_DISTANCE: f64 = 3.0;

/// Vanilla `Breeze`.
#[entity_behavior(class = "Breeze")]
pub struct BreezeEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<BreezeEntityData>,
    brain: SyncMutex<Brain>,
    /// Vanilla `Breeze.jumpTrailStartedTick`.
    jump_trail_started_tick: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BreezeEntity`.
unsafe impl DowncastType for BreezeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/breeze");
}

impl BreezeEntity {
    /// Creates a new breeze entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved breeze entity.
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
        let mut entity_data = BreezeEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        // Vanilla `Breeze` constructor pathfinding maluses.
        {
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::OnTopOfTrapdoor, -1.0);
            malus.set(PathType::Fire, -1.0);
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(brain::make_brain()),
            jump_trail_started_tick: SyncMutex::new(0),
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

    /// Vanilla `Breeze.getFiringYPosition`.
    #[must_use]
    pub fn firing_y_position(&self) -> f64 {
        self.position().y + f64::from(self.entity_type.dimensions.height) / 2.0 + 0.3
    }

    /// Vanilla `Breeze.playWhirlSound`.
    pub fn play_whirl_sound(&self) {
        self.play_sound(
            &sound_events::ENTITY_BREEZE_WHIRL,
            0.8 + rand::random::<f32>() * 0.2,
            0.7 + rand::random::<f32>() * 0.4,
        );
    }

    /// Vanilla `Breeze.resetJumpTrail`.
    pub fn reset_jump_trail(&self) {
        *self.jump_trail_started_tick.lock() = 0;
    }

    /// Vanilla `Breeze.emitJumpTrailParticles`.
    pub fn emit_jump_trail_particles(&self) {
        let mut tick = self.jump_trail_started_tick.lock();
        *tick += 1;
        if *tick > JUMP_TRAIL_PARTICLE_TICKS {
            return;
        }
        drop(tick);

        let Some(world) = self.level() else {
            return;
        };
        let ground = self.ground_state_for_particles();
        let centered = self.position() + self.velocity() + DVec3::new(0.0, 0.1, 0.0);
        for _ in 0..JUMP_TRAIL_PARTICLES {
            world.send_particles(
                ParticleData::new(
                    &vanilla_particle_types::BLOCK,
                    BlockParticleOption::new(ground),
                ),
                centered,
                1,
                DVec3::ZERO,
                0.0,
            );
        }
    }

    /// Vanilla `Breeze.emitGroundParticles`.
    pub fn emit_ground_particles(&self, amount: i32) {
        if self.is_passenger() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let aabb = self.bounding_box();
        let center = aabb.center();
        let position = DVec3::new(center.x, self.position().y, center.z);
        let ground = self.ground_state_for_particles();
        for _ in 0..amount {
            world.send_particles(
                ParticleData::new(
                    &vanilla_particle_types::BLOCK,
                    BlockParticleOption::new(ground),
                ),
                position,
                1,
                DVec3::ZERO,
                0.0,
            );
        }
    }

    /// Vanilla `getInBlockState().isAir() ? getBlockStateOn() : getInBlockState()`.
    fn ground_state_for_particles(&self) -> BlockStateId {
        if let Some(world) = self.level() {
            let in_block = self.in_block_state(&world);
            if !in_block.is_air() {
                return in_block;
            }
            return world.get_block_state(self.block_position().offset(0, -1, 0));
        }
        vanilla_blocks::AIR.default_state()
    }

    /// Vanilla `Breeze.canAttack`: only players and iron golems.
    #[must_use]
    pub fn can_attack_breeze_target(&self, target: &dyn LivingEntity) -> bool {
        let entity_type = target.entity_type();
        (entity_type == &vanilla_entities::PLAYER || entity_type == &vanilla_entities::IRON_GOLEM)
            && Mob::can_attack(self, target)
    }

    /// Vanilla `Breeze.withinInnerCircleRange` (`closerThan(ourPos, 4.0, 10.0)`).
    #[must_use]
    pub fn within_inner_circle_range(&self, target: DVec3) -> bool {
        let our_position = DVec3::new(
            f64::from(self.block_position().x()) + 0.5,
            f64::from(self.block_position().y()) + 0.5,
            f64::from(self.block_position().z()) + 0.5,
        );
        let delta = target - our_position;
        // Vanilla `Vec3.closerThan(ourPos, 4.0, 10.0)`: 4 blocks horizontally, 10 vertically.
        delta.length_squared() < 100.0 && delta.x * delta.x + delta.z * delta.z < 16.0
    }
}

impl Entity for BreezeEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
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

    /// Vanilla `Breeze.causeFallDamage` plays the landing sound for long falls.
    fn cause_fall_damage(
        self: Arc<Self>,
        fall_distance: f64,
        damage_modifier: f32,
        source: &DamageSource,
    ) -> bool {
        if fall_distance > LAND_SOUND_FALL_DISTANCE {
            self.play_sound(&sound_events::ENTITY_BREEZE_LAND, 1.0, 1.0);
        }
        LivingEntity::cause_living_fall_damage(
            self.as_ref(),
            fall_distance,
            damage_modifier,
            source,
        )
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
    }
}

impl LivingEntity for BreezeEntity {
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
        Some(&sound_events::ENTITY_BREEZE_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_BREEZE_DEATH)
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
    fn default_is_invulnerable_to(&self, source: &DamageSource) -> bool {
        // Vanilla `Breeze.isInvulnerableTo`: wind charges never hurt a breeze.
        // (This used to call `LivingEntity::is_invulnerable_to(self, ..)` from inside
        // its own override, i.e. unconditionally recurse.)
        source
            .causing_entity()
            .is_some_and(|entity| entity.entity_type() == &vanilla_entities::BREEZE)
            || self.is_invulnerable_to_base(source)
    }
}

impl Mob for BreezeEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Breeze.getAmbientSound` depends on being grounded.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.on_ground() {
            &sound_events::ENTITY_BREEZE_IDLE_GROUND
        } else {
            &sound_events::ENTITY_BREEZE_IDLE_AIR
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `Breeze.customServerAiStep`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
        brain::update_activity(&mut brain);
    }

    /// Vanilla `Breeze.getMaxHeadYRot`.
    fn max_head_y_rot(&self) -> f32 {
        30.0
    }

    /// Vanilla `Breeze.canAttack` restricts targets to players and iron golems.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        self.can_attack_breeze_target(target)
    }
}

impl Monster for BreezeEntity {}

impl PathfinderMob for BreezeEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::BreezeEntity;
    use crate::entity::Entity;

    #[test]
    fn breeze_inner_circle_range_matches_vanilla_bounds() {
        init_vanilla_registry();
        let breeze = BreezeEntity::new(
            &vanilla_entities::BREEZE,
            1,
            DVec3::new(0.5, 64.0, 0.5),
            Weak::new(),
        );

        // Within 4 blocks horizontally and 10 vertically.
        assert!(breeze.within_inner_circle_range(DVec3::new(0.5, 70.0, 0.5)));
        assert!(!breeze.within_inner_circle_range(DVec3::new(0.5, 80.0, 0.5)));
        assert!(!breeze.within_inner_circle_range(DVec3::new(10.5, 64.0, 0.5)));
        assert_eq!(breeze.entity_type(), &vanilla_entities::BREEZE);
    }
}
