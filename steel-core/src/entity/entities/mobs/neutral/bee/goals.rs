//! Vanilla `Bee` goals (`net.minecraft.world.entity.animal.bee.Bee$*Goal`).
//!
//! Vanilla implements these as inner classes that capture the `Bee` instance directly; Steel's
//! goals only receive `&dyn PathfinderMob`, so each goal resolves the concrete [`BeeEntity`] with
//! `downcast_ref` and drives the entity's lifecycle accessors, exactly like
//! `illager_spells::caster_of` does for the spellcasting illagers.
//!
//! Goal-side state that vanilla reads from another goal lives in shared handles handed to both:
//! [`PollinateState`] (vanilla `Bee.beePollinateGoal.isPollinating()`/`stopPollinating`) and
//! [`HiveBlacklist`] (vanilla `Bee$BeeGoToHiveGoal.blacklistedTargets`, read by
//! `Bee$BeeLocateHiveGoal`).
//!
//! Deviations:
//! - `Bee$BeeEnterHiveGoal.start` cannot record the occupancy because Steel's
//!   [`Goal::start`] receives no entity handle (owner: `entity/ai/goal/selector.rs`); the
//!   occupancy is recorded on the goal's first `tick`, which the selector runs in the same tick.
//! - `Bee$BeePollinateGoal.findNearbyFlower` scans vanilla's
//!   `BlockPos.withinBoxByManhattanDistance(pos, 5, 5, 5)` region in manhattan-distance order,
//!   but breaks equal-distance ties by `(x, y, z)` instead of vanilla's iterator nesting.
//! - `Bee$BeeGrowCropGoal` matches vanilla's `CropBlock`/`StemBlock`/`SweetBerryBushBlock` age
//!   branches by the block state's age property (Steel's crop behaviors share the `CropLike`
//!   trait instead of one `CropBlock` class) and reproduces vanilla's omission of
//!   `PitcherCropBlock`, which is in `#minecraft:bee_growables` but matches no branch.
//! - `Bee$BeeLookControl` (skip looks while angry, keep the x-rotation while pollinating) has no
//!   per-mob look control in Steel (owner: `entity/ai/control.rs`), so the shared `LookControl`
//!   is used unchanged.
//! - `Bee.pollinate`/`BeeWanderGoal` nectar particles (`ParticleTypes.FALLING_NECTAR`) and the
//!   roll-amount rendering state have no Steel counterpart (owner: `world` particles).

use std::cmp::Ordering;
use std::f64::consts::FRAC_PI_2;
use std::iter::empty;
use std::mem;
use std::ptr;
use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, DoubleBlockHalf, EnumProperty, IntProperty,
};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_poi_type_tags::PoiTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, level_events, sound_events};
use steel_utils::locks::SyncMutex;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Downcast};

use crate::behavior::BLOCK_BEHAVIORS;
use crate::block_entity::SharedBlockEntity;
use crate::block_entity::entities::BeehiveBlockEntity;
use crate::entity::ai::goal::random_pos::{HALF_PI, air_and_water_random_pos, hover_random_pos};
use crate::entity::ai::goal::{
    Goal, GoalControls, HurtByTargetGoal, MeleeAttackGoal, NearestAttackableTargetGoal,
};
use crate::entity::ai::path::Path;
use crate::entity::entities::BeeEntity;
use crate::entity::neutral_mob::NeutralMob;
use crate::entity::{Entity, Mob, PathfinderMob, SharedEntity};
use crate::poi::OccupationStatus;
use crate::world::World;

