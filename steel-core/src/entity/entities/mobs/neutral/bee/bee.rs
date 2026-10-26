//! Vanilla `Bee` (`net.minecraft.world.entity.animal.bee.Bee`).
//!
//! The entity owns the vanilla lifecycle state (`hivePos`, `savedFlowerPos`, the nectar/crop
//! counters, the hive and flower locate cooldowns, the stay-out countdown, the sting timer and
//! the drowning counter); [`super::goals`] implements the hive/flower goal set that drives it.
//!
//! Deviations:
//! - Vanilla `Bee.customServerAiStep` reads `EnvironmentAttributes.BEES_STAY_IN_HIVE`; Steel has
//!   no environment-attribute system (owner: `world`), so it always reads false.
//! - Vanilla `Bee.hurtServer` stops pollination before `super.hurtServer`; Steel's
//!   `LivingEntity::hurt_server` is a single trait body with no per-mob override hook, so the
//!   cancel happens in `before_actually_hurt` (after the invulnerability and damage-cooldown
//!   gates instead of before them).
//! - `Bee.tick`'s `FALLING_NECTAR` particles and the `rollAmount` rendering state have no Steel
//!   counterpart (owner: `world` particles and the client renderer).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList};
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::BeeEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_damage_types, vanilla_item_tags, vanilla_items,
    vanilla_mob_effects,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::{Difficulty, InteractionHand};
use steel_utils::{BlockPos, BlockStateId, Downcast, DowncastType, DowncastTypeKey};

