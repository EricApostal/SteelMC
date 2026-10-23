//! Vanilla `Hoglin` (`net.minecraft.world.entity.monster.hoglin.Hoglin`).
//!
//! Vanilla is brain-driven (`HoglinAi`, owned by the brain slice) and extends `Animal`; Steel
//! implements it as an [`Animal`] + [`AgeableMob`] with the shared brain hook.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::HoglinEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_item_tags};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::brain::Brain;
use crate::entity::conversion::ConversionParams;
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    MobEffectInstance, Monster, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

use super::brain;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Hoglin` adult XP reward.
const XP_REWARD_ADULT: i32 = 5;
/// Vanilla `Hoglin` baby XP reward.
const XP_REWARD_BABY: i32 = 3;
/// Vanilla `Hoglin` baby attack damage.
const BABY_ATTACK_DAMAGE: f64 = 0.5;
/// Vanilla `Hoglin` adult attack damage.
const ADULT_ATTACK_DAMAGE: f64 = 6.0;
/// Vanilla `Hoglin.ATTACK_KNOCKBACK`.
const ATTACK_ANIMATION_TICKS: i32 = 10;
/// Vanilla `Hoglin.CONVERSION_TIME`.
const CONVERSION_TIME: i32 = 300;
/// Vanilla `Hoglin` baby spawn chance.
const BABY_SPAWN_CHANCE: f32 = 0.2;
/// Vanilla `Hoglin` poses scale factor for the baby hitbox.
const HOGLIN_BABY_WIDTH: f32 = 0.75;
const HOGLIN_BABY_HEIGHT: f32 = 0.85;
const HOGLIN_BABY_EYE_HEIGHT: f32 = 0.625;

/// Vanilla `Hoglin`.
#[entity_behavior(class = "Hoglin")]
pub struct HoglinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<HoglinEntityData>,
    brain: SyncMutex<Brain>,
    /// Vanilla `Hoglin.attackAnimationRemainingTicks`.
    attack_animation_remaining_ticks: SyncMutex<i32>,
    /// Vanilla `Hoglin.timeInOverworld`.
    time_in_overworld: SyncMutex<i32>,
    /// Vanilla `Hoglin.cannotBeHunted`.
    cannot_be_hunted: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `HoglinEntity`.
unsafe impl DowncastType for HoglinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/hoglin");
}

impl HoglinEntity {
    /// Creates a new hoglin entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved hoglin entity.
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
        mob_base.set_xp_reward(XP_REWARD_ADULT);
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = HoglinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.immune_to_zombification.set(false);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(brain::make_hoglin_brain()),
            attack_animation_remaining_ticks: SyncMutex::new(0),
            time_in_overworld: SyncMutex::new(0),
            cannot_be_hunted: SyncMutex::new(false),
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

    /// Vanilla `Hoglin.isAdult`.
    #[must_use]
    pub fn is_adult(&self) -> bool {
        !AgeableMob::is_baby(self)
    }

    /// Vanilla `Hoglin.isImmuneToZombification`.
    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        *self.entity_data.lock().immune_to_zombification.get()
    }

    /// Vanilla `Hoglin.setImmuneToZombification`.
    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.entity_data.lock().immune_to_zombification.set(immune);
    }

    /// Vanilla `Hoglin.cannotBeHunted` (set by `PiglinAi.dontKillAnyMoreHoglinsForAWhile`).
    #[must_use]
    pub fn cannot_be_hunted(&self) -> bool {
        *self.cannot_be_hunted.lock()
    }

    /// Sets vanilla `Hoglin.cannotBeHunted`.
    pub fn set_cannot_be_hunted(&self, cannot_be_hunted: bool) {
        *self.cannot_be_hunted.lock() = cannot_be_hunted;
    }

    /// Vanilla `Hoglin.isConverting` (`!isImmuneToZombification && !isPersistenceRequired`).
    #[must_use]
    fn is_converting(&self) -> bool {
        !self.is_immune_to_zombification() && !self.is_persistence_required()
    }

    /// Vanilla `Hoglin.finishConversion`.
    fn finish_conversion(&self) {
        if let Some(_zoglin) = self.convert_to(
            &vanilla_entities::ZOGLIN,
            ConversionParams::single(true, false),
            Some(&|converted| {
                converted.add_mob_effect(MobEffectInstance::with_duration(
                    &steel_registry::vanilla_mob_effects::NAUSEA,
                    200,
                    0,
                ));
            }),
        ) {
            self.play_sound(
                &sound_events::ENTITY_HOGLIN_CONVERTED_TO_ZOMBIFIED,
                self.sound_volume(),
                1.0,
            );
        }
    }
}

impl Entity for HoglinEntity {
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

    /// Vanilla `Hoglin.getDefaultDimensions` uses the baby hitbox for babies.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            EntityDimensions::new(
                HOGLIN_BABY_WIDTH,
                HOGLIN_BABY_HEIGHT,
                HOGLIN_BABY_EYE_HEIGHT,
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
        self.play_sound(&sound_events::ENTITY_HOGLIN_STEP, 0.15, 1.0);
    }

    /// Vanilla `Hoglin.getWalkTargetValue` favors crimson nylium and avoids repellents.

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert(
            "IsImmuneToZombification",
            i8::from(self.is_immune_to_zombification()),
        );
        nbt.insert("TimeInOverworld", *self.time_in_overworld.lock());
        nbt.insert("CannotBeHunted", i8::from(self.cannot_be_hunted()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_immune_to_zombification(
            nbt.byte("IsImmuneToZombification")
                .is_some_and(|value| value != 0),
        );
        *self.time_in_overworld.lock() = nbt.int("TimeInOverworld").unwrap_or(0);
        self.set_cannot_be_hunted(nbt.byte("CannotBeHunted").is_some_and(|value| value != 0));
    }
}

impl LivingEntity for HoglinEntity {
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
        Some(&sound_events::ENTITY_HOGLIN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_HOGLIN_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        true
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

impl Mob for HoglinEntity {
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
        Some(&sound_events::ENTITY_HOGLIN_AMBIENT)
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

    /// Vanilla `Hoglin.customServerAiStep`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        {
            let mut brain = self.brain.lock();
            brain::tick_hoglin(&mut brain, &world, entity);
            brain::update_hoglin_activity(&mut brain);
        }

        if self.is_converting() {
            let mut time = self.time_in_overworld.lock();
            *time += 1;
            if *time > CONVERSION_TIME {
                drop(time);
                self.finish_conversion();
            }
        } else {
            *self.time_in_overworld.lock() = 0;
        }
    }

    /// Vanilla `Hoglin.getBaseExperienceReward` returns `xpReward` (baby: 3, adult: 5).
    fn base_experience_reward_mob(&self) -> i32 {
        if self.is_adult() {
            XP_REWARD_ADULT
        } else {
            XP_REWARD_BABY
        }
    }

    fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        self.is_food(item_stack)
    }

    fn mob_can_be_leashed(&self) -> bool {
        true
    }

    /// Vanilla `Hoglin.removeWhenFarAway` is always true.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        true
    }

    /// Vanilla `Hoglin.finalizeSpawn`: 20% babies.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if rand::random::<f32>() < BABY_SPAWN_CHANCE {
            self.set_baby(true);
        }
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    /// Vanilla `Hoglin.checkHoglinSpawnRules`: never above nether wart blocks.
    fn check_mob_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &dyn crate::world::LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        if level.get_block_state(pos.offset(0, -1, 0)).get_block()
            == &steel_registry::vanilla_blocks::NETHER_WART_BLOCK
        {
            return false;
        }
        <Self as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
    }

    /// Vanilla `Hoglin.doHurtTarget` plays the attack animation on the shared melee path.
    fn after_hurt_target(&self, _world: &World, target: &SharedEntity, was_hurt: bool) {
        if !was_hurt || target.as_living_entity().is_none() {
            return;
        }
        *self.attack_animation_remaining_ticks.lock() = ATTACK_ANIMATION_TICKS;
        self.play_sound(
            &sound_events::ENTITY_HOGLIN_ATTACK,
            self.sound_volume(),
            1.0,
        );
    }
}

impl Monster for HoglinEntity {}

impl AgeableMob for HoglinEntity {
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

    /// Vanilla `Hoglin.ageBoundaryReached`.
    fn age_boundary_changed(&self, baby: bool) {
        if baby {
            self.set_xp_reward(XP_REWARD_BABY);
            self.attributes()
                .lock()
                .set_base_value(vanilla_attributes::ATTACK_DAMAGE, BABY_ATTACK_DAMAGE);
        } else {
            self.set_xp_reward(XP_REWARD_ADULT);
            self.attributes()
                .lock()
                .set_base_value(vanilla_attributes::ATTACK_DAMAGE, ADULT_ATTACK_DAMAGE);
        }
    }
}

impl Animal for HoglinEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Hoglin.isFood` uses the `#minecraft:hoglin_food` tag.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is_empty()
            && item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::HOGLIN_FOOD)
    }
}