/// Vanilla `Bee.STING_DEATH_COUNTDOWN`.
pub const STING_DEATH_COUNTDOWN: i32 = 1200;
/// Vanilla `Bee.MAX_CROPS_GROWABLE`.
pub const MAX_CROPS_GROWABLE: i32 = 10;
/// Vanilla `Bee.TOO_FAR_DISTANCE`.
pub const TOO_FAR_DISTANCE: i32 = 48;
/// Vanilla `Bee.HIVE_CLOSE_ENOUGH_DISTANCE` (used as a block-center distance).
pub const HIVE_CLOSE_ENOUGH_DISTANCE: f64 = 2.0;
/// Vanilla `Bee.RESTRICTED_WANDER_DISTANCE_REDUCTION`.
const RESTRICTED_WANDER_DISTANCE_REDUCTION: i32 = 24;
/// Vanilla `Bee.DEFAULT_WANDER_DISTANCE_REDUCTION`.
const DEFAULT_WANDER_DISTANCE_REDUCTION: i32 = 16;
/// Vanilla `Bee.PATHFIND_TO_HIVE_WHEN_CLOSER_THAN`.
const PATHFIND_TO_HIVE_WHEN_CLOSER_THAN: i32 = 16;
/// Vanilla `Bee.HIVE_SEARCH_DISTANCE`.
const HIVE_SEARCH_DISTANCE: i32 = 20;
/// Vanilla `Bee.TICKS_BEFORE_GOING_TO_KNOWN_FLOWER`.
const TICKS_BEFORE_GOING_TO_KNOWN_FLOWER: i32 = 600;
/// Vanilla `Bee.COOLDOWN_BEFORE_LOCATING_NEW_HIVE`.
const COOLDOWN_BEFORE_LOCATING_NEW_HIVE: i32 = 200;
/// Vanilla `Bee.COOLDOWN_BEFORE_LOCATING_NEW_FLOWER`.
const COOLDOWN_BEFORE_LOCATING_NEW_FLOWER: i32 = 200;
/// Vanilla `Bee.MIN_FIND_FLOWER_RETRY_COOLDOWN`.
const MIN_FIND_FLOWER_RETRY_COOLDOWN: i32 = 20;
/// Vanilla `Bee.MAX_FIND_FLOWER_RETRY_COOLDOWN: i32` retry range (exclusive upper bound).
const MAX_FIND_FLOWER_RETRY_COOLDOWN: i32 = 60;
/// Vanilla `Bee$BeeGoToHiveGoal.TICKS_BEFORE_HIVE_DROP`.
const TICKS_BEFORE_HIVE_DROP: i32 = 60;
/// Vanilla `Bee$BeeGoToHiveGoal.MAX_BLACKLISTED_TARGETS`.
const MAX_BLACKLISTED_TARGETS: usize = 3;
/// Vanilla `Bee$BeeGoToHiveGoal.MAX_TRAVELLING_TICKS`.
const MAX_TRAVELLING_TICKS: i32 = 2400;
/// Vanilla `Bee$BeePollinateGoal.MIN_POLLINATION_TICKS`.
const MIN_POLLINATION_TICKS: i32 = 400;
/// Vanilla `Bee$BeePollinateGoal.MAX_POLLINATING_TICKS`.
const MAX_POLLINATING_TICKS: i32 = 600;
/// Vanilla `Bee$BeePollinateGoal.ARRIVAL_THRESHOLD`.
const ARRIVAL_THRESHOLD: f64 = 0.1;
/// Vanilla `Bee$BeePollinateGoal.POSITION_CHANGE_CHANCE`.
const POSITION_CHANGE_CHANCE: i32 = 25;
/// Vanilla `Bee$BeePollinateGoal.SPEED_MODIFIER`.
const SPEED_MODIFIER: f64 = 0.35;
/// Vanilla `Bee$BeePollinateGoal.HOVER_HEIGHT_WITHIN_FLOWER`.
const HOVER_HEIGHT_WITHIN_FLOWER: f64 = 0.6;
/// Vanilla `Bee$BeePollinateGoal.HOVER_POS_OFFSET`.
const HOVER_POS_OFFSET: f32 = 0.333_333_34;
/// Vanilla `Bee$BeePollinateGoal.FLOWER_SEARCH_RADIUS`.
const FLOWER_SEARCH_RADIUS: i32 = 5;
/// Vanilla `Bee$BeePollinateGoal.unreachableFlowerCache` entry lifetime.
const UNREACHABLE_FLOWER_CACHE_TICKS: i64 = 600;
/// Vanilla `Bee$BeePollinateGoal` sound interval (`lastSoundPlayedTick + 60`).
const POLLINATE_SOUND_INTERVAL: i32 = 60;
/// Vanilla `Bee$BeePollinateGoal` sound chance (`nextFloat() < 0.05F`).
const POLLINATE_SOUND_CHANCE: f32 = 0.05;
/// Vanilla `Bee$BeePollinateGoal.canBeeContinueToUse` early-exit chance (`nextFloat() < 0.2F`).
const POLLINATION_EARLY_EXIT_CHANCE: f32 = 0.2;
/// Vanilla `Bee$BeeGrowCropGoal.GROW_CHANCE`.
const GROW_CHANCE: i32 = 30;
/// Vanilla `Bee$BeeGrowCropGoal` grow roll (`nextFloat() >= 0.3F`).
const GROW_CHANCE_MINIMUM: f32 = 0.3;
/// Vanilla `Bee$BeeWanderGoal` horizontal/vertical search extents.
const WANDER_HORIZONTAL_DISTANCE: i32 = 8;
const WANDER_VERTICAL_DISTANCE: i32 = 7;
const WANDER_AIR_VERTICAL_DISTANCE: i32 = 4;
const WANDER_AIR_FLYING_HEIGHT: i32 = -2;
const WANDER_HOVER_MAX_HEIGHT: i32 = 3;
const WANDER_HOVER_MIN_HEIGHT: i32 = 1;
/// Vanilla `Cow.getMoveSpeedAttribute`-style wander speed for `Bee$BeeWanderGoal`.
const WANDER_SPEED: f64 = 0.8;
/// Vanilla `Bee$BeeWanderGoal.findPos` hive view direction when far from the hive.
const WANDER_VIEW_RADIANS: f64 = FRAC_PI_2;

const WATERLOGGED: &BoolProperty = &BlockStateProperties::WATERLOGGED;
const DOUBLE_BLOCK_HALF: &EnumProperty<DoubleBlockHalf> = &BlockStateProperties::DOUBLE_BLOCK_HALF;
const AGE_7: &IntProperty = &BlockStateProperties::AGE_7;
const AGE_3: &IntProperty = &BlockStateProperties::AGE_3;
const AGE_1: &IntProperty = &BlockStateProperties::AGE_1;

/// Vanilla `Goal.adjustedTickDelay`: goals without `requiresUpdateEveryTick` tick every second
/// server tick, so their internal countdowns are halved.
const fn adjusted_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
}

/// Vanilla `Bee$BeePollinateGoal.isPollinating`, shared with [`BeeEntity`] so the entity can
/// answer `Bee.wantsToEnterHive` and cancel pollination on damage.
#[derive(Debug, Default)]
pub struct PollinateState {
    pollinating: bool,
}

impl PollinateState {
    /// Vanilla `Bee$BeePollinateGoal.isPollinating`.
    #[must_use]
    pub const fn is_pollinating(&self) -> bool {
        self.pollinating
    }

    /// Vanilla `Bee$BeePollinateGoal.start`/`stop` assignment to the `pollinating` flag.
    pub const fn set_pollinating(&mut self, pollinating: bool) {
        self.pollinating = pollinating;
    }

    /// Vanilla `Bee$BeePollinateGoal.stopPollinating`.
    pub const fn stop_pollinating(&mut self) {
        self.pollinating = false;
    }
}

/// Vanilla `Bee$BeeGoToHiveGoal.blacklistedTargets`: hives the bee failed to reach, shared with
/// `Bee$BeeLocateHiveGoal` and exposed as `Bee.getBlacklistedHives`.
#[derive(Debug, Default)]
pub struct HiveBlacklist {
    targets: Vec<BlockPos>,
}

impl HiveBlacklist {
    /// Vanilla `Bee$BeeGoToHiveGoal.isTargetBlacklisted`.
    #[must_use]
    pub fn is_target_blacklisted(&self, pos: BlockPos) -> bool {
        self.targets.contains(&pos)
    }

    /// Vanilla `Bee$BeeGoToHiveGoal.blacklistTarget`: keeps at most three entries.
    pub fn blacklist_target(&mut self, pos: BlockPos) {
        self.targets.push(pos);
        while self.targets.len() > MAX_BLACKLISTED_TARGETS {
            self.targets.remove(0);
        }
    }

    /// Vanilla `Bee$BeeGoToHiveGoal.clearBlacklist`.
    pub fn clear(&mut self) {
        self.targets.clear();
    }

    /// Vanilla `Bee.getBlacklistedHives` view.
    #[must_use]
    pub fn positions(&self) -> Vec<BlockPos> {
        self.targets.clone()
    }
}

