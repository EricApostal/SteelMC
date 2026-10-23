//! Shared vanilla `WaterAnimal` / `AbstractFish` / `AbstractSchoolingFish` behaviour.
//!
//! Vanilla puts this behaviour in three superclasses; Steel has no inheritance, so the
//! same code lives here as `pub(crate)` free functions plus [`SchoolingState`], the
//! runtime state vanilla keeps in `AbstractSchoolingFish` fields. Every fish entity
//! calls into this module from its own `Entity`/`LivingEntity`/`Mob` impls.
//!
//! Mirrored vanilla classes:
//!
//! - `net.minecraft.world.entity.animal.fish.WaterAnimal`
//! - `net.minecraft.world.entity.animal.fish.AbstractFish`
//! - `net.minecraft.world.entity.animal.fish.AbstractSchoolingFish`
//!
//! ## Deviations
//!
//! - `WaterAnimal.baseTick`'s air-supply override is a separate [`handle_air_supply`]
//!   call from each fish's [`Entity::base_tick`], because Steel's `Mob::base_tick_mob`
//!   is shared and cannot be overridden per class.
//! - Steel's `SpawnGroupData` carries no fish-leader slot and no tropical-fish variant
//!   slot, so a school's members elect a leader through `FollowFlockLeaderGoal` on their
//!   first tick and each tropical fish rolls its own variant instead of sharing the
//!   group variant (`AbstractSchoolingFish.SchoolSpawnGroupData` and
//!   `TropicalFish.TropicalFishGroupData` are therefore not modeled).
//! - `AbstractFish.FishMoveControl` is not ported; Steel's shared move control is used.
//! - `Bucketable.bucketMobPickup`'s `FILLED_BUCKET` advancement trigger and
//!   `Leashable.dropLeash` are not part of Steel's shared bucket path (see
//!   [`try_bucket_pickup`]).
//! - `AbstractSchoolingFish.tick` compares `EntityTypeRef`s instead of Java classes.

use std::sync::Arc;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::components::CustomData;
use steel_registry::data_components::vanilla_components::{BUCKET_ENTITY_DATA, CUSTOM_NAME};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{sound_events, vanilla_blocks, vanilla_damage_types, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, Downcast as _};

use crate::entity::ai::goal::{
    AvoidEntityGoal, FollowFlockLeaderGoal, Goal, GoalControls, PanicGoal, RandomSwimmingGoal,
    SchoolingFish,
};
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntitySpawnReason, LivingEntity, Mob, MobBase, PathfinderMob, RemovalReason,
    SharedEntity,
};
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::World;

use super::cod::CodEntity;
use super::salmon::SalmonEntity;
use super::tropical_fish::TropicalFishEntity;

/// Vanilla `WaterAnimal.AMBIENT_SOUND_INTERVAL`.
pub(crate) const AMBIENT_SOUND_INTERVAL: i32 = 120;
/// Vanilla `WaterAnimal.handleAirSupply` refill value.
pub(crate) const WATER_ANIMAL_AIR_SUPPLY: i32 = 300;
/// Vanilla `WaterAnimal.handleAirSupply` drowning damage.
pub(crate) const WATER_ANIMAL_DROWN_DAMAGE: f32 = 2.0;
/// Vanilla `WaterAnimal.checkSurfaceWaterAnimalSpawnRules` depth below sea level.
pub(crate) const SURFACE_SPAWN_DEPTH: i32 = 13;
/// Vanilla `AbstractFish.getMaxSpawnClusterSize`.
pub(crate) const MAX_SPAWN_CLUSTER_SIZE: i32 = 8;
/// Vanilla `AbstractFish.aiStep` flop impulse.
const FLOP_HORIZONTAL_IMPULSE: f64 = 0.05;
const FLOP_VERTICAL_IMPULSE: f64 = 0.4;
/// Vanilla `AbstractFish.travelInWater` input speed.
const WATER_TRAVEL_SPEED: f32 = 0.01;
/// Vanilla `AbstractFish.travelInWater` drag and sink impulse.
const WATER_TRAVEL_DRAG: f64 = 0.9;
const WATER_SINK_IMPULSE: f64 = -0.005;
/// Vanilla `AbstractFish` flop sound volume/pitch (`makeSound` -> `getSoundVolume`,
/// `getVoicePitch`).
const FLOP_SOUND_VOLUME: f32 = 0.4;
const FLOP_SOUND_PITCH: f32 = 1.0;
/// Vanilla `Bucketable.bucketMobPickup` plays the pickup sound at volume/pitch 1.
const BUCKET_SOUND_VOLUME: f32 = 1.0;
const BUCKET_SOUND_PITCH: f32 = 1.0;
/// Vanilla `AbstractSchoolingFish.tick` swarm reset chance (`nextInt(200) == 1`).
const SWARM_RESET_BOUND: i32 = 200;
/// Vanilla `AbstractSchoolingFish.tick` neighbour search inflation.
const SWARM_SEARCH_RADIUS: f64 = 8.0;
/// Vanilla `AbstractSchoolingFish.inRangeOfLeader` (`distanceToSqr <= 121.0`).
const LEADER_RANGE_SQR: f64 = 121.0;

