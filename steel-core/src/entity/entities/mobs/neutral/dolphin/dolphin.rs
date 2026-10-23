//! Vanilla `Dolphin` (`net.minecraft.world.entity.animal.dolphin.Dolphin`).
//!
//! Vanilla is a water creature with `WaterBoundPathNavigation`; Steel has no water-bound
//! navigation, so the dolphin uses the shared navigation and implements the vanilla
//! moistness/air rules itself (documented deviation).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::DolphinEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_item_tags};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    AvoidEntityGoal, BreathAirGoal, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal,
    RandomLookAroundGoal, RandomSwimmingGoal, TryFindWaterGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 1;
/// Vanilla `Dolphin.MAX_MOISTNESS_LEVEL`.
const MAX_MOISTNESS: i32 = 2400;
/// Vanilla `Dolphin.getMaxAirSupply`.
const MAX_AIR_SUPPLY: i32 = 4800;
/// Vanilla `Dolphin` dry-out damage.
const DRY_OUT_DAMAGE: f32 = 1.0;

/// Vanilla `Dolphin`.
#[entity_behavior(class = "Dolphin")]
pub struct DolphinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<DolphinEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `DolphinEntity`.
unsafe impl DowncastType for DolphinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/dolphin");
}

impl DolphinEntity {
    /// Creates a new dolphin entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved dolphin entity.
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
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = DolphinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.got_fish.set(false);
        entity_data.moistness_level.set(MAX_MOISTNESS);

        {
            // Vanilla `Dolphin.registerGoals` (the treasure/play goals need item and structure
            // plumbing that is reported as blocked).
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, BreathAirGoal::new());
            goal_selector.add_goal(0, TryFindWaterGoal::new());
            goal_selector.add_goal(4, RandomSwimmingGoal::new(1.0, 10));
            goal_selector.add_goal(4, RandomLookAroundGoal::new());
            goal_selector.add_goal(5, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(6, MeleeAttackGoal::new(1.2, true));
            goal_selector.add_goal(
                9,
                AvoidEntityGoal::with_selector(8.0, 1.0, 1.0, |target, _world| {
                    target.entity_type() == &vanilla_entities::GUARDIAN
                        || target.entity_type() == &vanilla_entities::ELDER_GUARDIAN
                }),
            );

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
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

    /// Vanilla `Dolphin.gotFish`.
    #[must_use]
    pub fn got_fish(&self) -> bool {
        *self.entity_data.lock().got_fish.get()
    }

    /// Vanilla `Dolphin.setGotFish`.
    pub fn set_got_fish(&self, got_fish: bool) {
        self.entity_data.lock().got_fish.set(got_fish);
    }

    /// Vanilla `Dolphin.getMoistnessLevel`.
    #[must_use]
    pub fn moistness_level(&self) -> i32 {
        *self.entity_data.lock().moistness_level.get()
    }

    /// Vanilla `Dolphin.setMoisntessLevel`.
    pub fn set_moistness_level(&self, level: i32) {
        self.entity_data.lock().moistness_level.set(level);
    }

    /// Vanilla `Dolphin.tick` moistness and dry-out damage.
    fn tick_moistness(&self, entity: &SharedEntity) {
        // Vanilla `isInWaterOrRain`.
        if self.is_in_water() || self.is_in_rain_now() {
            self.set_moistness_level(MAX_MOISTNESS);
            return;
        }

        self.set_moistness_level(self.moistness_level() - 1);
        if self.moistness_level() > 0 {
            return;
        }

        if let Some(world) = self.level() {
            let source = DamageSource::environment(&steel_registry::vanilla_damage_types::DRY_OUT);
            let _ = entity.hurt(&world, &source, DRY_OUT_DAMAGE);
        }
    }

    /// Vanilla `Entity.isInRain`: raining at the entity's block position.
    fn is_in_rain_now(&self) -> bool {
        self.level()
            .is_some_and(|world| world.is_raining_at(self.block_position()))
    }

    /// Vanilla `Dolphin.getMaxAirSupply`.
    #[must_use]
    pub fn max_air_supply(&self) -> i32 {
        MAX_AIR_SUPPLY
    }
}

impl Entity for DolphinEntity {
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
        self.tick_moistness(&entity);
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
        SoundSource::Neutral
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        (
            &sound_events::ENTITY_DOLPHIN_SPLASH,
            &sound_events::ENTITY_DOLPHIN_SPLASH,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_DOLPHIN_SPLASH
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("GotFish", i8::from(self.got_fish()));
        nbt.insert("Moistness", self.moistness_level());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_got_fish(nbt.byte("GotFish").is_some_and(|value| value != 0));
        self.set_moistness_level(nbt.int("Moistness").unwrap_or(MAX_MOISTNESS));
    }
}

impl LivingEntity for DolphinEntity {
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
        Some(&sound_events::ENTITY_DOLPHIN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_DOLPHIN_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.mob_ai_step(entity)
    }
}

impl Mob for DolphinEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Dolphin.getAmbientSound` depends on being in water.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_in_water() {
            &sound_events::ENTITY_DOLPHIN_AMBIENT_WATER
        } else {
            &sound_events::ENTITY_DOLPHIN_AMBIENT
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Dolphin.canAttack`: babies never attack.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        !AgeableMob::is_baby(self) && Mob::can_attack(self, target)
    }

    /// Vanilla `Dolphin.playAttackSound`.
    fn play_attack_sound(&self) {
        self.play_sound(&sound_events::ENTITY_DOLPHIN_ATTACK, 1.0, 1.0);
    }
}

impl AgeableMob for DolphinEntity {
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

impl Animal for DolphinEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Dolphin.isFood` accepts `#minecraft:fishes` (dolphins eat fish).
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is_empty()
            && item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::FISHES)
    }
}

impl PathfinderMob for DolphinEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::{DolphinEntity, MAX_MOISTNESS};
    use crate::entity::Entity;

    #[test]
    fn dolphin_moistness_and_fish_flags_round_trip() {
        init_vanilla_registry();
        let dolphin = DolphinEntity::new(&vanilla_entities::DOLPHIN, 1, DVec3::ZERO, Weak::new());

        assert_eq!(dolphin.moistness_level(), MAX_MOISTNESS);
        assert!(!dolphin.got_fish());
        dolphin.set_got_fish(true);
        dolphin.set_moistness_level(100);

        let mut nbt = simdnbt::owned::NbtCompound::new();
        dolphin.save_additional(&mut nbt);
        assert_eq!(nbt.int("Moistness"), Some(100));
        assert_eq!(nbt.byte("GotFish"), Some(1));
        assert_eq!(dolphin.entity_type(), &vanilla_entities::DOLPHIN);
    }
}