/// Vanilla `Bee.attractsBees`: a bee-attractive, non-waterlogged block, with sunflowers counted
/// only for their upper half.
#[must_use]
pub fn attracts_bees(state: BlockStateId) -> bool {
    if !state.get_block().has_tag(&BlockTag::BEE_ATTRACTIVE) {
        return false;
    }
    if state.try_get_value(WATERLOGGED).unwrap_or(false) {
        return false;
    }
    if !ptr::eq(state.get_block(), &raw const vanilla_blocks::SUNFLOWER) {
        return true;
    }

    state.try_get_value(DOUBLE_BLOCK_HALF) == Some(DoubleBlockHalf::Upper)
}

/// Resolves the concrete [`BeeEntity`] behind a goal's mob handle.
fn bee_of(mob: &dyn PathfinderMob) -> Option<&BeeEntity> {
    mob.downcast_ref::<BeeEntity>()
}

/// Vanilla `Bee.getBeehiveBlockEntity`: the hive at `hivePos` when it is in range.
fn hive_block_entity(bee: &BeeEntity) -> Option<SharedBlockEntity> {
    let pos = bee.hive_pos()?;
    if bee.is_too_far_away(pos) {
        return None;
    }
    let world = bee.level()?;
    let entity = world.get_block_entity(pos)?;
    entity.downcast_ref::<BeehiveBlockEntity>()?;

    Some(entity)
}

/// Vanilla `Bee.doesHiveHaveSpace`: the block entity at `pos` is a non-full beehive.
fn does_hive_have_space(world: &World, pos: BlockPos) -> bool {
    world.get_block_entity(pos).is_some_and(|entity| {
        entity
            .downcast_ref::<BeehiveBlockEntity>()
            .is_some_and(|hive| !hive.is_full())
    })
}

/// Vanilla `Vec3i.distSqr`.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let dx = f64::from(a.x() - b.x());
    let dy = f64::from(a.y() - b.y());
    let dz = f64::from(a.z() - b.z());
    dx * dx + dy * dy + dz * dz
}

/// Vanilla `BlockPos.withinBoxByManhattanDistance(origin, 5, 5, 5)`, ordered by manhattan
/// distance (see the module docs for the tie-break deviation).
fn box_by_manhattan_distance(origin: BlockPos, horizontal: i32, vertical: i32) -> Vec<BlockPos> {
    let mut positions = Vec::with_capacity(
        ((2 * horizontal + 1) * (2 * vertical + 1) * (2 * horizontal + 1)) as usize,
    );
    for dx in -horizontal..=horizontal {
        for dy in -vertical..=vertical {
            for dz in -horizontal..=horizontal {
                positions.push(origin.offset(dx, dy, dz));
            }
        }
    }
    positions.sort_by_key(|pos| {
        (pos.x() - origin.x()).abs() + (pos.y() - origin.y()).abs() + (pos.z() - origin.z()).abs()
    });
    positions
}

/// Vanilla `Bee$BeePollinateGoal.findNearbyFlower`'s state predicate: `Bee.attractsBees`.
fn flower_state_predicate(state: BlockStateId) -> bool {
    attracts_bees(state)
}

/// Vanilla `Bee.isTooFarAway`: `!closerThan(pos, TOO_FAR_DISTANCE)`.
fn closer_than(mob: &dyn Entity, pos: BlockPos, distance: i32) -> bool {
    dist_sqr(pos, mob.block_position()) < f64::from(distance) * f64::from(distance)
}

/// Vanilla `Bee.attractsBees`-gated hive search used by `Bee$BeeLocateHiveGoal.start`.
fn find_nearby_hives_with_space(mob: &dyn PathfinderMob) -> Vec<BlockPos> {
    let Some(world) = mob.level() else {
        return Vec::new();
    };
    let origin = mob.block_position();
    let mut hives: Vec<BlockPos> = world
        .poi_storage
        .lock()
        .get_in_range(
            &|poi| REGISTRY.poi_types.is_in_tag(poi, &PoiTag::BEE_HOME),
            origin,
            HIVE_SEARCH_DISTANCE,
            OccupationStatus::Any,
        )
        .into_iter()
        .map(|(_, pos)| pos)
        .filter(|pos| does_hive_have_space(world.as_ref(), *pos))
        .collect();
    hives.sort_by(|a, b| {
        dist_sqr(*a, origin)
            .partial_cmp(&dist_sqr(*b, origin))
            .unwrap_or(Ordering::Equal)
    });
    hives
}

/// Vanilla `Bee.closerThan(BlockPos, int)` on the mob position.
fn bee_closer_than(mob: &dyn PathfinderMob, pos: BlockPos, distance: i32) -> bool {
    closer_than(mob, pos, distance)
}

/// Vanilla `PathNavigation.isInProgress`: a path exists and is not finished.
fn navigation_in_progress(mob: &dyn PathfinderMob) -> bool {
    mob.mob_base()
        .navigation()
        .lock()
        .path()
        .is_some_and(|path| !path.is_done())
}

/// Returns a copy of the navigation's current path, if any.
fn navigation_path(mob: &dyn PathfinderMob) -> Option<Path> {
    mob.mob_base().navigation().lock().path().cloned()
}

/// Vanilla `Bee$BeeGoToHiveGoal`(`Bee$BeeGoToKnownFlowerGoal`).stop navigation reset.
fn stop_navigation(mob: &dyn PathfinderMob) {
    let mut navigation = mob.mob_base().navigation().lock();
    navigation.stop();
    navigation.reset_max_visited_nodes_multiplier();
}

/// Vanilla `Bee$BeeAttackGoal`: a `MeleeAttackGoal` that only runs while the bee is angry and
/// has not stung yet.
pub struct BeeAttackGoal {
    inner: MeleeAttackGoal,
}

impl BeeAttackGoal {
    /// Vanilla `new Bee$BeeAttackGoal(bee, bee, speedModifier, pauseWhenMobIdle)`.
    #[must_use]
    pub const fn new(speed_modifier: f64, pause_when_mob_idle: bool) -> Self {
        Self {
            inner: MeleeAttackGoal::new(speed_modifier, pause_when_mob_idle),
        }
    }

    /// Vanilla `Bee`'s `isAngry() && !hasStung()` gate shared by `canUse`/`canContinueToUse`.
    fn bee_can_attack(mob: &dyn PathfinderMob) -> bool {
        bee_of(mob).is_some_and(|bee| NeutralMob::is_angry(bee) && !bee.has_stung())
    }
}