/// Vanilla `WaterAnimal.checkSurfaceWaterAnimalSpawnRules`, the `SpawnPlacements`
/// predicate for cod, salmon and pufferfish.
///
/// Vanilla ignores `entity_type`, `spawn_reason` and `random`; the position must be at
/// most 13 blocks below sea level and at most at sea level, the block below must carry
/// water fluid and the block above must be a full water block.
#[must_use]
pub(crate) fn check_surface_water_animal_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    _spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    let min_spawn_level = level.sea_level() - SURFACE_SPAWN_DEPTH;
    pos.y() >= min_spawn_level
        && pos.y() <= level.sea_level()
        && level
            .get_block_state(pos.below())
            .get_fluid_state()
            .is_water()
        && level.get_block_state(pos.above()).get_block() == &vanilla_blocks::WATER
}

/// Vanilla `EntityGetter.isUnobstructed(Entity)`: no other building entity intersects
/// this entity's bounding box. Water creatures use this instead of
/// `Mob.checkSpawnObstruction`'s liquid test.
#[must_use]
pub(crate) fn check_water_animal_spawn_obstruction(
    world: &Arc<World>,
    entity: &dyn Entity,
) -> bool {
    let aabb = entity.bounding_box();
    !world.has_entity_in_aabb_matching(&aabb, |other| {
        other.id() != entity.id()
            && !other.is_removed()
            && other.blocks_building()
            && !other.is_passenger_of_same_vehicle(entity.as_entity_event_source())
    })
}

/// Vanilla `WaterAnimal.handleAirSupply`: drown after `shouldTakeDrowningDamage`, refill
/// to 300 out of water.
///
/// Vanilla calls this from the `WaterAnimal.baseTick` override *after* the generic
/// `LivingEntity.baseTick` air tick; Steel's fish call it at the same point from
/// `Entity::base_tick`.
pub(crate) fn handle_air_supply(entity: &dyn LivingEntity, pre_tick_air_supply: i32) {
    if LivingEntity::is_alive(entity) && !entity.is_in_water() {
        entity.set_air_supply(pre_tick_air_supply - 1);
        if entity.should_take_drowning_damage() {
            entity.set_air_supply(0);
            if let Some(world) = entity.level() {
                entity.hurt(
                    &world,
                    &DamageSource::environment(&vanilla_damage_types::DROWN),
                    WATER_ANIMAL_DROWN_DAMAGE,
                );
            }
        }
    } else {
        entity.set_air_supply(WATER_ANIMAL_AIR_SUPPLY);
    }
}

/// Vanilla `WaterAnimal.getBaseExperienceReward`.
///
/// Steel routes this through `Mob::base_experience_reward_mob`, the non-animal branch of
/// `LivingEntity::base_experience_reward`.
#[must_use]
pub(crate) fn base_experience_reward() -> i32 {
    1 + rand::random_range(0..3)
}

/// Vanilla `WaterAnimal` constructor: `setPathfindingMalus(WATER, 0.0F)`.
pub(crate) fn initialize_water_pathfinding_malus(mob_base: &MobBase) {
    mob_base
        .pathfinding_malus()
        .lock()
        .set(PathType::Water, 0.0);
}

/// Vanilla `AbstractFish.registerGoals`: panic, avoid players, random swim.
pub(crate) fn register_abstract_fish_goals(mob_base: &MobBase) {
    let mut goal_selector = mob_base.goal_selector().lock();
    goal_selector.add_goal(0, PanicGoal::new(1.25));
    // Vanilla `AvoidEntityGoal<>(this, Player.class, 8.0F, 1.6, 1.4,
    // EntitySelector.NO_SPECTATORS)`.
    goal_selector.add_goal(
        2,
        AvoidEntityGoal::with_predicates(
            8.0,
            1.6,
            1.4,
            |entity, _world| entity.as_player().is_some(),
            |target, _world| !target.is_spectator(),
        ),
    );
    goal_selector.add_goal(4, FishSwimGoal::new(1.0, 40));
}

