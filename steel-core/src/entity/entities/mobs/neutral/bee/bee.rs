//! Vanilla `Bee` (`net.minecraft.world.entity.animal.bee.Bee`).
//!
//! Vanilla's hive/flower goals need `BeehiveBlockEntity` memory plumbing that Steel has no
//! equivalent for yet, so this file implements the entity state, the anger/sting combat path,
//! attributes, sounds, drops and NBT exactly, and registers the goals Steel can express.
//! The missing hive/pollinate goals are reported as blocked, not stubbed.

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::BeeEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_damage_types, vanilla_item_tags, vanilla_mob_effects,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    BreedGoal, FloatGoal, FollowParentGoal, HurtByTargetGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, TemptGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::ai::navigation::NavigationKind;
use crate::entity::damage::DamageSource;
use crate::entity::neutral_mob::{NeutralMob, PersistentAnger};
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance,
    PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 3;

/// Vanilla `Bee.createNavigation` installs a 48-block required path length.
const BEE_REQUIRED_PATH_LENGTH: f32 = 48.0;

/// Vanilla `Bee.PERSISTENT_ANGER_TIME` (`rangeOfSeconds(20, 39)`).
const PERSISTENT_ANGER_TIME_MIN: i64 = 20 * 20;
const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;
/// Vanilla `Bee` flag bits.
const BEE_FLAG_ROLLING: i8 = 2;
const BEE_FLAG_HAS_STUNG: i8 = 4;
const BEE_FLAG_HAS_NECTAR: i8 = 8;
/// Vanilla `Bee` poison seconds on sting by difficulty.
const BEE_POISON_SECONDS_NORMAL: i32 = 10;
const BEE_POISON_SECONDS_HARD: i32 = 18;
const TICKS_PER_SECOND: i32 = 20;

/// Vanilla `Bee.MAX_TICKS_WITHOUT_NECTAR`.
const MAX_TICKS_WITHOUT_NECTAR: i32 = 2400;