impl Goal for BeeAttackGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob) && Self::bee_can_attack(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob) && Self::bee_can_attack(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `Bee$BeeEnterHiveGoal`: walks the bee into its hive and records the occupancy.
#[derive(Default)]
pub struct BeeEnterHiveGoal {
    pending_enter: bool,
}

impl BeeEnterHiveGoal {
    /// Creates the goal.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending_enter: false,
        }
    }

    /// Vanilla `Bee$BeeEnterHiveGoal.canBeeUse`.
    fn bee_can_use(bee: &BeeEntity) -> bool {
        let Some(hive_pos) = bee.hive_pos() else {
            return false;
        };
        if !bee.wants_to_enter_hive() {
            return false;
        }
        if !hive_pos.closer_to_center_than(bee.position(), HIVE_CLOSE_ENOUGH_DISTANCE) {
            return false;
        }
        let Some(shared) = hive_block_entity(bee) else {
            return false;
        };
        let Some(hive) = shared.downcast_ref::<BeehiveBlockEntity>() else {
            return false;
        };
        if hive.is_full() {
            bee.set_hive_pos(None);
            return false;
        }

        true
    }
}

impl Goal for BeeEnterHiveGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        // Vanilla `BaseBeeGoal.canUse`: `canBeeUse() && !bee.isAngry()`.
        !NeutralMob::is_angry(bee) && Self::bee_can_use(bee)
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        // Steel's `Goal::start` has no entity handle; the selector runs `start` and `tick` in the
        // same tick, so recording the occupancy here instead is behaviour-identical.
        self.pending_enter = bee_of(mob).is_some();
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.pending_enter = false;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        if !mem::take(&mut self.pending_enter) {
            return;
        }
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(shared) = hive_block_entity(bee) else {
            return;
        };
        let Some(hive) = shared.downcast_ref::<BeehiveBlockEntity>() else {
            return;
        };
        hive.add_occupant(entity);
    }
}

/// Vanilla `Bee$ValidateHiveGoal`: drops a stale hive position on a random cooldown.
pub struct ValidateHiveGoal {
    validate_hive_cooldown: i32,
    last_validate_tick: i64,
}

impl ValidateHiveGoal {
    /// Vanilla `new Bee$ValidateHiveGoal(bee)`: cooldown `Mth.nextInt(random, 20, 40)`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            validate_hive_cooldown: rand::random_range(
                MIN_FIND_FLOWER_RETRY_COOLDOWN..MAX_FIND_FLOWER_RETRY_COOLDOWN,
            ),
            last_validate_tick: -1,
        }
    }
}

impl Default for ValidateHiveGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for ValidateHiveGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }
        let Some(world) = bee.level() else {
            return false;
        };

        world.game_time() > self.last_validate_tick + i64::from(self.validate_hive_cooldown)
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        if let Some(hive_pos) = bee.hive_pos()
            && let Some(world) = bee.level()
        {
            let loaded = world.has_full_chunk(ChunkPos::from_block_pos(hive_pos));
            if loaded && !bee.is_hive_valid() {
                bee.drop_hive();
            }
        }
        if let Some(world) = bee.level() {
            self.last_validate_tick = world.game_time();
        }
    }
}

/// Vanilla `Bee$ValidateFlowerGoal`: drops a flower position that no longer attracts bees.
pub struct ValidateFlowerGoal {
    validate_flower_cooldown: i32,
    last_validate_tick: i64,
}

impl ValidateFlowerGoal {
    /// Vanilla `new Bee$ValidateFlowerGoal(bee)`: cooldown `Mth.nextInt(random, 20, 40)`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            validate_flower_cooldown: rand::random_range(
                MIN_FIND_FLOWER_RETRY_COOLDOWN..MAX_FIND_FLOWER_RETRY_COOLDOWN,
            ),
            last_validate_tick: -1,
        }
    }
}

impl Default for ValidateFlowerGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for ValidateFlowerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }
        let Some(world) = bee.level() else {
            return false;
        };

        world.game_time() > self.last_validate_tick + i64::from(self.validate_flower_cooldown)
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        if let Some(flower_pos) = bee.flower_pos()
            && let Some(world) = bee.level()
        {
            let loaded = world.has_full_chunk(ChunkPos::from_block_pos(flower_pos));
            if loaded && !attracts_bees(world.get_block_state(flower_pos)) {
                bee.drop_flower();
            }
        }
        if let Some(world) = bee.level() {
            self.last_validate_tick = world.game_time();
        }
    }
}

/// Vanilla `Bee$BeePollinateGoal`: hovers over a nearby flower to gather nectar.
pub struct BeePollinateGoal {
    state: Arc<SyncMutex<PollinateState>>,
    successful_pollinating_ticks: i32,
    last_sound_played_tick: i32,
    pollinating_ticks: i32,
    hover_pos: Option<DVec3>,
    unreachable_flower_cache: FxHashMap<BlockPos, i64>,
}

impl BeePollinateGoal {
    /// Creates the goal, sharing `state` with the bee entity.
    #[must_use]
    pub fn new(state: Arc<SyncMutex<PollinateState>>) -> Self {
        Self {
            state,
            successful_pollinating_ticks: 0,
            last_sound_played_tick: 0,
            pollinating_ticks: 0,
            hover_pos: None,
            unreachable_flower_cache: FxHashMap::default(),
        }
    }

    /// Vanilla `Bee$BeePollinateGoal.isPollinating`.
    #[must_use]
    pub fn is_pollinating(&self) -> bool {
        self.state.lock().is_pollinating()
    }

    /// Vanilla `Bee$BeePollinateGoal.isPollinating` reads.
    fn set_pollinating(&self, pollinating: bool) {
        self.state.lock().set_pollinating(pollinating);
    }

    /// Vanilla `Bee$BeePollinateGoal.hasPollinatedLongEnough`.
    #[must_use]
    pub const fn has_pollinated_long_enough(&self) -> bool {
        self.successful_pollinating_ticks > MIN_POLLINATION_TICKS
    }

    /// Vanilla `Bee$BeePollinateGoal.setWantedPos`.
    fn set_wanted_pos(&self, bee: &BeeEntity) {
        let Some(hover_pos) = self.hover_pos else {
            return;
        };
        bee.mob_base()
            .controls()
            .lock()
            .move_control
            .set_wanted_position(hover_pos, SPEED_MODIFIER);
    }