/// Vanilla `AbstractSchoolingFish.registerGoals` addition.
pub(crate) fn register_schooling_goals(mob_base: &MobBase) {
    mob_base
        .goal_selector()
        .lock()
        .add_goal(5, FollowFlockLeaderGoal::new());
}

/// Vanilla `AbstractFish.FishSwimGoal`: `RandomSwimmingGoal` gated on `canRandomSwim`.
pub(crate) struct FishSwimGoal {
    swim: RandomSwimmingGoal,
}

impl FishSwimGoal {
    const fn new(speed_modifier: f64, interval: i32) -> Self {
        Self {
            swim: RandomSwimmingGoal::new(speed_modifier, interval),
        }
    }

    /// Vanilla `AbstractFish.canRandomSwim` (`true`) narrowed by
    /// `AbstractSchoolingFish.canRandomSwim` (`!isFollower()`).
    fn can_random_swim(mob: &dyn PathfinderMob) -> bool {
        !mob.as_schooling_fish()
            .is_some_and(|fish| fish.is_follower())
    }
}

impl Goal for FishSwimGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::can_random_swim(mob) && self.swim.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.swim.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.swim.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.swim.stop(mob);
    }
}

/// Vanilla `AbstractFish.travelInWater`.
pub(crate) fn travel_in_water(entity: &SharedEntity, input: DVec3) -> Option<MoveResult> {
    entity.move_relative(WATER_TRAVEL_SPEED, input);
    let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, entity.velocity())?;
    entity.set_velocity(entity.velocity() * WATER_TRAVEL_DRAG);
    if entity.as_mob().is_none_or(|mob| mob.target().is_none()) {
        entity.set_velocity(entity.velocity() + DVec3::new(0.0, WATER_SINK_IMPULSE, 0.0));
    }
    Some(result)
}

/// Vanilla `AbstractFish.aiStep` flop: bounce and play the flop sound on land.
pub(crate) fn tick_flop(entity: &dyn Entity, flop_sound: SoundEventRef) {
    if entity.is_in_water() || !entity.on_ground() || !entity.vertical_collision() {
        return;
    }

    let velocity = entity.velocity();
    entity.set_velocity(DVec3::new(
        velocity.x + f64::from(rand::random::<f32>() * 2.0 - 1.0) * FLOP_HORIZONTAL_IMPULSE,
        FLOP_VERTICAL_IMPULSE,
        velocity.z + f64::from(rand::random::<f32>() * 2.0 - 1.0) * FLOP_HORIZONTAL_IMPULSE,
    ));
    entity.set_on_ground(false);
    entity.play_sound(flop_sound, FLOP_SOUND_VOLUME, FLOP_SOUND_PITCH);
}

/// Vanilla `Bucketable.bucketMobPickup`: a water bucket captures this fish.
///
/// Deviation: the `FILLED_BUCKET` advancement trigger and `Leashable.dropLeash` are not
/// part of Steel's shared bucket path (same as `TadpoleEntity`/`AxolotlEntity`).
pub(crate) fn try_bucket_pickup(
    entity: &dyn Entity,
    player: &Player,
    hand: InteractionHand,
    pickup_sound: SoundEventRef,
    create_bucket_stack: impl FnOnce() -> ItemStack,
) -> bool {
    let holds_water_bucket = {
        let inventory = player.inventory.lock();
        inventory
            .get_item_in_hand(hand)
            .is(&vanilla_items::WATER_BUCKET)
    };
    if !holds_water_bucket || !entity.is_alive() {
        return false;
    }

    entity.play_sound(pickup_sound, BUCKET_SOUND_VOLUME, BUCKET_SOUND_PITCH);
    let bucket = create_bucket_stack();
    let overflow = {
        let mut inventory = player.inventory.lock();
        inventory.apply_filled_result(hand, bucket, player.has_infinite_materials(), false)
    };
    if !overflow.is_empty() {
        let _ = player.drop_item(overflow, false, Prediction::Predicted);
    }

    entity.set_removed(RemovalReason::Discarded);
    true
}

