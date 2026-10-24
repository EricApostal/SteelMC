//! Vanilla `Zoglin` (`net.minecraft.world.entity.monster.Zoglin`).
//!
//! Vanilla is brain-driven (`Zoglin.getActivities`, owned by the brain slice) and keeps its own
//! baby flag (`DATA_BABY_ID`) rather than extending `AgeableMob`.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::ZoglinEntityData;
use steel_registry::{sound_events, vanilla_attributes};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::brain::Brain;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

use super::brain;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Zoglin.ATTACK_ANIMATION` ticks.
const ATTACK_ANIMATION_TICKS: i32 = 10;
/// Vanilla `Zoglin` baby spawn chance.
const BABY_SPAWN_CHANCE: f32 = 0.2;
const ZOGLIN_BABY_WIDTH: f32 = 0.75;
const ZOGLIN_BABY_HEIGHT: f32 = 0.85;
const ZOGLIN_BABY_EYE_HEIGHT: f32 = 0.625;

/// Vanilla `Zoglin`.
#[entity_behavior(class = "Zoglin")]
pub struct ZoglinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<ZoglinEntityData>,
    brain: SyncMutex<Brain>,
    /// Vanilla `Zoglin.attackAnimationRemainingTicks`.
    attack_animation_remaining_ticks: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ZoglinEntity`.
unsafe impl DowncastType for ZoglinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/zoglin");
}

impl ZoglinEntity {
    /// Creates a new zoglin entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved zoglin entity.
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
        let mut entity_data = ZoglinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.baby.set(false);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(brain::make_zoglin_brain()),
            attack_animation_remaining_ticks: SyncMutex::new(0),
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

    /// Vanilla `Zoglin.isBaby` (`DATA_BABY_ID`).
    #[must_use]
    pub fn is_baby(&self) -> bool {
        *self.entity_data.lock().baby.get()
    }

    /// Vanilla `Zoglin.setBaby`.
    pub fn set_baby(&self, baby: bool) {
        self.entity_data.lock().baby.set(baby);
    }

    /// Vanilla `Zoglin.isAdult`.
    #[must_use]
    pub fn is_adult(&self) -> bool {
        !self.is_baby()
    }
}

impl Entity for ZoglinEntity {
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
        {
            let mut attack = self.attack_animation_remaining_ticks.lock();
            if *attack > 0 {
                *attack -= 1;
            }
        }
        self.tick_living_entity(&entity);
    }

    /// Vanilla `Zoglin.getDefaultDimensions` uses the baby hitbox for babies.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.is_baby() {
            EntityDimensions::new(
                ZOGLIN_BABY_WIDTH,
                ZOGLIN_BABY_HEIGHT,
                ZOGLIN_BABY_EYE_HEIGHT,
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
        SoundSource::Hostile
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_ZOGLIN_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("IsBaby", i8::from(self.is_baby()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_baby(nbt.byte("IsBaby").is_some_and(|value| value != 0));
    }
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = LivingEntity::hurt_server(self, world, source, amount);
        if !was_hurt {
            return false;
        }
        if let Some(attacker) = source.causing_entity()
            && let Some(attacker_living) = attacker.as_living_entity()
            && Mob::can_attack(self, attacker_living)
        {
            self.set_target(Some(attacker));
        }
        was_hurt
    }
}

impl LivingEntity for ZoglinEntity {
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
        Some(&sound_events::ENTITY_ZOGLIN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ZOGLIN_DEATH)
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

impl Mob for ZoglinEntity {
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
        Some(&sound_events::ENTITY_ZOGLIN_AMBIENT)
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

    /// Vanilla `Zoglin.customServerAiStep`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        let mut brain = self.brain.lock();
        brain::tick_zoglin(&mut brain, &world, entity);
        brain::update_zoglin_activity(&mut brain);
    }

    fn mob_can_be_leashed(&self) -> bool {
        true
    }

    /// Vanilla `Zoglin.finalizeSpawn`: 20% babies.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if rand::random::<f32>() < BABY_SPAWN_CHANCE {
            self.set_baby(true);
        }
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    /// Vanilla `Zoglin.doHurtTarget` plays the attack animation on the shared melee path.
    fn after_hurt_target(&self, _world: &World, target: &SharedEntity, was_hurt: bool) {
        if !was_hurt || target.as_living_entity().is_none() {
            return;
        }
        *self.attack_animation_remaining_ticks.lock() = ATTACK_ANIMATION_TICKS;
        self.play_sound(
            &sound_events::ENTITY_ZOGLIN_ATTACK,
            self.sound_volume(),
            1.0,
        );
    }
}

impl Monster for ZoglinEntity {}

impl PathfinderMob for ZoglinEntity {}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::ZoglinEntity;
    use crate::entity::Entity;

    #[test]
    fn zoglin_baby_flag_round_trips() {
        init_vanilla_registry();
        let zoglin = ZoglinEntity::new(&vanilla_entities::ZOGLIN, 1, DVec3::ZERO, Weak::new());

        assert!(zoglin.is_adult());
        zoglin.set_baby(true);
        assert!(zoglin.is_baby());

        let mut nbt = NbtCompound::new();
        zoglin.save_additional(&mut nbt);
        assert_eq!(nbt.byte("IsBaby"), Some(1));
        assert_eq!(zoglin.entity_type(), &vanilla_entities::ZOGLIN);
    }
}