    /// Vanilla `Bee$BeePollinateGoal.getOffset`.
    fn offset() -> f64 {
        f64::from(rand::random::<f32>() * 2.0 - 1.0) * f64::from(HOVER_POS_OFFSET)
    }

    /// Vanilla `Bee$BeePollinateGoal.findNearbyFlower`.
    fn find_nearby_flower(&mut self, mob: &dyn PathfinderMob, bee: &BeeEntity) -> Option<BlockPos> {
        let world = bee.level()?;
        let game_time = world.game_time();
        for pos in box_by_manhattan_distance(
            bee.block_position(),
            FLOWER_SEARCH_RADIUS,
            FLOWER_SEARCH_RADIUS,
        ) {
            let state = world.get_block_state(pos);
            if !flower_state_predicate(state) {
                continue;
            }
            if let Some(until) = self.unreachable_flower_cache.get(&pos)
                && game_time < *until
            {
                continue;
            }
            if mob
                .create_path_to(pos, 1)
                .is_some_and(|path| path.can_reach())
            {
                return Some(pos);
            }
            self.unreachable_flower_cache
                .insert(pos, game_time + UNREACHABLE_FLOWER_CACHE_TICKS);
        }

        None
    }
}

impl Goal for BeePollinateGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }
        if bee.remaining_cooldown_before_locating_new_flower() > 0 {
            return false;
        }
        if bee.has_nectar() {
            return false;
        }
        let Some(world) = bee.level() else {
            return false;
        };
        if world.is_raining() {
            return false;
        }

        if let Some(flower_pos) = self.find_nearby_flower(mob, bee) {
            bee.set_flower_pos(Some(flower_pos));
            let center = DVec3::new(
                f64::from(flower_pos.x()) + 0.5,
                f64::from(flower_pos.y()) + 0.5,
                f64::from(flower_pos.z()) + 0.5,
            );
            mob.move_to_pos(center, 1.2)
        } else {
            bee.set_remaining_cooldown_before_locating_new_flower(rand::random_range(
                MIN_FIND_FLOWER_RETRY_COOLDOWN..MAX_FIND_FLOWER_RETRY_COOLDOWN,
            ));
            false
        }
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }
        if !self.is_pollinating() {
            return false;
        }
        if bee.flower_pos().is_none() {
            return false;
        }
        let Some(world) = bee.level() else {
            return false;
        };
        if world.is_raining() {
            return false;
        }
        if self.has_pollinated_long_enough() {
            return rand::random::<f32>() < POLLINATION_EARLY_EXIT_CHANCE;
        }

        true
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.successful_pollinating_ticks = 0;
        self.pollinating_ticks = 0;
        self.last_sound_played_tick = 0;
        self.set_pollinating(true);
        if let Some(bee) = bee_of(mob) {
            bee.reset_ticks_without_nectar_since_exiting_hive();
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        // Vanilla reads the goal-local flag; the shared handle is the single source here.
        if self.has_pollinated_long_enough()
            && let Some(bee) = bee_of(mob)
        {
            bee.set_has_nectar(true);
        }
        self.set_pollinating(false);
        if let Some(bee) = bee_of(mob) {
            stop_navigation(mob);
            bee.set_remaining_cooldown_before_locating_new_flower(
                COOLDOWN_BEFORE_LOCATING_NEW_FLOWER,
            );
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(flower_pos) = bee.flower_pos() else {
            return;
        };

        self.pollinating_ticks += 1;
        if self.pollinating_ticks > MAX_POLLINATING_TICKS {
            bee.drop_flower();
            self.set_pollinating(false);
            bee.set_remaining_cooldown_before_locating_new_flower(
                COOLDOWN_BEFORE_LOCATING_NEW_FLOWER,
            );
            return;
        }

        let (x, y, z) = flower_pos.get_bottom_center();
        let hover_above_flower = DVec3::new(x, y + HOVER_HEIGHT_WITHIN_FLOWER, z);
        if hover_above_flower.distance(bee.position()) > 1.0 {
            self.hover_pos = Some(hover_above_flower);
            self.set_wanted_pos(bee);
            return;
        }
        if self.hover_pos.is_none() {
            self.hover_pos = Some(hover_above_flower);
        }

        let hover_pos = self.hover_pos.unwrap_or(hover_above_flower);
        let arrived = bee.position().distance(hover_pos) <= ARRIVAL_THRESHOLD;
        let mut set_wanted = true;
        if arrived {
            let position_change = rand::random_range(0..POSITION_CHANGE_CHANCE) == 0;
            if position_change {
                self.hover_pos = Some(DVec3::new(
                    hover_above_flower.x + Self::offset(),
                    hover_above_flower.y,
                    hover_above_flower.z + Self::offset(),
                ));
                bee.mob_base().navigation().lock().stop();
            } else {
                set_wanted = false;
            }

            let look_control = {
                let mut controls = bee.mob_base().controls().lock();
                let y_speed = controls.look_control.y_max_rot_speed();
                let x_angle = controls.look_control.x_max_rot_angle();
                controls
                    .look_control
                    .set_look_at(hover_above_flower, y_speed, x_angle);
                true
            };
            debug_assert!(look_control);
        }

        if set_wanted {
            self.set_wanted_pos(bee);
        }

        self.successful_pollinating_ticks += 1;
        if rand::random::<f32>() < POLLINATE_SOUND_CHANCE
            && self.successful_pollinating_ticks
                > self.last_sound_played_tick + POLLINATE_SOUND_INTERVAL
        {
            self.last_sound_played_tick = self.successful_pollinating_ticks;
            bee.play_sound(&sound_events::ENTITY_BEE_POLLINATE, 1.0, 1.0);
        }
    }
}

/// Vanilla `Bee$BeeLocateHiveGoal`: finds a nearby hive with space when the bee wants to go home.
pub struct BeeLocateHiveGoal {
    blacklist: Arc<SyncMutex<HiveBlacklist>>,
}

impl BeeLocateHiveGoal {
    /// Creates the goal, sharing the hive blacklist with `Bee$BeeGoToHiveGoal`.
    #[must_use]
    pub const fn new(blacklist: Arc<SyncMutex<HiveBlacklist>>) -> Self {
        Self { blacklist }
    }
}