/// Vanilla `Bucketable.saveDefaultDataToBucketTag`.
pub(crate) fn save_default_bucket_data(entity: &dyn Mob, stack: &mut ItemStack) {
    let mut tag = NbtCompound::new();
    if entity.is_no_ai() {
        tag.insert("NoAI", 1i8);
    }
    if entity.is_silent() {
        tag.insert("Silent", 1i8);
    }
    if entity.is_no_gravity() {
        tag.insert("NoGravity", 1i8);
    }
    if entity.has_glowing_tag() {
        tag.insert("Glowing", 1i8);
    }
    if entity.is_invulnerable() {
        tag.insert("Invulnerable", 1i8);
    }
    if entity.is_persistence_required() {
        tag.insert("PersistenceRequired", 1i8);
    }
    tag.insert("Health", entity.get_health());

    if let Some(custom_data) = CustomData::try_from_compound(tag) {
        stack.set(BUCKET_ENTITY_DATA, custom_data);
    }
    if let Some(custom_name) = entity.custom_name() {
        stack.set(CUSTOM_NAME, custom_name);
    }
}

/// Vanilla `Bucketable.loadDefaultDataFromBucketTag`.
pub(crate) fn load_default_bucket_data(entity: &dyn Mob, nbt: BorrowedNbtCompoundView<'_, '_>) {
    if let Some(no_ai) = nbt.byte("NoAI") {
        entity.set_no_ai(no_ai != 0);
    }
    if let Some(silent) = nbt.byte("Silent") {
        entity.set_silent(silent != 0);
    }
    if let Some(no_gravity) = nbt.byte("NoGravity") {
        entity.set_no_gravity(no_gravity != 0);
    }
    if let Some(glowing) = nbt.byte("Glowing") {
        entity.set_glowing_tag(glowing != 0);
    }
    if let Some(invulnerable) = nbt.byte("Invulnerable") {
        entity.set_invulnerable(invulnerable != 0);
    }
    if nbt
        .byte("PersistenceRequired")
        .is_some_and(|value| value != 0)
    {
        entity.set_persistence_required();
    }
    if let Some(health) = nbt.float("Health") {
        entity.set_health(health);
    }
}

/// Vanilla `AbstractFish.requiresCustomPersistence` (the `super` half is
/// `Mob.requiresCustomPersistence`).
pub(crate) fn requires_custom_persistence(entity: &dyn Mob, from_bucket: bool) -> bool {
    Mob::requires_custom_persistence(entity) || from_bucket
}

/// Vanilla `AbstractFish.removeWhenFarAway`.
pub(crate) fn remove_when_far_away(entity: &dyn Mob, from_bucket: bool) -> bool {
    !from_bucket && entity.custom_name().is_none()
}

/// Vanilla `AbstractFish.getSwimSound`.
#[must_use]
pub(crate) fn swim_sound() -> SoundEventRef {
    &sound_events::ENTITY_FISH_SWIM
}

/// Vanilla `AbstractSchoolingFish` flock membership state.
#[derive(Debug)]
pub(crate) struct SchoolingState {
    /// Vanilla `AbstractSchoolingFish.leader`.
    leader: Option<SharedEntity>,
    /// Vanilla `AbstractSchoolingFish.schoolSize`, initialized to 1.
    school_size: i32,
}

impl SchoolingState {
    /// Vanilla `AbstractSchoolingFish` field initialization (`schoolSize = 1`).
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            leader: None,
            school_size: 1,
        }
    }

    /// Vanilla `AbstractSchoolingFish.hasFollowers`.
    #[must_use]
    pub(crate) fn has_followers(&self) -> bool {
        self.school_size > 1
    }

    /// Vanilla `AbstractSchoolingFish.isFollower`.
    #[must_use]
    pub(crate) fn is_follower(&self) -> bool {
        self.leader.as_ref().is_some_and(|leader| leader.is_alive())
    }

    /// Vanilla `AbstractSchoolingFish.canBeFollowed`.
    #[must_use]
    pub(crate) fn can_be_followed(&self, max_school_size: i32) -> bool {
        self.has_followers() && self.school_size < max_school_size
    }

    /// Vanilla `AbstractSchoolingFish.schoolSize`.
    #[must_use]
    pub(crate) fn school_size(&self) -> i32 {
        self.school_size
    }

    /// Vanilla `AbstractSchoolingFish.addFollower`.
    fn add_follower(&mut self) {
        self.school_size += 1;
    }

    /// Vanilla `AbstractSchoolingFish.removeFollower`.
    fn remove_follower(&mut self) {
        self.school_size -= 1;
    }

    /// Vanilla `AbstractSchoolingFish.startFollowing` / `stopFollowing` leader write.
    fn take_leader(&mut self) -> Option<SharedEntity> {
        self.leader.take()
    }

    /// Vanilla `AbstractSchoolingFish.startFollowing` leader write.
    fn set_leader(&mut self, leader: SharedEntity) {
        self.leader = Some(leader);
    }

    /// Vanilla `AbstractSchoolingFish.tick` swarm reset.
    fn reset_school_size(&mut self) {
        self.school_size = 1;
    }
}

