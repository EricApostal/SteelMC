//! Vanilla `Fox` (`net.minecraft.world.entity.animal.fox.Fox`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::FoxEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_item_tags};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    AvoidEntityGoal, BreedGoal, ClimbOnTopOfPowderSnowGoal, FloatGoal, FollowParentGoal,
    LeapAtTargetGoal, MeleeAttackGoal, PanicGoal, RandomLookAroundGoal,
    WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 1;
/// Vanilla `Fox` sleeping/defending flags inside `DATA_FLAGS_ID`.
const FOX_FLAG_SITTING: i8 = 1;
const FOX_FLAG_CROUCHING: i8 = 2;
const FOX_FLAG_INTERESTED: i8 = 4;
const FOX_FLAG_POUNCING: i8 = 8;
const FOX_FLAG_SLEEPING: i8 = 16;
const FOX_FLAG_FACEPLANTED: i8 = 32;
const FOX_FLAG_DEFENDING: i8 = 64;
/// Vanilla `Fox.Variant` values.
const FOX_VARIANT_RED: i32 = 0;
const FOX_VARIANT_SNOW: i32 = 1;

/// Vanilla `Fox.Variant`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FoxVariant {
    /// The default red fox.
    #[default]
    Red,
    /// The snowy taiga fox.
    Snow,
}

impl FoxVariant {
    /// Vanilla `Variant.byId`.
    #[must_use]
    pub const fn by_id(id: i32) -> Self {
        if id == FOX_VARIANT_SNOW {
            Self::Snow
        } else {
            Self::Red
        }
    }

    /// Vanilla `Variant.getId`.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Red => FOX_VARIANT_RED,
            Self::Snow => FOX_VARIANT_SNOW,
        }
    }

    /// Vanilla `Variant.getByName`.
    #[must_use]
    pub fn by_name(name: &str) -> Self {
        if name == "snow" {
            Self::Snow
        } else {
            Self::Red
        }
    }

    /// Vanilla `Variant.getName`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Snow => "snow",
        }
    }
}

/// Vanilla `Fox`.
#[entity_behavior(class = "Fox")]
pub struct FoxEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<FoxEntityData>,
    /// Vanilla `Fox.isDefending`.
    defending: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `FoxEntity`.
unsafe impl DowncastType for FoxEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/fox");
}

impl FoxEntity {
    /// Creates a new fox entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved fox entity.
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
        let mut entity_data = FoxEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.variant_type.set(FOX_VARIANT_RED);
        entity_data.flags.set(0);