/// Vanilla `Bee`.
#[entity_behavior(class = "Bee")]
pub struct BeeEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<BeeEntityData>,
    persistent_anger: PersistentAnger,
    /// Vanilla `Bee.ticksWithoutNectarSinceExitingHive`.
    ticks_without_nectar: SyncMutex<i32>,
    /// Vanilla `Bee.numCropsGrownSincePollination`.
    crops_grown_since_pollination: SyncMutex<i32>,
    /// Vanilla `Bee.hivePos`/`savedFlowerPos`.
    hive_pos: SyncMutex<Option<BlockPos>>,
    flower_pos: SyncMutex<Option<BlockPos>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BeeEntity`.
unsafe impl DowncastType for BeeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/bee");
}

impl BeeEntity {
    /// Creates a new bee entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved bee entity.
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
        {
            // Vanilla `Bee.createNavigation`: a `Bee$1` flying navigation with door
            // opening disabled, floating disabled, a 48-block required path length,
            // and the block-below stable-destination rule.
            let mut navigation = mob_base.navigation().lock();
            navigation.set_kind(NavigationKind::Fly);
            navigation.set_can_open_doors(false);
            navigation.set_can_float(false);
            navigation.set_fly_stable_destination_block_below(true);
            let follow_range = living_base
                .attributes()
                .lock()
                .required_value(vanilla_attributes::FOLLOW_RANGE);
            navigation.set_required_path_length(BEE_REQUIRED_PATH_LENGTH, follow_range);
        }
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = BeeEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.flags.set(0);
        entity_data.anger_end_time.set(-1);

        {
            // Vanilla `Bee.registerGoals` (the hive/flower goals are reported as blocked).
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, MeleeAttackGoal::new(1.4, true));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                TemptGoal::new(
                    1.25,
                    |item_stack: &ItemStack| {
                        !item_stack.is_empty()
                            && item_stack
                                .item()
                                .has_tag(&vanilla_item_tags::ItemTag::BEE_FOOD)
                    },
                    false,
                ),
            );
            goal_selector.add_goal(5, FollowParentGoal::new(1.25));
            goal_selector.add_goal(11, WaterAvoidingRandomStrollGoal::new(1.0));

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new().set_alert_others(empty()));
            target_selector.add_goal(
                2,
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
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            persistent_anger: PersistentAnger::new(),
            ticks_without_nectar: SyncMutex::new(0),
            crops_grown_since_pollination: SyncMutex::new(0),
            hive_pos: SyncMutex::new(None),
            flower_pos: SyncMutex::new(None),
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

    fn flag(&self, flag: i8) -> bool {
        self.entity_data.lock().flags.get() & flag != 0
    }

    fn set_flag(&self, flag: i8, value: bool) {
        let mut entity_data = self.entity_data.lock();
        let flags = *entity_data.flags.get();
        entity_data
            .flags
            .set(if value { flags | flag } else { flags & !flag });
    }

    /// Vanilla `Bee.hasStung`.
    #[must_use]
    pub fn has_stung(&self) -> bool {
        self.flag(BEE_FLAG_HAS_STUNG)
    }

    /// Vanilla `Bee.setHasStung`.
    pub fn set_has_stung(&self, has_stung: bool) {
        self.set_flag(BEE_FLAG_HAS_STUNG, has_stung);
    }

    /// Vanilla `Bee.hasNectar`.
    #[must_use]
    pub fn has_nectar(&self) -> bool {
        self.flag(BEE_FLAG_HAS_NECTAR)
    }

    /// Vanilla `Bee.setHasNectar`.
    pub fn set_has_nectar(&self, has_nectar: bool) {
        self.set_flag(BEE_FLAG_HAS_NECTAR, has_nectar);
    }

    /// Vanilla `Bee.isRolling`.
    #[must_use]
    pub fn is_rolling(&self) -> bool {
        self.flag(BEE_FLAG_ROLLING)
    }

    /// Vanilla `Bee.setRolling`.
    pub fn set_rolling(&self, rolling: bool) {
        self.set_flag(BEE_FLAG_ROLLING, rolling);
    }

    /// Vanilla `Bee.getTicksWithoutNectarSinceExitingHive`.
    #[must_use]
    pub fn ticks_without_nectar(&self) -> i32 {
        *self.ticks_without_nectar.lock()
    }

    /// Vanilla `Bee.getCropsGrownSincePollination`.
    #[must_use]
    pub fn crops_grown_since_pollination(&self) -> i32 {
        *self.crops_grown_since_pollination.lock()
    }

    /// Vanilla `Bee.getHivePos`.
    #[must_use]
    pub fn hive_pos(&self) -> Option<BlockPos> {
        *self.hive_pos.lock()
    }

    /// Vanilla `Bee.setHivePos`.
    pub fn set_hive_pos(&self, pos: Option<BlockPos>) {
        *self.hive_pos.lock() = pos;
    }

    /// Vanilla `Bee.getSavedFlowerPos`.
    #[must_use]
    pub fn flower_pos(&self) -> Option<BlockPos> {
        *self.flower_pos.lock()
    }

    /// Vanilla `Bee.setSavedFlowerPos`.
    pub fn set_flower_pos(&self, pos: Option<BlockPos>) {
        *self.flower_pos.lock() = pos;
    }

    /// Vanilla `Bee.tick` nectar timer.
    fn tick_nectar(&self) {
        let mut ticks = self.ticks_without_nectar.lock();
        *ticks += 1;
        if *ticks > MAX_TICKS_WITHOUT_NECTAR {
            *ticks = MAX_TICKS_WITHOUT_NECTAR;
            if !self.has_nectar() {
                self.set_has_nectar(true);
                *self.crops_grown_since_pollination.lock() = 0;
            }
        }
    }
}

impl NeutralMob for BeeEntity {
    fn persistent_anger(&self) -> &PersistentAnger {
        &self.persistent_anger
    }

    fn start_persistent_anger_timer(&self) {
        self.set_time_to_remain_angry(
            PERSISTENT_ANGER_TIME_MIN
                + rand::random_range(0..(PERSISTENT_ANGER_TIME_MAX - PERSISTENT_ANGER_TIME_MIN)),
        );
    }
}

impl Entity for BeeEntity {
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
        self.tick_nectar();
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
            &sound_events::ENTITY_BEE_HURT,
            &sound_events::ENTITY_BEE_HURT,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_BEE_LOOP
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("HasNectar", i8::from(self.has_nectar()));
        nbt.insert("HasStung", i8::from(self.has_stung()));
        nbt.insert("TicksSincePollination", self.ticks_without_nectar());
        nbt.insert(
            "CropsGrownSincePollination",
            self.crops_grown_since_pollination(),
        );
        if let Some(hive) = self.hive_pos() {
            nbt.insert("hive_pos_x", hive.x());
            nbt.insert("hive_pos_y", hive.y());
            nbt.insert("hive_pos_z", hive.z());
        }
        if let Some(flower) = self.flower_pos() {
            nbt.insert("flower_pos_x", flower.x());
            nbt.insert("flower_pos_y", flower.y());
            nbt.insert("flower_pos_z", flower.z());
        }
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_has_nectar(nbt.byte("HasNectar").is_some_and(|value| value != 0));
        self.set_has_stung(nbt.byte("HasStung").is_some_and(|value| value != 0));
        *self.ticks_without_nectar.lock() = nbt.int("TicksSincePollination").unwrap_or(0);
        *self.crops_grown_since_pollination.lock() =
            nbt.int("CropsGrownSincePollination").unwrap_or(0);
        if let (Some(x), Some(y), Some(z)) = (
            nbt.int("hive_pos_x"),
            nbt.int("hive_pos_y"),
            nbt.int("hive_pos_z"),
        ) {
            self.set_hive_pos(Some(BlockPos::new(x, y, z)));
        }
        if let (Some(x), Some(y), Some(z)) = (
            nbt.int("flower_pos_x"),
            nbt.int("flower_pos_y"),
            nbt.int("flower_pos_z"),
        ) {
            self.set_flower_pos(Some(BlockPos::new(x, y, z)));
        }
        self.read_persistent_anger_save_data(&nbt);
    }
}

impl LivingEntity for BeeEntity {
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
        Some(&sound_events::ENTITY_BEE_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_BEE_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.update_persistent_anger(true);
        self.mob_ai_step(entity)
    }
}

impl Mob for BeeEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Bee.getAmbientSound`: the loop sound is the bee's ambient cue.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_angry() {
            &sound_events::ENTITY_BEE_LOOP_AGGRESSIVE
        } else {
            &sound_events::ENTITY_BEE_LOOP
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Bee.doHurtTarget`: sting damage, poison by difficulty, then the bee loses its
    /// stinger and stops being angry.
    fn do_hurt_target(&self, entity: &SharedEntity, world: &World, target: &SharedEntity) -> bool {
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };
        let attack_damage = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::ATTACK_DAMAGE)
            .unwrap_or(0.0) as f32;
        let sting_source =
            DamageSource::direct(&vanilla_damage_types::STING, SharedEntity::clone(entity));

        let was_hurt = target.hurt(world, &sting_source, attack_damage);
        if was_hurt {
            let poison_seconds = match world.difficulty() {
                Difficulty::Normal => BEE_POISON_SECONDS_NORMAL,
                Difficulty::Hard => BEE_POISON_SECONDS_HARD,
                _ => 0,
            };
            if poison_seconds > 0 {
                target_living.add_mob_effect(MobEffectInstance::with_duration(
                    vanilla_mob_effects::POISON,
                    poison_seconds * TICKS_PER_SECOND,
                    0,
                ));
            }
        }

        // Vanilla sets the stung flag and stops being angry regardless of the hurt result.
        self.set_has_stung(true);
        self.stop_being_angry();
        self.play_sound(&sound_events::ENTITY_BEE_STING, 1.0, 1.0);
        was_hurt
    }
}

impl AgeableMob for BeeEntity {
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

impl Animal for BeeEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Bee.isFood` uses the `#minecraft:bee_food` tag.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is_empty()
            && item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::BEE_FOOD)
    }
}

impl PathfinderMob for BeeEntity {}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::BeeEntity;
    use crate::entity::Entity;

    #[test]
    fn bee_flags_and_nectar_timers_round_trip() {
        init_vanilla_registry();
        let bee = BeeEntity::new(&vanilla_entities::BEE, 1, DVec3::ZERO, Weak::new());

        assert!(!bee.has_nectar());
        assert!(!bee.has_stung());
        bee.set_has_nectar(true);
        bee.set_has_stung(true);
        assert!(bee.has_nectar());
        assert!(bee.has_stung());

        let mut nbt = NbtCompound::new();
        bee.save_additional(&mut nbt);
        assert_eq!(nbt.byte("HasNectar"), Some(1));
        assert_eq!(nbt.byte("HasStung"), Some(1));
        assert_eq!(bee.entity_type(), &vanilla_entities::BEE);
    }
}