/// Returns the flock state of a schooling fish, downcasting the concrete entity.
///
/// Steel's `SchoolingFish` capability trait is read-only, so the `addFollowers` /
/// `stopFollowing` bookkeeping needs the concrete state of the *other* fish.
fn schooling_state_of(entity: &dyn Entity) -> Option<&SyncMutex<SchoolingState>> {
    let entity = entity.as_entity_event_source();
    if let Some(cod) = entity.downcast_ref::<CodEntity>() {
        return Some(cod.schooling_state());
    }
    if let Some(salmon) = entity.downcast_ref::<SalmonEntity>() {
        return Some(salmon.schooling_state());
    }
    if let Some(fish) = entity.downcast_ref::<TropicalFishEntity>() {
        return Some(fish.schooling_state());
    }
    None
}

/// Vanilla `AbstractSchoolingFish.startFollowing`.
fn start_following(
    follower: &dyn Entity,
    leader: &SharedEntity,
    leader_state: &SyncMutex<SchoolingState>,
) {
    let Some(follower_state) = schooling_state_of(follower) else {
        return;
    };
    follower_state.lock().set_leader(Arc::clone(leader));
    leader_state.lock().add_follower();
}

/// Vanilla `AbstractSchoolingFish.addFollowers`.
///
/// Deviation: the leader handle is resolved from the world entity map because the
/// capability trait hands goals only `&dyn PathfinderMob`; a fish that is not registered
/// in a world cannot hand its handle to followers.
pub(crate) fn add_followers(
    state: &SyncMutex<SchoolingState>,
    entity: &dyn Entity,
    followers: Vec<SharedEntity>,
    max_school_size: i32,
) {
    let remaining = max_school_size - state.lock().school_size;
    if remaining <= 0 {
        return;
    }
    let Some(leader) = entity
        .level()
        .and_then(|world| world.get_entity_by_id(entity.id()))
    else {
        return;
    };

    for follower in followers.into_iter().take(remaining as usize) {
        if follower.id() == entity.id() {
            continue;
        }
        start_following(follower.as_ref(), &leader, state);
    }
}

/// Vanilla `AbstractSchoolingFish.stopFollowing`.
pub(crate) fn stop_following(state: &SyncMutex<SchoolingState>) {
    let leader = state.lock().take_leader();
    let Some(leader) = leader else {
        return;
    };
    if let Some(leader_state) = schooling_state_of(leader.as_ref()) {
        leader_state.lock().remove_follower();
    }
}

/// Vanilla `AbstractSchoolingFish.inRangeOfLeader`.
pub(crate) fn in_range_of_leader(state: &SyncMutex<SchoolingState>, entity: &dyn Entity) -> bool {
    let leader = state.lock().leader.clone();
    leader.is_some_and(|leader| entity.distance_to_sqr(leader.position()) <= LEADER_RANGE_SQR)
}

/// Vanilla `AbstractSchoolingFish.pathToLeader`.
pub(crate) fn path_to_leader(state: &SyncMutex<SchoolingState>, mob: &SharedEntity) {
    let leader = {
        let state = state.lock();
        if !state.is_follower() {
            return;
        }
        state.leader.clone()
    };
    let Some(leader) = leader else {
        return;
    };
    if let Some(pathfinder) = mob.as_pathfinder_mob() {
        pathfinder.move_to_pos(leader.position(), 1.0);
    }
}

/// Vanilla `AbstractSchoolingFish.tick` swarm reset: a leader that finds at most one
/// same-type fish within 8 blocks drops back to a school of one.
pub(crate) fn tick_schooling(entity: &dyn Entity) {
    let Some(state) = schooling_state_of(entity) else {
        return;
    };
    if !state.lock().has_followers() {
        return;
    }
    if rand::random_range(0..SWARM_RESET_BOUND) != 1 {
        return;
    }

    let Some(world) = entity.level() else {
        return;
    };
    let entity_type = entity.entity_type();
    let neighbors = world.get_entities_in_aabb_matching(
        &entity.bounding_box().inflate(SWARM_SEARCH_RADIUS),
        |other| other.entity_type() == entity_type,
    );
    if neighbors.len() <= 1 {
        state.lock().reset_school_size();
    }
}

/// Vanilla `AbstractSchoolingFish.getMaxSpawnClusterSize`, which returns
/// `getMaxSchoolSize`.
#[must_use]
pub(crate) fn max_spawn_cluster_size(max_school_size: i32) -> i32 {
    max_school_size
}