        {
            // Vanilla `Fox.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(0, ClimbOnTopOfPowderSnowGoal::new());
            goal_selector.add_goal(2, PanicGoal::new(2.2));
            goal_selector.add_goal(3, BreedGoal::new(1.0));
            goal_selector.add_goal(
                4,
                AvoidEntityGoal::with_selector(8.0, 1.6, 1.4, |target, _world| {
                    target.entity_type() == &vanilla_entities::WOLF
                }),
            );
            goal_selector.add_goal(
                4,
                AvoidEntityGoal::with_selector(8.0, 1.6, 1.4, |target, _world| {
                    target.entity_type() == &vanilla_entities::POLAR_BEAR
                }),
            );
            goal_selector.add_goal(7, MeleeAttackGoal::new(1.2, true));
            goal_selector.add_goal(8, FollowParentGoal::new(1.25));
            goal_selector.add_goal(10, LeapAtTargetGoal::new(0.4));
            goal_selector.add_goal(11, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(12, RandomLookAroundGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            defending: SyncMutex::new(false),
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

    fn flags(&self) -> i8 {
        *self.entity_data.lock().flags.get()
    }

    fn set_flag(&self, flag: i8, value: bool) {
        let mut entity_data = self.entity_data.lock();
        let flags = *entity_data.flags.get();
        entity_data
            .flags
            .set(if value { flags | flag } else { flags & !flag });
    }

    /// Vanilla `Fox.getVariant`.
    #[must_use]
    pub fn variant(&self) -> FoxVariant {
        FoxVariant::by_id(*self.entity_data.lock().variant_type.get())
    }

    /// Vanilla `Fox.setVariant`.
    pub fn set_variant(&self, variant: FoxVariant) {
        self.entity_data.lock().variant_type.set(variant.id());
    }

    /// Vanilla `Fox.isDefending`.
    #[must_use]
    pub fn is_defending(&self) -> bool {
        *self.defending.lock()
    }

    /// Vanilla `Fox.setDefending`.
    pub fn set_defending(&self, defending: bool) {
        *self.defending.lock() = defending;
        self.set_flag(FOX_FLAG_DEFENDING, defending);
    }

    /// Vanilla `Fox.isSleeping` on the fox flag byte.
    #[must_use]
    pub fn is_sleeping(&self) -> bool {
        self.flags() & FOX_FLAG_SLEEPING != 0
    }

    /// Vanilla `Fox.setSleeping`.
    pub fn set_sleeping(&self, sleeping: bool) {
        self.set_flag(FOX_FLAG_SLEEPING, sleeping);
    }

    /// Vanilla `Fox.isSitting`.
    #[must_use]
    pub fn is_sitting(&self) -> bool {
        self.flags() & FOX_FLAG_SITTING != 0
    }

    /// Vanilla `Fox.setSitting`.
    pub fn set_sitting(&self, sitting: bool) {
        self.set_flag(FOX_FLAG_SITTING, sitting);
    }

    /// Vanilla `Fox.isPouncing`.
    #[must_use]
    pub fn is_pouncing(&self) -> bool {
        self.flags() & FOX_FLAG_POUNCING != 0
    }

    /// Vanilla `Fox.setPouncing`.
    pub fn set_pouncing(&self, pouncing: bool) {
        self.set_flag(FOX_FLAG_POUNCING, pouncing);
    }

    /// Vanilla `Fox.isCrouching`.
    #[must_use]
    pub fn is_crouching(&self) -> bool {
        self.flags() & FOX_FLAG_CROUCHING != 0
    }

    /// Vanilla `Fox.setCrouching`.
    pub fn set_crouching(&self, crouching: bool) {
        self.set_flag(FOX_FLAG_CROUCHING, crouching);
    }

    /// Vanilla `Fox.isInterested`.
    #[must_use]
    pub fn is_interested(&self) -> bool {
        self.flags() & FOX_FLAG_INTERESTED != 0
    }

    /// Vanilla `Fox.setIsInterested`.
    pub fn set_is_interested(&self, interested: bool) {
        self.set_flag(FOX_FLAG_INTERESTED, interested);
    }

    /// Vanilla `Fox.isFaceplanted`.
    #[must_use]
    pub fn is_faceplanted(&self) -> bool {
        self.flags() & FOX_FLAG_FACEPLANTED != 0
    }

    /// Vanilla `Fox.setFaceplanted`.
    pub fn set_faceplanted(&self, faceplanted: bool) {
        self.set_flag(FOX_FLAG_FACEPLANTED, faceplanted);
    }

    /// Vanilla `Fox.trusts`: a player whose UUID is in the trusted list.
    #[must_use]
    pub fn trusts(&self, uuid: uuid::Uuid) -> bool {
        let entity_data = self.entity_data.lock();
        *entity_data.trusted_id_0.get() == Some(uuid)
            || *entity_data.trusted_id_1.get() == Some(uuid)
    }
}

impl Entity for FoxEntity {
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
        SoundSource::Neutral
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        (
            &sound_events::ENTITY_FOX_AMBIENT,
            &sound_events::ENTITY_FOX_AMBIENT,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_FOX_AMBIENT
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        // Vanilla `Fox.playStepSound` uses `getSoundSet().stepSound()`; the classic set is the
        // generic fox step, which has no dedicated sound event, so the ambient sound stands in.
        self.play_sound(&sound_events::ENTITY_FOX_AMBIENT, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("Type", self.variant().name());
        nbt.insert("Trusted", {
            let mut list = Vec::new();
            let entity_data = self.entity_data.lock();
            if let Some(uuid) = entity_data.trusted_id_0.get() {
                list.push(uuid.to_string());
            }
            if let Some(uuid) = entity_data.trusted_id_1.get() {
                list.push(uuid.to_string());
            }
            list
        });
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        if let Some(variant) = nbt.string("Type") {
            self.set_variant(FoxVariant::by_name(variant.to_str().as_ref()));
        }
    }
}

impl LivingEntity for FoxEntity {
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
        Some(&sound_events::ENTITY_FOX_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_FOX_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.mob_ai_step(entity)
    }
}

impl Mob for FoxEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Fox.getAmbientSound`: sleeping foxes use the sleep sound.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_sleeping() {
            &sound_events::ENTITY_FOX_SLEEP
        } else {
            &sound_events::ENTITY_FOX_AMBIENT
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Fox.finalizeSpawn`: snowy foxes in snowy biomes.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla picks `Variant.SNOW` for snowy biomes and inherits otherwise.
        if let Some(biome) = world.biome_at(self.block_position())
            && biome.has_tag(&steel_utils::Identifier::vanilla_static(
                "spawns_snow_foxes",
            ))
        {
            self.set_variant(FoxVariant::Snow);
        }
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }
}

impl AgeableMob for FoxEntity {
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

impl Animal for FoxEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Fox.isFood` uses the `#minecraft:fox_food` tag.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is_empty()
            && item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::FOX_FOOD)
    }
}

impl PathfinderMob for FoxEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::{FoxEntity, FoxVariant};
    use crate::entity::Entity;

    #[test]
    fn fox_variant_and_flags_round_trip() {
        init_vanilla_registry();
        let fox = FoxEntity::new(&vanilla_entities::FOX, 1, DVec3::ZERO, Weak::new());

        assert_eq!(fox.variant(), FoxVariant::Red);
        fox.set_variant(FoxVariant::Snow);
        assert_eq!(fox.variant(), FoxVariant::Snow);

        assert!(!fox.is_sleeping());
        fox.set_sleeping(true);
        assert!(fox.is_sleeping());
        fox.set_sleeping(false);
        assert!(!fox.is_sleeping());

        let mut nbt = simdnbt::owned::NbtCompound::new();
        fox.save_additional(&mut nbt);
        assert!(nbt.string("Type").is_some());
        assert_eq!(fox.entity_type(), &vanilla_entities::FOX);
    }
}