impl Goal for BeeLocateHiveGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }

        bee.remaining_cooldown_before_locating_new_hive() == 0
            && bee.hive_pos().is_none()
            && bee.wants_to_enter_hive()
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        bee.set_remaining_cooldown_before_locating_new_hive(COOLDOWN_BEFORE_LOCATING_NEW_HIVE);

        let hives = find_nearby_hives_with_space(mob);
        let Some(first) = hives.first().copied() else {
            return;
        };

        let mut blacklist = self.blacklist.lock();
        for hive_pos in &hives {
            if !blacklist.is_target_blacklisted(*hive_pos) {
                drop(blacklist);
                bee.set_hive_pos(Some(*hive_pos));
                return;
            }
        }
        blacklist.clear();
        drop(blacklist);
        bee.set_hive_pos(Some(first));
    }
}

/// Vanilla `Bee$BeeGoToHiveGoal`: flies the bee to its known hive.
pub struct BeeGoToHiveGoal {
    blacklist: Arc<SyncMutex<HiveBlacklist>>,
    travelling_ticks: i32,
    last_path: Option<Path>,
    ticks_stuck: i32,
}

impl BeeGoToHiveGoal {
    /// Creates the goal, sharing the hive blacklist with `Bee$BeeLocateHiveGoal`.
    #[must_use]
    pub const fn new(blacklist: Arc<SyncMutex<HiveBlacklist>>) -> Self {
        Self {
            blacklist,
            travelling_ticks: 0,
            last_path: None,
            ticks_stuck: 0,
        }
    }

    /// Vanilla `Bee$BeeGoToHiveGoal.hasReachedTarget`.
    fn has_reached_target(mob: &dyn PathfinderMob, pos: BlockPos) -> bool {
        if bee_closer_than(mob, pos, HIVE_CLOSE_ENOUGH_DISTANCE as i32) {
            return true;
        }

        navigation_path(mob)
            .is_some_and(|path| path.target() == pos && path.can_reach() && path.is_done())
    }

    /// Vanilla `Bee$BeeGoToHiveGoal.pathfindDirectlyTowards`.
    fn pathfind_directly_towards(mob: &dyn PathfinderMob, pos: BlockPos) -> bool {
        let reach_range = if bee_closer_than(mob, pos, 3) { 1 } else { 2 };
        mob.mob_base()
            .navigation()
            .lock()
            .set_max_visited_nodes_multiplier(10.0);
        mob.move_to_pos_with_reach(
            DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z())),
            reach_range,
            1.0,
        );

        navigation_path(mob).is_some_and(|path| path.can_reach())
    }

    /// Vanilla `Bee$BeeGoToHiveGoal.dropAndBlacklistHive`.
    fn drop_and_blacklist_hive(&self, bee: &BeeEntity) {
        if let Some(hive_pos) = bee.hive_pos() {
            self.blacklist.lock().blacklist_target(hive_pos);
        }
        bee.drop_hive();
    }
}

impl Goal for BeeGoToHiveGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }
        let Some(hive_pos) = bee.hive_pos() else {
            return false;
        };
        if bee.is_too_far_away(hive_pos)
            || mob.has_home()
            || !bee.wants_to_enter_hive()
            || Self::has_reached_target(mob, hive_pos)
        {
            return false;
        }
        let Some(world) = bee.level() else {
            return false;
        };

        world
            .get_block_state(hive_pos)
            .get_block()
            .has_tag(&BlockTag::BEEHIVES)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob)
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.travelling_ticks = 0;
        self.ticks_stuck = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.travelling_ticks = 0;
        self.ticks_stuck = 0;
        stop_navigation(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(hive_pos) = bee.hive_pos() else {
            return;
        };

        self.travelling_ticks += 1;
        if self.travelling_ticks > adjusted_tick_delay(MAX_TRAVELLING_TICKS) {
            self.drop_and_blacklist_hive(bee);
            return;
        }
        if navigation_in_progress(mob) {
            return;
        }

        if bee_closer_than(mob, hive_pos, PATHFIND_TO_HIVE_WHEN_CLOSER_THAN) {
            if Self::pathfind_directly_towards(mob, hive_pos) {
                let current = navigation_path(mob);
                let same_as_last = match (&self.last_path, &current) {
                    (Some(last), Some(current)) => current.same_as(last),
                    _ => false,
                };
                if same_as_last {
                    self.ticks_stuck += 1;
                    if self.ticks_stuck > TICKS_BEFORE_HIVE_DROP {
                        bee.drop_hive();
                        self.ticks_stuck = 0;
                    }
                } else {
                    self.last_path = current;
                }
            } else {
                self.drop_and_blacklist_hive(bee);
            }
        } else if bee.is_too_far_away(hive_pos) {
            bee.drop_hive();
        } else {
            bee.pathfind_randomly_towards(hive_pos);
        }
    }
}

/// Vanilla `Bee$BeeGoToKnownFlowerGoal`: flies back to a remembered flower when nectar-less.
pub struct BeeGoToKnownFlowerGoal {
    travelling_ticks: i32,
}

impl BeeGoToKnownFlowerGoal {
    /// Creates the goal.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            travelling_ticks: 0,
        }
    }

    /// Vanilla `Bee$BeeGoToKnownFlowerGoal.wantsToGoToKnownFlower`.
    fn wants_to_go_to_known_flower(bee: &BeeEntity) -> bool {
        bee.ticks_without_nectar() > TICKS_BEFORE_GOING_TO_KNOWN_FLOWER
    }
}

impl Default for BeeGoToKnownFlowerGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for BeeGoToKnownFlowerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }
        let Some(flower_pos) = bee.flower_pos() else {
            return false;
        };

        !mob.has_home()
            && Self::wants_to_go_to_known_flower(bee)
            && !bee_closer_than(mob, flower_pos, HIVE_CLOSE_ENOUGH_DISTANCE as i32)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob)
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.travelling_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.travelling_ticks = 0;
        stop_navigation(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(flower_pos) = bee.flower_pos() else {
            return;
        };

        self.travelling_ticks += 1;
        if self.travelling_ticks > adjusted_tick_delay(MAX_TRAVELLING_TICKS) {
            bee.drop_flower();
            return;
        }
        if navigation_in_progress(mob) {
            return;
        }
        if bee.is_too_far_away(flower_pos) {
            bee.drop_flower();
            return;
        }

        bee.pathfind_randomly_towards(flower_pos);
    }
}