use super::goals::{
    BeeAttackGoal, BeeBecomeAngryTargetGoal, BeeEnterHiveGoal, BeeGoToHiveGoal,
    BeeGoToKnownFlowerGoal, BeeGrowCropGoal, BeeHurtByOtherGoal, BeeLocateHiveGoal,
    BeePollinateGoal, BeeWanderGoal, HiveBlacklist, PollinateState, STING_DEATH_COUNTDOWN,
    TOO_FAR_DISTANCE, ValidateFlowerGoal, ValidateHiveGoal,
};
use crate::behavior::InteractionResult;
use crate::block_entity::SharedBlockEntity;
use crate::block_entity::entities::BeehiveBlockEntity;
use crate::entity::ai::goal::random_pos::{TENTH_PI, air_random_pos_towards};
use crate::entity::ai::goal::{
    BreedGoal, FloatGoal, FollowParentGoal, ResetUniversalAngerTargetGoal, TemptGoal,
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
use crate::player::Player;
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
/// Vanilla `Bee.MAX_TICKS_WITHOUT_NECTAR` does not exist; the real counter is
/// `Bee.ticksWithoutNectarSinceExitingHive` compared against
/// `TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME`.
const TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME: i32 = 3600;

/// Vanilla `Bee` poison seconds on sting by difficulty.
const BEE_POISON_SECONDS_NORMAL: i32 = 10;
const BEE_POISON_SECONDS_HARD: i32 = 18;
const TICKS_PER_SECOND: i32 = 20;
/// Vanilla `Bee.customServerAiStep` drowning threshold (`underWaterTicks > 20`).
const MAX_UNDERWATER_TICKS: i32 = 20;
/// Vanilla `Bee.customServerAiStep` sting-death roll interval.
const STING_DEATH_ROLL_INTERVAL: i32 = 5;
/// Vanilla `Bee.MIN_ATTACK_DIST`, compared against the squared target distance.
const MIN_ATTACK_DIST: f64 = 4.0;
/// Vanilla `WitherRoseBlock.getBeeInteractionEffect`: `WITHER` for 40 ticks.
const WITHER_ROSE_BEE_EFFECT_TICKS: i32 = 40;

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
    /// Vanilla `Bee.stayOutOfHiveCountdown`.
    stay_out_of_hive_countdown: SyncMutex<i32>,
    /// Vanilla `Bee.remainingCooldownBeforeLocatingNewHive`.
    remaining_cooldown_before_locating_new_hive: SyncMutex<i32>,
    /// Vanilla `Bee.remainingCooldownBeforeLocatingNewFlower`.
    remaining_cooldown_before_locating_new_flower: SyncMutex<i32>,
    /// Vanilla `Bee.timeSinceSting`.
    time_since_sting: SyncMutex<i32>,
    /// Vanilla `Bee.underWaterTicks`.
    underwater_ticks: SyncMutex<i32>,
    /// Shared `Bee.beePollinateGoal.isPollinating()` view (see [`super::goals`]).
    pollinate_state: Arc<SyncMutex<PollinateState>>,
    /// Shared `Bee$BeeGoToHiveGoal.blacklistedTargets` (see [`super::goals`]).
    hive_blacklist: Arc<SyncMutex<HiveBlacklist>>,
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

        let pollinate_state = Arc::new(SyncMutex::new(PollinateState::default()));
        let hive_blacklist = Arc::new(SyncMutex::new(HiveBlacklist::default()));

        {
            // Vanilla `Bee.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, BeeAttackGoal::new(1.4, true));
            goal_selector.add_goal(1, BeeEnterHiveGoal::new());
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
            goal_selector.add_goal(3, ValidateHiveGoal::new());
            goal_selector.add_goal(3, ValidateFlowerGoal::new());
            goal_selector.add_goal(4, BeePollinateGoal::new(Arc::clone(&pollinate_state)));
            goal_selector.add_goal(5, FollowParentGoal::new(1.25));
            goal_selector.add_goal(5, BeeLocateHiveGoal::new(Arc::clone(&hive_blacklist)));
            goal_selector.add_goal(5, BeeGoToHiveGoal::new(Arc::clone(&hive_blacklist)));
            goal_selector.add_goal(6, BeeGoToKnownFlowerGoal::new());
            goal_selector.add_goal(7, BeeGrowCropGoal::new());
            goal_selector.add_goal(8, BeeWanderGoal::new());
            goal_selector.add_goal(9, FloatGoal::new(&mob_base));

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, BeeHurtByOtherGoal::new());
            target_selector.add_goal(2, BeeBecomeAngryTargetGoal::new());
            target_selector.add_goal(3, ResetUniversalAngerTargetGoal::new(true));
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
            stay_out_of_hive_countdown: SyncMutex::new(0),
            remaining_cooldown_before_locating_new_hive: SyncMutex::new(0),
            // Vanilla `Bee` constructor: `Mth.nextInt(random, 20, 60)`.
            remaining_cooldown_before_locating_new_flower: SyncMutex::new(rand::random_range(
                20..60,
            )),
            time_since_sting: SyncMutex::new(0),
            underwater_ticks: SyncMutex::new(0),
            pollinate_state,
            hive_blacklist,
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

    /// Vanilla `Bee.incrementNumCropsGrownSincePollination`.
    pub fn increment_crops_grown_since_pollination(&self) {
        *self.crops_grown_since_pollination.lock() += 1;
    }

    /// Vanilla `Bee.resetNumCropsGrownSincePollination`.
    pub fn reset_crops_grown_since_pollination(&self) {
        *self.crops_grown_since_pollination.lock() = 0;
    }

    /// Vanilla `Bee.resetTicksWithoutNectarSinceExitingHive`.
    pub fn reset_ticks_without_nectar_since_exiting_hive(&self) {
        *self.ticks_without_nectar.lock() = 0;
    }

    /// Vanilla `Bee.stayOutOfHiveCountdown`.
    #[must_use]
    pub fn stay_out_of_hive_countdown(&self) -> i32 {
        *self.stay_out_of_hive_countdown.lock()
    }

    /// Vanilla `Bee.setStayOutOfHiveCountdown`.
    pub fn set_stay_out_of_hive_countdown(&self, ticks: i32) {
        *self.stay_out_of_hive_countdown.lock() = ticks;
    }

    /// Vanilla `Bee.remainingCooldownBeforeLocatingNewHive`.
    #[must_use]
    pub fn remaining_cooldown_before_locating_new_hive(&self) -> i32 {
        *self.remaining_cooldown_before_locating_new_hive.lock()
    }

    /// Vanilla assignment to `Bee.remainingCooldownBeforeLocatingNewHive`.
    pub fn set_remaining_cooldown_before_locating_new_hive(&self, ticks: i32) {
        *self.remaining_cooldown_before_locating_new_hive.lock() = ticks;
    }

    /// Vanilla `Bee.remainingCooldownBeforeLocatingNewFlower`.
    #[must_use]
    pub fn remaining_cooldown_before_locating_new_flower(&self) -> i32 {
        *self.remaining_cooldown_before_locating_new_flower.lock()
    }

    /// Vanilla assignment to `Bee.remainingCooldownBeforeLocatingNewFlower`.
    pub fn set_remaining_cooldown_before_locating_new_flower(&self, ticks: i32) {
        *self.remaining_cooldown_before_locating_new_flower.lock() = ticks;
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

    /// Vanilla `Bee.hasHive`.
    #[must_use]
    pub fn has_hive(&self) -> bool {
        self.hive_pos().is_some()
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

    /// Vanilla `Bee.dropHive`.
    pub fn drop_hive(&self) {
        self.set_hive_pos(None);
        self.set_remaining_cooldown_before_locating_new_hive(200);
    }

    /// Vanilla `Bee.isTiredOfLookingForNectar`.
    #[must_use]
    pub fn is_tired_of_looking_for_nectar(&self) -> bool {
        self.ticks_without_nectar() > TICKS_WITHOUT_NECTAR_BEFORE_GOING_HOME
    }

    /// Vanilla `Bee.wantsToEnterHive`.
    ///
    /// Deviations: `BEES_STAY_IN_HIVE` always reads false (see the module docs), and
    /// `beePollinateGoal.isPollinating()` is the shared [`PollinateState`].
    #[must_use]
    pub fn wants_to_enter_hive(&self) -> bool {
        if self.stay_out_of_hive_countdown() > 0
            || self.is_pollinating()
            || self.has_stung()
            || self.target().is_some()
        {
            return false;
        }

        let wants = self.has_nectar() || self.is_tired_of_looking_for_nectar();
        wants && !self.is_hive_near_fire()
    }

    /// Vanilla `Bee.beePollinateGoal.isPollinating()`.
    #[must_use]
    pub fn is_pollinating(&self) -> bool {
        self.pollinate_state.lock().is_pollinating()
    }

    /// Vanilla `Bee.stopPollinating` (via `hurtServer`).
    pub fn stop_pollinating(&self) {
        self.pollinate_state.lock().stop_pollinating();
    }

    /// Vanilla `Bee.getBlacklistedHives`.
    #[must_use]
    pub fn blacklisted_hives(&self) -> Vec<BlockPos> {
        self.hive_blacklist.lock().positions()
    }

    /// Vanilla `Bee.closerThan(BlockPos, int)`.
    #[must_use]
    pub fn closer_than(&self, pos: BlockPos, distance: i32) -> bool {
        let origin = self.block_position();
        let dx = f64::from(pos.x() - origin.x());
        let dy = f64::from(pos.y() - origin.y());
        let dz = f64::from(pos.z() - origin.z());
        dx * dx + dy * dy + dz * dz < f64::from(distance) * f64::from(distance)
    }

    /// Vanilla `Bee.isTooFarAway`.
    #[must_use]
    pub fn is_too_far_away(&self, pos: BlockPos) -> bool {
        !self.closer_than(pos, TOO_FAR_DISTANCE)
    }

    /// Vanilla `Bee.getBeehiveBlockEntity`.
    #[must_use]
    pub fn beehive_block_entity(&self) -> Option<SharedBlockEntity> {
        let pos = self.hive_pos()?;
        if self.is_too_far_away(pos) {
            return None;
        }
        let world = self.level()?;

        world.get_block_entity(pos)
    }

    /// Vanilla `Bee.isHiveValid`.
    #[must_use]
    pub fn is_hive_valid(&self) -> bool {
        self.beehive_block_entity()
            .is_some_and(|entity| entity.downcast_ref::<BeehiveBlockEntity>().is_some())
    }

    /// Vanilla `Bee.isHiveNearFire`.
    #[must_use]
    pub fn is_hive_near_fire(&self) -> bool {
        self.beehive_block_entity()
            .and_then(|entity| {
                entity
                    .downcast_ref::<BeehiveBlockEntity>()
                    .map(BeehiveBlockEntity::is_fire_nearby)
            })
            .unwrap_or(false)
    }

    /// Vanilla `Bee.pathfindRandomlyTowards`.
    pub fn pathfind_randomly_towards(&self, pos: BlockPos) {
        let target = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let origin = self.block_position();
        let vertical_center = pos.y() - origin.y();
        let vertical_offset = if vertical_center > 2 {
            4
        } else if vertical_center < -2 {
            -4
        } else {
            0
        };
        let manhattan = (origin.x() - pos.x()).abs()
            + (origin.y() - pos.y()).abs()
            + (origin.z() - pos.z()).abs();
        let (horizontal, vertical) = if manhattan < 15 {
            (manhattan / 2, manhattan / 2)
        } else {
            (6, 8)
        };
        let Some(target_pos) = air_random_pos_towards(
            self,
            horizontal,
            vertical,
            vertical_offset,
            target,
            TENTH_PI,
        ) else {
            return;
        };

        self.mob_base()
            .navigation()
            .lock()
            .set_max_visited_nodes_multiplier(0.5);
        self.move_to_pos(target_pos, 1.0);
    }

    /// Vanilla `Bee.dropFlower`.
    pub fn drop_flower(&self) {
        self.set_flower_pos(None);
        self.set_remaining_cooldown_before_locating_new_flower(rand::random_range(20..60));
    }

    /// Vanilla `Bee.dropOffNectar`.
    pub fn drop_off_nectar(&self) {
        self.set_has_nectar(false);
        self.reset_crops_grown_since_pollination();
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
        self.tick_living_entity(&entity);
        // Vanilla `Bee.tick` also spawns `FALLING_NECTAR` particles and advances the roll-amount
        // render state; Steel has neither (see the module docs).
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
        // Vanilla `Bee.addAdditionalSaveData`: `hive_pos`/`flower_pos` use the `BlockPos` codec,
        // which writes an int array, and `IGNORED_BEE_TAGS` strips `hive_pos` from occupants.
        if let Some(hive) = self.hive_pos() {
            nbt.insert("hive_pos", NbtList::Int(vec![hive.x(), hive.y(), hive.z()]));
        }
        if let Some(flower) = self.flower_pos() {
            nbt.insert(
                "flower_pos",
                NbtList::Int(vec![flower.x(), flower.y(), flower.z()]),
            );
        }
        nbt.insert("HasNectar", i8::from(self.has_nectar()));
        nbt.insert("HasStung", i8::from(self.has_stung()));
        nbt.insert("TicksSincePollination", self.ticks_without_nectar());
        nbt.insert("CannotEnterHiveTicks", self.stay_out_of_hive_countdown());
        nbt.insert(
            "CropsGrownSincePollination",
            self.crops_grown_since_pollination(),
        );
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_has_nectar(nbt.byte("HasNectar").is_some_and(|value| value != 0));
        self.set_has_stung(nbt.byte("HasStung").is_some_and(|value| value != 0));
        *self.ticks_without_nectar.lock() = nbt.int("TicksSincePollination").unwrap_or(0);
        self.set_stay_out_of_hive_countdown(nbt.int("CannotEnterHiveTicks").unwrap_or(0));
        *self.crops_grown_since_pollination.lock() =
            nbt.int("CropsGrownSincePollination").unwrap_or(0);
        self.set_hive_pos(read_block_pos(nbt.int_array("hive_pos")));
        self.set_flower_pos(read_block_pos(nbt.int_array("flower_pos")));
        self.read_persistent_anger_save_data(&nbt);
    }
}

/// Vanilla `BlockPos.CODEC` decoding: the first three int-array entries.
fn read_block_pos(values: Option<Vec<i32>>) -> Option<BlockPos> {
    let values = values?;
    let [x, y, z, ..] = values.as_slice() else {
        return None;
    };

    Some(BlockPos::new(*x, *y, *z))
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
        let result = self.mob_ai_step(entity);

        // Vanilla `Bee.aiStep`: the three cooldowns, the rolling flag and the hive recheck.
        let stay_out = self.stay_out_of_hive_countdown();
        if stay_out > 0 {
            self.set_stay_out_of_hive_countdown(stay_out - 1);
        }
        let hive_cooldown = self.remaining_cooldown_before_locating_new_hive();
        if hive_cooldown > 0 {
            self.set_remaining_cooldown_before_locating_new_hive(hive_cooldown - 1);
        }
        let flower_cooldown = self.remaining_cooldown_before_locating_new_flower();
        if flower_cooldown > 0 {
            self.set_remaining_cooldown_before_locating_new_flower(flower_cooldown - 1);
        }

        let rolling = NeutralMob::is_angry(self)
            && !self.has_stung()
            && self.target().is_some_and(|target| {
                target.as_ref().distance_to_sqr(self.position()) < MIN_ATTACK_DIST
            });
        self.set_rolling(rolling);

        if self.tick_count() % 20 == 0 && !self.is_hive_valid() {
            self.set_hive_pos(None);
        }

        result
    }

    /// Vanilla `Bee.hurtServer` cancels pollination before `super.hurtServer`; Steel's single
    /// `hurt_server` body has no per-mob pre-hook, so it happens here (see the module docs).
    fn before_actually_hurt(&self, _source: &DamageSource, _amount: f32) {
        if let Some(animal) = self.as_animal() {
            animal.reset_love();
        }
        self.stop_pollinating();
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

    /// Vanilla `Bee.customServerAiStep`: drowning, the sting-death timer, the nectar-less
    /// counter, and the persistent-anger update with `stayAngryIfTargetPresent = false`.
    fn custom_server_ai_step(&self) {
        let has_stung = self.has_stung();

        let underwater_ticks = if self.is_in_water() {
            *self.underwater_ticks.lock() + 1
        } else {
            0
        };
        *self.underwater_ticks.lock() = underwater_ticks;
        if underwater_ticks > MAX_UNDERWATER_TICKS
            && let Some(world) = self.level()
        {
            self.hurt(
                &world,
                &DamageSource::environment(&vanilla_damage_types::DROWN),
                1.0,
            );
        }

        if has_stung {
            let should_die = {
                let mut time_since_sting = self.time_since_sting.lock();
                *time_since_sting += 1;
                if *time_since_sting % STING_DEATH_ROLL_INTERVAL == 0 {
                    let bound =
                        (STING_DEATH_COUNTDOWN - *time_since_sting).clamp(1, STING_DEATH_COUNTDOWN);
                    rand::random_range(0..bound) == 0
                } else {
                    false
                }
            };
            if should_die && let Some(world) = self.level() {
                let health = self.get_health();
                self.hurt(
                    &world,
                    &DamageSource::environment(&vanilla_damage_types::GENERIC),
                    health,
                );
            }
        }

        if !self.has_nectar() {
            *self.ticks_without_nectar.lock() += 1;
        }

        self.update_persistent_anger(false);
    }

    /// Vanilla `Bee.mobInteract`: a `#minecraft:bee_food` flower in the player's hand is consumed
    /// and its `FlowerBlock.getBeeInteractionEffect` applied.
    ///
    /// Only `WitherRoseBlock` overrides that effect in vanilla (`WITHER`, 40 ticks), so testing
    /// for the wither-rose item reproduces the whole vanilla branch exactly.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let stack = inventory.get_item_in_hand(hand);
            stack.copy_with_count(stack.count())
        };

        if self.is_food(&item_stack) && item_stack.is(&vanilla_items::WITHER_ROSE) {
            Mob::use_player_item(self, player, hand);
            self.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::WITHER,
                WITHER_ROSE_BEE_EFFECT_TICKS,
                0,
            ));
            return InteractionResult::Success;
        }

        Animal::mob_interact_animal(self, player, hand)
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