impl PathfinderMob for HoglinEntity {
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        if let Some(world) = self.level() {
            // BLOCKED(HoglinAi.isPosNearNearestRepellent): the repellent check lives in the
            // brain/`HoglinSpecificSensor` half; without a memory read here only the terrain
            // preference is applied.
            if world.get_block_state(pos.offset(0, -1, 0)).get_block()
                == &steel_registry::vanilla_blocks::CRIMSON_NYLIUM
            {
                return 10.0;
            }
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::HoglinEntity;
    use crate::entity::ageable::AgeableMob;
    use crate::entity::{Animal, Entity, LivingEntity};

    #[test]
    fn hoglin_age_boundary_updates_damage_and_nbt() {
        init_vanilla_registry();
        let hoglin = HoglinEntity::new(&vanilla_entities::HOGLIN, 1, DVec3::ZERO, Weak::new());

        assert!(hoglin.is_adult());
        hoglin.set_baby(true);
        assert!(!hoglin.is_adult());
        assert_eq!(
            hoglin
                .attributes()
                .lock()
                .get_value(steel_registry::vanilla_attributes::ATTACK_DAMAGE),
            Some(0.5)
        );

        hoglin.set_immune_to_zombification(true);
        let mut nbt = simdnbt::owned::NbtCompound::new();
        hoglin.save_additional(&mut nbt);
        assert_eq!(nbt.byte("IsImmuneToZombification"), Some(1));
        assert_eq!(hoglin.entity_type(), &vanilla_entities::HOGLIN);
    }
}