/// Vanilla `Bee$BeeGrowCropGoal`: grows the block below the bee when it carries nectar.
pub struct BeeGrowCropGoal;

impl BeeGrowCropGoal {
    /// Creates the goal.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Vanilla's `CropBlock`/`StemBlock`/`SWEET_BERRY_BUSH` age-`+1` branches.
    ///
    /// Steel has no single `CropBlock` class (each crop owns a behavior implementing `CropLike`),
    /// and the registry's property lookup matches by property *name*, so the branch is matched by
    /// block identity plus the block's own age property. `PitcherCropBlock` is intentionally
    /// absent: vanilla's `instanceof` chain matches no branch for it even though it is in
    /// `#minecraft:bee_growables`.
    fn grown_age_state(state: BlockStateId) -> Option<BlockStateId> {
        let block = state.get_block();
        let (property, max_age) = if ptr::eq(block, &raw const vanilla_blocks::WHEAT)
            || ptr::eq(block, &raw const vanilla_blocks::CARROTS)
            || ptr::eq(block, &raw const vanilla_blocks::POTATOES)
            || ptr::eq(block, &raw const vanilla_blocks::MELON_STEM)
            || ptr::eq(block, &raw const vanilla_blocks::PUMPKIN_STEM)
        {
            (AGE_7, 7_u8)
        } else if ptr::eq(block, &raw const vanilla_blocks::BEETROOTS)
            || ptr::eq(block, &raw const vanilla_blocks::SWEET_BERRY_BUSH)
        {
            (AGE_3, 3_u8)
        } else if ptr::eq(block, &raw const vanilla_blocks::TORCHFLOWER_CROP) {
            (AGE_1, 1_u8)
        } else {
            return None;
        };

        state
            .try_get_value(property)
            .filter(|age| *age < max_age)
            .map(|age| state.set_value(property, age + 1))
    }

    /// Vanilla `Bee$BeeGrowCropGoal`'s cave-vines branch: bonemeal the block.
    fn bonemeal_cave_vines(world: &Arc<World>, pos: BlockPos, state: BlockStateId) -> bool {
        let Some(behavior) = BLOCK_BEHAVIORS.get_behavior_for_state(state) else {
            return false;
        };
        let Some(bonemealable) = behavior.as_bonemealable() else {
            return false;
        };
        if !bonemealable.is_valid_bonemeal_target(state, world.as_ref(), pos) {
            return false;
        }

        bonemealable.perform_bonemeal(state, world, &mut rand::rng(), pos);
        true
    }

    /// Vanilla `state.is(Blocks.CAVE_VINES) || state.is(Blocks.CAVE_VINES_PLANT)`.
    fn is_cave_vines(state: BlockStateId) -> bool {
        let block = state.get_block();
        ptr::eq(block, &raw const vanilla_blocks::CAVE_VINES)
            || ptr::eq(block, &raw const vanilla_blocks::CAVE_VINES_PLANT)
    }
}

impl Default for BeeGrowCropGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for BeeGrowCropGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(bee) = bee_of(mob) else {
            return false;
        };
        if NeutralMob::is_angry(bee) {
            return false;
        }
        if bee.crops_grown_since_pollination() >= MAX_CROPS_GROWABLE {
            return false;
        }
        if rand::random::<f32>() < GROW_CHANCE_MINIMUM {
            return false;
        }

        bee.has_nectar() && bee.is_hive_valid()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob)
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        if rand::random_range(0..adjusted_tick_delay(GROW_CHANCE)) != 0 {
            return;
        }
        let Some(world) = bee.level() else {
            return;
        };

        for distance in 1..=2 {
            let pos = bee.block_position().below_n(distance);
            let state = world.get_block_state(pos);
            if !state.get_block().has_tag(&BlockTag::BEE_GROWABLES) {
                continue;
            }

            let new_state = if Self::is_cave_vines(state) {
                if Self::bonemeal_cave_vines(&world, pos, state) {
                    Some(world.get_block_state(pos))
                } else {
                    None
                }
            } else {
                Self::grown_age_state(state)
            };

            if let Some(new_state) = new_state {
                world.level_event(level_events::PARTICLES_BEE_GROWTH, pos, 15, None);
                world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
                bee.increment_crops_grown_since_pollination();
            }
        }
    }
}

/// Vanilla `Bee$BeeWanderGoal`: a random flying wander, biased toward the hive when far away.
pub struct BeeWanderGoal;

impl BeeWanderGoal {
    /// Creates the goal.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Vanilla `Bee.isHiveValid`-gated wander threshold: `48 - (24 or 16)`.
    fn wander_threshold(bee: &BeeEntity) -> i32 {
        let reduction = if bee.hive_pos().is_some() || bee.flower_pos().is_some() {
            RESTRICTED_WANDER_DISTANCE_REDUCTION
        } else {
            DEFAULT_WANDER_DISTANCE_REDUCTION
        };

        TOO_FAR_DISTANCE - reduction
    }

    /// Vanilla `Bee$BeeWanderGoal.findPos`.
    fn find_pos(mob: &dyn PathfinderMob, bee: &BeeEntity) -> Option<DVec3> {
        let direction = if bee.is_hive_valid()
            && bee.hive_pos().is_some_and(|hive_pos| {
                !bee_closer_than(mob, hive_pos, Self::wander_threshold(bee))
            }) {
            let (x, y, z) = bee.hive_pos()?.get_center();
            (DVec3::new(x, y, z) - bee.position()).normalize()
        } else {
            let (yaw, _) = bee.rotation();
            bee.calculate_view_vector(0.0, yaw)
        };

        hover_random_pos(
            mob,
            WANDER_HORIZONTAL_DISTANCE,
            WANDER_VERTICAL_DISTANCE,
            direction.x,
            direction.z,
            HALF_PI,
            WANDER_HOVER_MAX_HEIGHT,
            WANDER_HOVER_MIN_HEIGHT,
        )
        .or_else(|| {
            air_and_water_random_pos(
                mob,
                WANDER_HORIZONTAL_DISTANCE,
                WANDER_AIR_VERTICAL_DISTANCE,
                WANDER_AIR_FLYING_HEIGHT,
                direction.x,
                direction.z,
                WANDER_VIEW_RADIANS,
            )
        })
    }
}

