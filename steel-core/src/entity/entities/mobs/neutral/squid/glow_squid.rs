//! Vanilla `GlowSquid` (`net.minecraft.world.entity.animal.squid.GlowSquid`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::GlowSquidEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_blocks};
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::water::abstract_fish::check_water_animal_spawn_obstruction;
use crate::entity::{
    AgeableMob, AgeableMobBase, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `GlowSquid` hurt response (`setDarkTicks(100)`).
const HURT_DARK_TICKS: i32 = 100;

/// Vanilla `GlowSquid`.
#[entity_behavior(class = "GlowSquid")]
pub struct GlowSquidEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    entity_data: SyncMutex<GlowSquidEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `GlowSquidEntity`.
unsafe impl DowncastType for GlowSquidEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/glow_squid");
}

impl GlowSquidEntity {
    /// Creates a new glow squid entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved glow squid entity.
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
        let mut entity_data = GlowSquidEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.dark_ticks_remaining.set(0);

        {
            // Vanilla `GlowSquid` inherits `Squid.registerGoals` unchanged.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, super::squid::SquidRandomMovementGoal::new());
            goal_selector.add_goal(1, super::squid::SquidFleeGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
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

    /// Vanilla `GlowSquid.getDarkTicksRemaining`.
    #[must_use]
    pub fn dark_ticks_remaining(&self) -> i32 {
        *self.entity_data.lock().dark_ticks_remaining.get()
    }

    /// Vanilla `GlowSquid.setDarkTicks`.
    pub fn set_dark_ticks(&self, ticks: i32) {
        self.entity_data.lock().dark_ticks_remaining.set(ticks);
    }

    /// Vanilla `GlowSquid.getInkParticle`.
    #[must_use]
    pub fn ink_particle(&self) -> steel_registry::particle_type::ParticleTypeRef {
        &steel_registry::vanilla_particle_types::GLOW_SQUID_INK
    }

    /// Vanilla `GlowSquid.getSquirtSound`.
    #[must_use]
    pub fn squirt_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_GLOW_SQUID_SQUIRT
    }

    /// Vanilla `GlowSquid.aiStep` glow trail.
    fn glow_effects(&self, world: &Arc<World>) {
        let dark_ticks = self.dark_ticks_remaining();
        if dark_ticks > 0 {
            self.set_dark_ticks(dark_ticks - 1);
        }

        let position = self.position();
        let spread = 0.6;
        world.send_particles(
            ParticleData::simple(&steel_registry::vanilla_particle_types::GLOW),
            DVec3::new(
                position.x + (rand::random::<f64>() * 2.0 - 1.0) * spread,
                position.y + rand::random::<f64>() * f64::from(self.entity_type.dimensions.height),
                position.z + (rand::random::<f64>() * 2.0 - 1.0) * spread,
            ),
            1,
            DVec3::ZERO,
            0.0,
        );
    }

    /// Vanilla `GlowSquid.checkGlowSquidSpawnRules`, the `SpawnPlacements` predicate
    /// for `EntityType.GLOW_SQUID`.
    ///
    /// Vanilla ignores `entity_type`, `spawnReason` and `random`; the position must
    /// be at least 33 blocks below sea level, fully dark and inside water.
    #[must_use]
    pub fn check_glow_squid_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut RandomSource,
    ) -> bool {
        pos.y() <= level.sea_level() - 33
            && level.raw_brightness(pos, 0) == 0
            && level.get_block_state(pos).get_block() == &vanilla_blocks::WATER
    }
}

impl Entity for GlowSquidEntity {
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
        if let Some(world) = self.level() {
            self.glow_effects(&world);
        }
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        // Vanilla `GlowSquid` inherits `Squid.getDefaultDimensions`.
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            EntityDimensions::new(
                super::squid::SQUID_BABY_WIDTH,
                super::squid::SQUID_BABY_HEIGHT,
                super::squid::SQUID_BABY_EYE_HEIGHT,
            )
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
            &sound_events::ENTITY_GLOW_SQUID_SQUIRT,
            &sound_events::ENTITY_GLOW_SQUID_SQUIRT,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_GLOW_SQUID_SQUIRT
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        nbt.insert("DarkTicksRemaining", self.dark_ticks_remaining());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.set_dark_ticks(nbt.int("DarkTicksRemaining").unwrap_or(0));
    }
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let hurt = LivingEntity::hurt_server(self, world, source, amount);
        if hurt {
            self.set_dark_ticks(HURT_DARK_TICKS);
        }
        hurt
    }
}

impl LivingEntity for GlowSquidEntity {
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
        Some(&sound_events::ENTITY_GLOW_SQUID_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_GLOW_SQUID_DEATH)
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
        self.mob_ai_step(entity)
    }
}

impl Mob for GlowSquidEntity {
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
        Some(&sound_events::ENTITY_GLOW_SQUID_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn mob_can_be_leashed(&self) -> bool {
        true
    }
}

impl AgeableMob for GlowSquidEntity {
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

impl PathfinderMob for GlowSquidEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::GlowSquidEntity;
    use crate::entity::Entity;

    #[test]
    fn glow_squid_dark_ticks_round_trip() {
        init_vanilla_registry();
        let squid =
            GlowSquidEntity::new(&vanilla_entities::GLOW_SQUID, 1, DVec3::ZERO, Weak::new());

        assert_eq!(squid.dark_ticks_remaining(), 0);
        squid.set_dark_ticks(100);

        let mut nbt = simdnbt::owned::NbtCompound::new();
        squid.save_additional(&mut nbt);
        assert_eq!(nbt.int("DarkTicksRemaining"), Some(100));

        let loaded =
            GlowSquidEntity::new(&vanilla_entities::GLOW_SQUID, 2, DVec3::ZERO, Weak::new());
        loaded.set_dark_ticks(0);
        assert_eq!(loaded.dark_ticks_remaining(), 0);
        assert_eq!(squid.entity_type(), &vanilla_entities::GLOW_SQUID);
    }
}