impl Default for BeeWanderGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for BeeWanderGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.mob_base().navigation().lock().is_done() && rand::random_range(0..10) == 0
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        navigation_in_progress(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        if let Some(pos) = Self::find_pos(mob, bee) {
            mob.move_to_pos(pos, WANDER_SPEED);
        }
    }
}

/// Vanilla `Bee$BeeHurtByOtherGoal`: `HurtByTargetGoal` that only keeps running while angry.
pub struct BeeHurtByOtherGoal {
    inner: HurtByTargetGoal,
}

impl BeeHurtByOtherGoal {
    /// Vanilla `new Bee$BeeHurtByOtherGoal(bee, bee)` plus `setAlertOthers()` (no classes, so
    /// `alertOther` is never reached).
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: HurtByTargetGoal::new().set_alert_others(empty()),
        }
    }
}

impl Default for BeeHurtByOtherGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for BeeHurtByOtherGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        bee_of(mob).is_some_and(NeutralMob::is_angry) && self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `Bee$BeeBecomeAngryTargetGoal`: targets a player while the bee is angry and un-stung.
pub struct BeeBecomeAngryTargetGoal {
    inner: NearestAttackableTargetGoal,
}

impl BeeBecomeAngryTargetGoal {
    /// Vanilla `new Bee$BeeBecomeAngryTargetGoal(bee)`:
    /// `NearestAttackableTargetGoal(bee, Player.class, 10, true, false, bee::beeCanTarget)`.
    #[must_use]
    pub fn new() -> Self {
        // Vanilla's selector is `Bee$BeeBecomeAngryTargetGoal::beeCanTarget`, which ignores the
        // candidate; `can_use`/`can_continue_to_use` gate on it, so a constant selector by
        // candidate is equivalent.
        Self {
            inner: NearestAttackableTargetGoal::new_for_players_with_interval(
                10,
                true,
                false,
                |_, _| true,
            ),
        }
    }

    /// Vanilla `Bee$BeeBecomeAngryTargetGoal.beeCanTarget`.
    fn bee_can_target(mob: &dyn PathfinderMob) -> bool {
        bee_of(mob).is_some_and(|bee| NeutralMob::is_angry(bee) && !bee.has_stung())
    }
}

impl Default for BeeBecomeAngryTargetGoal {
    fn default() -> Self {
        Self::new()
    }
}

impl Goal for BeeBecomeAngryTargetGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::bee_can_target(mob) && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !Self::bee_can_target(mob) || mob.target().is_none() {
            return false;
        }

        self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::init_vanilla_registry;

    #[test]
    fn hive_blacklist_keeps_only_the_three_newest_targets() {
        let mut blacklist = HiveBlacklist::default();
        for x in 0..4 {
            blacklist.blacklist_target(BlockPos::new(x, 64, 0));
        }

        assert!(!blacklist.is_target_blacklisted(BlockPos::new(0, 64, 0)));
        assert!(blacklist.is_target_blacklisted(BlockPos::new(3, 64, 0)));

        blacklist.clear();
        assert!(!blacklist.is_target_blacklisted(BlockPos::new(3, 64, 0)));
    }

    #[test]
    fn crop_age_growth_matches_vanilla_crop_stem_and_berry_branches() {
        init_vanilla_registry();

        let wheat = vanilla_blocks::WHEAT.default_state().set_value(AGE_7, 3);
        assert_eq!(
            BeeGrowCropGoal::grown_age_state(wheat).map(|state| state.get_value(AGE_7)),
            Some(4)
        );

        let ripe = vanilla_blocks::WHEAT.default_state().set_value(AGE_7, 7);
        assert!(BeeGrowCropGoal::grown_age_state(ripe).is_none());

        let berry = vanilla_blocks::SWEET_BERRY_BUSH
            .default_state()
            .set_value(AGE_3, 2);
        assert_eq!(
            BeeGrowCropGoal::grown_age_state(berry).map(|state| state.get_value(AGE_3)),
            Some(3)
        );
        let ripe_berry = vanilla_blocks::SWEET_BERRY_BUSH
            .default_state()
            .set_value(AGE_3, 3);
        assert!(BeeGrowCropGoal::grown_age_state(ripe_berry).is_none());

        let torchflower = vanilla_blocks::TORCHFLOWER_CROP.default_state();
        assert_eq!(
            BeeGrowCropGoal::grown_age_state(torchflower).map(|state| state.get_value(AGE_1)),
            Some(1)
        );

        // Vanilla's `instanceof CropBlock`/`StemBlock`/`SWEET_BERRY_BUSH` chain skips pitcher crops.
        let pitcher = vanilla_blocks::PITCHER_CROP.default_state();
        assert!(BeeGrowCropGoal::grown_age_state(pitcher).is_none());
    }

    #[test]
    fn attracts_bees_requires_the_upper_sunflower_half() {
        init_vanilla_registry();

        assert!(attracts_bees(vanilla_blocks::DANDELION.default_state()));
        assert!(!attracts_bees(vanilla_blocks::STONE.default_state()));

        let lower = vanilla_blocks::SUNFLOWER
            .default_state()
            .set_value(DOUBLE_BLOCK_HALF, DoubleBlockHalf::Lower);
        let upper = vanilla_blocks::SUNFLOWER
            .default_state()
            .set_value(DOUBLE_BLOCK_HALF, DoubleBlockHalf::Upper);
        assert!(!attracts_bees(lower));
        assert!(attracts_bees(upper));
    }

    #[test]
    fn manhattan_order_is_brute_force_sorted_by_distance() {
        let origin = BlockPos::new(4, 64, 4);
        let positions = box_by_manhattan_distance(origin, 2, 1);

        assert_eq!(positions.len(), 5 * 3 * 5);
        assert_eq!(positions[0], origin);
        let mut previous = 0;
        for pos in &positions {
            let distance = (pos.x() - origin.x()).abs()
                + (pos.y() - origin.y()).abs()
                + (pos.z() - origin.z()).abs();
            assert!(distance >= previous);
            previous = distance;
        }
    }

    #[test]
    fn distance_squared_matches_vanilla_dist_sqr() {
        assert_eq!(
            dist_sqr(BlockPos::new(3, 4, 0), BlockPos::new(0, 0, 0)),
            25.0
        );
    }
}
