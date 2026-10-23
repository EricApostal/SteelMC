//! Vanilla `HappyGhast` (`net.minecraft.world.entity.animal.happyghast.HappyGhast`).
//!
//! A flying `Animal` that babies ride as a brain mob ([`brain`] mirrors
//! `HappyGhastAi`), grows into a four-seat harness vehicle, heals itself in clouds
//! and rain, and stays put while a player stands on its roof.
//!
//! ## Deviations
//!
//! - Vanilla swaps the `moveControl`/`lookControl`/`navigation` objects in
//!   `babyGhastSetup`/`adultGhastSetup`; Steel's controls live on `MobBase`, so
//!   [`HappyGhastEntity::tick_move_control`] and [`HappyGhastEntity::tick_look_control`]
//!   branch on the baby flag and the navigation is reconfigured in place.
//! - `HappyGhastAi.initCoreActivity` uses `new AnimalPanic(2.0F, 0)`, the flying
//!   panic variant; Steel's `AnimalPanic` only has the land/water position getter.
//! - `TemptGoal.ForNonPathfinders` has no Steel constructor and its item predicate
//!   reads entity state, so [`HappyGhastTemptGoal`] reproduces it locally.
//! - GAP(happy-ghast-position-sync): vanilla `setServerStillTimeout` sends a
//!   `ClientboundEntityPositionSyncPacket` to tracking players on the 0 -> >0
//!   transition; Steel has no entity packet-codec hook nor a
//!   "send to this entity's tracking players" helper, so only the runtime value and
//!   the synced `stays_still` flag change.
//! - GAP(happy-ghast-precise-position): vanilla `aiStep` calls
//!   `setRequiresPrecisePosition(isOnStillTimeout())`; Steel's movement sync has no
//!   per-entity precise-position requirement.
//! - GAP(happy-ghast-passenger-hooks): vanilla plays the harness sounds and adjusts
//!   the still timeout from `addPassenger`/`removePassenger`. Steel's boarding path
//!   (`start_riding_entities`) has no vehicle-side hook, so
//!   [`HappyGhastEntity::track_passenger_transitions`] performs the same work from
//!   `tick` when the passenger id list changes.
//! - GAP(happy-ghast-leash-hooks): the blanket `impl<T: Mob> Leashable for T` gives
//!   no hook for `leashElasticDistance`/`leashSnapDistance`/`onElasticLeashPull`/
//!   `getLeashOffset`/quad offsets; those are exposed as inherent methods and the
//!   defaults stay in force until `Mob` grows the hooks.
//! - GAP(happy-ghast-quad-leash-support): `Leashable` has no `supportQuadLeash`, so
//!   [`HappyGhastEntity::notify_leash_holder`] treats a happy ghast leashee as the
//!   quad-leash case (the only Steel entity declaring quad support).
//! - VANILLA CLIENT-ONLY: `canBeCollidedWith`'s client-side "player standing on the
//!   roof" branch and `HappyGhastBodyRotationControl` are client rendering, not
//!   ported.

mod brain;

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::{DEGREE_90, DEGREE_180, DEGREE_360, RAD_TO_DEG, trig, wrap_degrees};
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_data::EntityPose;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::HappyGhastEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_entities,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, WorldAabb};

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::control::{FlyingMoveControl, FlyingMoveControlInput, MoveControlOperation};
use crate::entity::ai::goal::{FloatGoal, Goal, GoalControls};
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, ENTITIES, Entity, EntityBase, EntityBaseLoad,
    EntitySpawnReason, EntitySyncedData, Leashable as _, LivingEntity, LivingEntityBase,
    LivingTravelInput, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::world::{HeightmapType, World};

/// Vanilla `HappyGhast.BABY_SCALE`.
pub const BABY_SCALE: f32 = 0.2375;
/// Vanilla `HappyGhast.BABY_DIMENSIONS` eye height.
const BABY_EYE_HEIGHT: f32 = 0.46875;
/// Vanilla `HappyGhast.WANDER_GROUND_DISTANCE`.
pub const WANDER_GROUND_DISTANCE: i32 = 16;
/// Vanilla `HappyGhast.SMALL_RESTRICTION_RADIUS`.
pub const SMALL_RESTRICTION_RADIUS: i32 = 32;
/// Vanilla `HappyGhast.LARGE_RESTRICTION_RADIUS`.
pub const LARGE_RESTRICTION_RADIUS: i32 = 64;
/// Vanilla `HappyGhast.RESTRICTION_RADIUS_BUFFER`.
pub const RESTRICTION_RADIUS_BUFFER: i32 = 16;
/// Vanilla `HappyGhast.FAST_HEALING_TICKS`.
pub const FAST_HEALING_TICKS: i32 = 20;
/// Vanilla `HappyGhast.SLOW_HEALING_TICKS`.
pub const SLOW_HEALING_TICKS: i32 = 600;
/// Vanilla `HappyGhast.MAX_PASSANGERS`.
pub const MAX_PASSENGERS: usize = 4;
/// Vanilla `HappyGhast.STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD`.
const STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD: i32 = 60;
/// Vanilla `HappyGhast.MAX_STILL_TIMEOUT`.
const MAX_STILL_TIMEOUT: i32 = 10;
/// Vanilla `Mob.getAmbientSoundInterval`.
const MOB_AMBIENT_SOUND_INTERVAL: i32 = 80;
/// Vanilla `HappyGhast.SPEED_MULTIPLIER_WHEN_PANICKING`.
pub const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.0;
/// Vanilla `HappyGhast` tame/tempt goal speed modifier.
const TEMPT_SPEED_MODIFIER: f64 = 1.0;
/// Vanilla `TemptGoal.ForNonPathfinders` stop distance for the happy ghast.
const TEMPT_STOP_DISTANCE: f64 = 7.0;
/// Vanilla `TemptGoal.stop()`'s `reducedTickDelay(100)` (`(100 + 1) / 2`).
const TEMPT_CALM_DOWN_TICKS: i32 = 50;
/// Vanilla `TemptGoal.canContinueToUse` "player barely moved" radius.
const TEMPT_SCARE_DISTANCE_SQR: f64 = 36.0;
/// Vanilla `TemptGoal.canContinueToUse` movement threshold.
const TEMPT_SCARE_MOVE_SQR: f64 = 0.01;
/// Vanilla `TemptGoal.canContinueToUse` rotation threshold, in degrees.
const TEMPT_SCARE_ROTATION: f32 = 5.0;
/// Vanilla `HappyGhast.travel` flying speed multiplier (`* 5.0F / 3.0F`).
const FLYING_SPEED_MULTIPLIER: f32 = 5.0 / 3.0;
/// Vanilla `HappyGhast.getRiddenInput` input scale (`3.9F * FLYING_SPEED`).
const RIDDEN_INPUT_SCALE: f32 = 3.9;
/// Vanilla `HappyGhast.tickRidden` yaw turn factor.
const RIDDEN_TURN_FACTOR: f32 = 0.08;
/// Vanilla `HappyGhast.tickRidden` ridden pitch factor.
const RIDDEN_PITCH_FACTOR: f32 = 0.5;
/// Vanilla `LivingEntity.travelFlying` in-water relative speed.
const FLYING_WATER_SPEED: f32 = 0.02;
/// Vanilla `LivingEntity.travelFlying` in-lava relative speed.
const FLYING_LAVA_SPEED: f32 = 0.02;
/// Vanilla `LivingEntity.travelFlying` drag values.
const FLYING_WATER_DRAG: f64 = 0.8;
const FLYING_LAVA_DRAG: f64 = 0.5;
const FLYING_AIR_DRAG: f64 = 0.91;
/// Vanilla `HappyGhast.BabyFlyingPathNavigation` required path length.
const BABY_REQUIRED_PATH_LENGTH: f32 = 48.0;
/// Vanilla `PathNavigation.requiredPathLength` default, reinstalled for adults.
const ADULT_REQUIRED_PATH_LENGTH: f32 = 16.0;
/// Vanilla `HappyGhast.leashElasticDistance`.
const LEASH_ELASTIC_DISTANCE: f64 = 10.0;
/// Vanilla `HappyGhast.leashSnapDistance`.
const LEASH_SNAP_DISTANCE: f64 = 16.0;
/// Vanilla `HappyGhast.notifyLeashHolder` sets `leashHolderTime = 5`.
const LEASH_HOLDER_TIME: i32 = 5;
/// Vanilla `Ghast.RandomFloatAroundGoal.MAX_ATTEMPTS`.
const FLOAT_ATTEMPTS: i32 = 64;
/// Vanilla `Ghast.RandomFloatAroundGoal` candidate spread (`* 16.0F`).
const FLOAT_SPREAD: f64 = 16.0;
/// Vanilla `Ghast.RandomFloatAroundGoal` re-target bounds.
const FLOAT_MIN_DISTANCE_SQR: f64 = 1.0;
const FLOAT_MAX_DISTANCE_SQR: f64 = 3600.0;
/// Vanilla `Ghast.GhastMoveControl.floatDuration` range.
const MOVE_FLOAT_DURATION_BASE: i32 = 2;
const MOVE_FLOAT_DURATION_RANGE: i32 = 5;
/// Vanilla `GhastMoveControl.canReach` arrival threshold (`travel.lengthSqr() < 1.0`).
const MOVE_REACH_DISTANCE_SQR: f64 = 1.0;
/// Vanilla `FlyingMoveControl(this, 180, true)`.
const BABY_FLYING_MOVE_CONTROL: FlyingMoveControl = FlyingMoveControl::new(180, true);
/// The six block-face directions, in the same order as `Direction.values()`.
const BLOCK_DIRECTIONS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

/// Runtime state vanilla keeps in `HappyGhast` fields rather than synched data.
#[derive(Debug, Clone, Copy, Default)]
struct HappyGhastState {
    leash_holder_time: i32,
    server_still_timeout: i32,
    /// Vanilla `Ghast.GhastMoveControl.floatDuration`.
    float_duration: i32,
    /// Steel-only: last observed direct passenger ids, for the vanilla
    /// `addPassenger`/`removePassenger` side effects (see the module docs).
    last_passenger_ids: Vec<i32>,
}

#[entity_behavior(class = "HappyGhast")]
/// Vanilla happy ghast: a baby brain mob that grows into a four-seat flying vehicle.
pub struct HappyGhastEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    animal_base: AnimalBase,
    ageable_base: AgeableMobBase,
    entity_data: SyncMutex<HappyGhastEntityData>,
    state: SyncMutex<HappyGhastState>,
    brain: SyncMutex<Brain>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `HappyGhastEntity`.
unsafe impl DowncastType for HappyGhastEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/happy_ghast");
}

impl HappyGhastEntity {
    /// Creates a new happy ghast at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a happy ghast from persisted base entity state.
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
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = HappyGhastEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            animal_base: AnimalBase::new(),
            ageable_base: AgeableMobBase::new(),
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(HappyGhastState::default()),
            brain: SyncMutex::new(Brain::default()),
        };
        // Vanilla `HappyGhast.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        *entity.brain.lock() = brain::make_brain(&entity);
        // Vanilla `Mob` constructor -> `registerGoals`.
        entity.register_goals();
        entity
    }

    /// Vanilla `HappyGhast.registerGoals`.
    fn register_goals(&self) {
        let mut goal_selector = self.mob_base.goal_selector().lock();
        // Vanilla `new HappyGhastFloatGoal()`.
        goal_selector.add_goal(3, HappyGhastFloatGoal::new(&self.mob_base));
        // Vanilla `new TemptGoal.ForNonPathfinders(this, 1.0, predicate, false, 7.0)`.
        goal_selector.add_goal(
            4,
            HappyGhastTemptGoal::new(TEMPT_SPEED_MODIFIER, TEMPT_STOP_DISTANCE),
        );
        // Vanilla `new Ghast.RandomFloatAroundGoal(this, 16)`.
        goal_selector.add_goal(5, HappyGhastRandomFloatGoal::new(WANDER_GROUND_DISTANCE));
    }

    /// Vanilla `HappyGhast.babyGhastSetup`.
    fn baby_ghast_setup(&self) {
        {
            let mut navigation = self.mob_base.navigation().lock();
            // Vanilla `BabyFlyingPathNavigation`: a flying navigation that cannot
            // open doors, floats, and requires a 48-block path.
            navigation.set_flying(true);
            navigation.set_can_open_doors(false);
            navigation.set_can_float(true);
            let follow_range = self
                .attributes()
                .lock()
                .required_value(vanilla_attributes::FOLLOW_RANGE);
            navigation.set_required_path_length(BABY_REQUIRED_PATH_LENGTH, follow_range);
        }
        self.set_server_still_timeout(0);
        PathfinderMob::remove_all_goals(self, |_| true);
    }

    /// Vanilla `HappyGhast.adultGhastSetup`.
    fn adult_ghast_setup(&self) {
        {
            let mut navigation = self.mob_base.navigation().lock();
            // Vanilla reinstalls `Mob.createNavigation` -> `GroundPathNavigation`.
            navigation.set_flying(false);
            navigation.set_can_open_doors(false);
            let follow_range = self
                .attributes()
                .lock()
                .required_value(vanilla_attributes::FOLLOW_RANGE);
            navigation.set_required_path_length(ADULT_REQUIRED_PATH_LENGTH, follow_range);
        }
        PathfinderMob::remove_all_goals(self, |_| true);
        self.register_goals();

        if let Some(world) = self.level()
            && let Some(entity) = world.get_entity_by_id(self.id())
        {
            let mut brain = self.brain.lock();
            brain.stop_all(world.as_ref(), &entity);
            brain.clear_memories();
        }
    }

    /// Vanilla `HappyGhast.serverStillTimeout`.
    #[must_use]
    pub fn server_still_timeout(&self) -> i32 {
        self.state.lock().server_still_timeout
    }

    /// Vanilla `HappyGhast.setServerStillTimeout`.
    ///
    /// GAP(happy-ghast-position-sync): see the module docs for the missing
    /// `ClientboundEntityPositionSyncPacket` broadcast.
    fn set_server_still_timeout(&self, server_still_timeout: i32) {
        self.state.lock().server_still_timeout = server_still_timeout;
        self.sync_stay_still_flag();
    }

    /// Vanilla `HappyGhast.syncStayStillFlag`.
    fn sync_stay_still_flag(&self) {
        let stays_still = self.state.lock().server_still_timeout > 0;
        self.entity_data.lock().stays_still.set(stays_still);
    }

    /// Vanilla `HappyGhast.staysStill`.
    #[must_use]
    pub fn stays_still(&self) -> bool {
        *self.entity_data.lock().stays_still.get()
    }

    /// Vanilla `HappyGhast.isOnStillTimeout`.
    #[must_use]
    pub fn is_on_still_timeout(&self) -> bool {
        self.stays_still() || self.server_still_timeout() > 0
    }

    /// Vanilla `HappyGhast.isLeashHolder`.
    #[must_use]
    pub fn is_leash_holder(&self) -> bool {
        *self.entity_data.lock().is_leash_holder.get()
    }

    /// Vanilla `HappyGhast.setLeashHolder`.
    fn set_leash_holder(&self, is_leash_holder: bool) {
        self.entity_data
            .lock()
            .is_leash_holder
            .set(is_leash_holder);
    }

    /// Vanilla `HappyGhast.leashHolderTime`.
    #[must_use]
    pub fn leash_holder_time(&self) -> i32 {
        self.state.lock().leash_holder_time
    }

    /// Vanilla `HappyGhast.scanPlayerAboveGhast`.
    #[must_use]
    pub fn scan_player_above_ghast(&self) -> bool {
        let bounding_box = self.bounding_box();
        let detection_box = WorldAabb::new(
            bounding_box.min_x() - 1.0,
            bounding_box.max_y() - 1.0e-5,
            bounding_box.min_z() - 1.0,
            bounding_box.max_x() + 1.0,
            bounding_box.max_y() + bounding_box.height() / 2.0,
            bounding_box.max_z() + 1.0,
        );
        let Some(world) = self.level() else {
            return false;
        };

        let mut found = false;
        world.players.iter_players(|_, player| {
            if player.is_spectator() {
                return true;
            }
            let root_vehicle = player.root_vehicle();
            let riding_ghast = root_vehicle
                .as_ref()
                .is_some_and(|vehicle| vehicle.entity_type() == &vanilla_entities::HAPPY_GHAST);
            let position = root_vehicle
                .as_ref()
                .map_or_else(|| player.position(), |vehicle| vehicle.position());
            if !riding_ghast && detection_box.contains(position) {
                found = true;
                return false;
            }
            true
        });
        found
    }

    /// Vanilla `HappyGhast.getHappyGhastRestrictionRadius`.
    #[must_use]
    pub fn restriction_radius(&self) -> i32 {
        if !AgeableMob::is_baby(self) && self.equipment_in_slot(EquipmentSlot::Body).is_empty() {
            LARGE_RESTRICTION_RADIUS
        } else {
            SMALL_RESTRICTION_RADIUS
        }
    }

    /// Vanilla `HappyGhast.checkRestriction`.
    fn check_restriction(&self) {
        if self.is_leashed() || self.is_vehicle() {
            return;
        }

        let radius = self.restriction_radius();
        let home = self.home_position();
        let position = self.block_position();
        let dx = f64::from(home.x() - position.x());
        let dy = f64::from(home.y() - position.y());
        let dz = f64::from(home.z() - position.z());
        let distance = dx.mul_add(dx, dy.mul_add(dy, dz * dz)).sqrt();
        let closer_than = distance < f64::from(radius + RESTRICTION_RADIUS_BUFFER);
        if !self.has_home() || !closer_than || radius != self.home_radius() {
            self.set_home_to(position, radius);
        }
    }

    /// Vanilla `HappyGhast.continuousHeal`.
    fn continuous_heal(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if !LivingEntity::is_alive(self)
            || self.is_dead_or_dying()
            || self.get_max_health() == self.get_health()
        {
            return;
        }

        let block_position = self.block_position();
        // Vanilla `level.precipitationAt(pos) != NONE`; Steel exposes the same
        // predicate as rain + sky exposure + biome precipitation.
        let precipitation = world.is_raining()
            && world.can_see_sky(block_position)
            && world
                .biome_at(block_position)
                .is_some_and(|biome| biome.has_precipitation);
        let is_fast_healing = self.is_in_clouds(&world) || precipitation;
        let period = if is_fast_healing {
            FAST_HEALING_TICKS
        } else {
            SLOW_HEALING_TICKS
        };
        if self.tick_count() % period == 0 {
            LivingEntity::heal(self, 1.0);
        }
    }

    /// Vanilla `Entity.isInClouds`, using the dimension type's cloud height.
    ///
    /// Deviation: vanilla also requires a non-zero `CLOUD_COLOR` alpha; Steel has no
    /// environment-attribute system, so the cloud height alone decides.
    #[must_use]
    pub fn is_in_clouds(&self, world: &World) -> bool {
        let Some(cloud_bottom) = world.dimension_type.cloud_height else {
            return false;
        };
        let cloud_bottom = f64::from(cloud_bottom);
        if self.position().y + self.bounding_box().height() < cloud_bottom {
            return false;
        }
        let cloud_top = cloud_bottom + 4.0;
        self.position().y <= cloud_top
    }

    /// Vanilla `Mob.isWearingBodyArmor`.
    #[must_use]
    pub fn is_wearing_body_armor(&self) -> bool {
        let mut wearing = false;
        self.with_equipment_slot(EquipmentSlot::Body, &mut |item_stack| {
            wearing = self.is_equippable_in_slot(item_stack, EquipmentSlot::Body);
        });
        wearing
    }

    /// Vanilla `HappyGhast.doPlayerRide`.
    fn do_player_ride(&self, player: &Player) {
        if self.level().is_none() {
            return;
        }
        if let Some(world) = self.level()
            && let Some(vehicle) = world.get_entity_by_id(self.id())
        {
            player.start_riding(&vehicle);
        }
    }

    /// Vanilla `HappyGhast.isOnStillTimeout` for goal predicates.
    fn still_timeout_of(mob: &dyn PathfinderMob) -> bool {
        mob.as_entity_event_source()
            .downcast_ref::<HappyGhastEntity>()
            .is_some_and(HappyGhastEntity::is_on_still_timeout)
    }

    /// Vanilla `Ghast.faceMovementDirection`.
    fn face_movement_direction(&self) {
        let (_, pitch) = self.rotation();
        let movement = self.velocity();
        let yaw = -((movement.x as f32).atan2(movement.z as f32)).to_degrees();
        self.set_rotation((yaw, pitch));
        self.set_y_body_rot(yaw);
    }

    /// Vanilla `HappyGhast.tick` server-side half.
    fn tick_happy_ghast_server_state(&self) {
        if self.level().is_none() {
            return;
        }

        let (leash_holder_time, still_timeout) = {
            let mut state = self.state.lock();
            if state.leash_holder_time > 0 {
                state.leash_holder_time -= 1;
            }
            (state.leash_holder_time, state.server_still_timeout)
        };
        self.set_leash_holder(leash_holder_time > 0);

        if still_timeout > 0 {
            let next = if self.tick_count() > STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD {
                still_timeout - 1
            } else {
                still_timeout
            };
            self.set_server_still_timeout(next);
        }

        if self.scan_player_above_ghast() {
            self.set_server_still_timeout(MAX_STILL_TIMEOUT);
        }

        self.track_passenger_transitions();
    }

    /// Vanilla `HappyGhast.addPassenger`/`removePassenger` side effects, observed
    /// from the passenger list because Steel has no vehicle-side boarding hook.
    fn track_passenger_transitions(&self) {
        let current = self
            .passengers()
            .iter()
            .map(|passenger| passenger.id())
            .collect::<Vec<_>>();
        let previous = {
            let mut state = self.state.lock();
            std::mem::replace(&mut state.last_passenger_ids, current.clone())
        };
        if previous == current {
            return;
        }

        if previous.is_empty() && !current.is_empty() {
            // Vanilla `addPassenger`: `if (!this.isVehicle()) playSound(...)`.
            self.play_sound(&sound_events::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN, 1.0, 1.0);
            if self.level().is_some() {
                if !self.scan_player_above_ghast() {
                    self.set_server_still_timeout(0);
                } else if self.server_still_timeout() > MAX_STILL_TIMEOUT {
                    self.set_server_still_timeout(MAX_STILL_TIMEOUT);
                }
            }
        } else if !previous.is_empty() && current.is_empty() {
            // Vanilla `removePassenger`: reset the timeout, then clear home/sound.
            if self.level().is_some() {
                self.set_server_still_timeout(MAX_STILL_TIMEOUT);
            }
            self.clear_home();
            self.play_sound(&sound_events::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP, 1.0, 1.0);
        }
    }

    /// Vanilla `HappyGhast.getRiddenRotation`.
    #[must_use]
    pub fn get_ridden_rotation(&self, controller: &dyn LivingEntity) -> (f32, f32) {
        let (yaw, pitch) = controller.rotation();
        (pitch * RIDDEN_PITCH_FACTOR, yaw)
    }

    /// Vanilla `HappyGhast.getDismountLocationForPassenger`.
    #[must_use]
    pub fn get_dismount_location_for_passenger(&self, _passenger: &dyn LivingEntity) -> DVec3 {
        DVec3::new(self.position().x, self.bounding_box().max_y(), self.position().z)
    }

    /// Vanilla `HappyGhast.supportQuadLeashAsHolder`.
    ///
    /// GAP(happy-ghast-leash-hooks): `Leashable` has no `support_quad_leash_as_holder`
    /// hook, so the leash spring still uses the single attachment point.
    #[must_use]
    pub const fn support_quad_leash_as_holder(&self) -> bool {
        true
    }

    /// Vanilla `Leashable.createQuadLeashOffsets`.
    #[must_use]
    pub fn create_quad_leash_offsets(
        &self,
        front_offset: f64,
        front_back: f64,
        left_right: f64,
        height: f64,
    ) -> [DVec3; 4] {
        let dimensions = self.dimensions_for_pose(self.pose());
        let width = f64::from(dimensions.width);
        let front_offset_scaled = front_offset * width;
        let front_back_scaled = front_back * width;
        let left_right_scaled = left_right * width;
        let height_scaled = height * f64::from(dimensions.height);
        [
            DVec3::new(
                -left_right_scaled,
                height_scaled,
                front_back_scaled + front_offset_scaled,
            ),
            DVec3::new(
                -left_right_scaled,
                height_scaled,
                -front_back_scaled + front_offset_scaled,
            ),
            DVec3::new(
                left_right_scaled,
                height_scaled,
                -front_back_scaled + front_offset_scaled,
            ),
            DVec3::new(
                left_right_scaled,
                height_scaled,
                front_back_scaled + front_offset_scaled,
            ),
        ]
    }

    /// Vanilla `HappyGhast.getQuadLeashHolderOffsets`.
    #[must_use]
    pub fn quad_leash_holder_offsets(&self) -> [DVec3; 4] {
        self.create_quad_leash_offsets(-0.03125, 0.4375, 0.46875, 0.03125)
    }

    /// Vanilla `HappyGhast.getLeashOffset`.
    #[must_use]
    pub const fn leash_offset(&self) -> DVec3 {
        DVec3::ZERO
    }

    /// Vanilla `HappyGhast.leashElasticDistance`.
    #[must_use]
    pub const fn leash_elastic_distance(&self) -> f64 {
        LEASH_ELASTIC_DISTANCE
    }

    /// Vanilla `HappyGhast.leashSnapDistance`.
    #[must_use]
    pub const fn leash_snap_distance(&self) -> f64 {
        LEASH_SNAP_DISTANCE
    }

    /// Vanilla `HappyGhast.onElasticLeashPull`.
    pub fn on_elastic_leash_pull(&self) {
        self.check_fall_distance_accumulation();
        self.mob_base().controls().lock().move_control.set_wait();
    }

    /// Vanilla `Mob.stopInPlace`.
    fn stop_in_place(&self) {
        self.mob_base().navigation().lock().stop();
        self.set_mob_speed(0.0);
        self.set_travel_input(LivingTravelInput::ZERO);
        self.set_velocity(DVec3::ZERO);
        if let Some(leash_data) = self.leash_data().lock().as_mut() {
            leash_data.angular_momentum = 0.0;
        }
    }

    /// Vanilla `HappyGhast.HappyGhastLookControl.tick`.
    fn tick_happy_ghast_look_control(&self) {
        if self.is_on_still_timeout() {
            let (yaw, _) = self.rotation();
            let close_angle = wrap_degrees90(yaw);
            let new_yaw = yaw - close_angle;
            self.set_rotation((new_yaw, self.rotation().1));
            self.set_y_head_rot(new_yaw);
            return;
        }

        let (looking, wanted_position) = {
            let mut controls = self.mob_base().controls().lock();
            let looking = controls.look_control.tick_cooldown();
            (looking, controls.look_control.wanted_position())
        };

        if looking {
            let position = self.position();
            let xd = wanted_position.x - position.x;
            let zd = wanted_position.z - position.z;
            let yaw = -((xd as f32).atan2(zd as f32)).to_degrees();
            self.set_rotation((yaw, self.rotation().1));
            self.set_y_body_rot(yaw);
            self.set_y_head_rot(yaw);
        } else {
            self.face_movement_direction();
        }
    }

    /// Vanilla `LookControl.tick` (the plain control babies keep).
    fn tick_plain_look_control(&self) {
        let look_control = {
            let mut controls = self.mob_base().controls().lock();
            let look_control = controls.look_control;
            controls.look_control.tick_cooldown();
            look_control
        };

        let mut rotation = self.rotation();
        rotation.1 = 0.0;
        if look_control.is_looking_at_target() {
            let position = self.position();
            let wanted_position = look_control.wanted_position();
            let xd = wanted_position.x - position.x;
            let yd = wanted_position.y - self.get_eye_y();
            let zd = wanted_position.z - position.z;
            let horizontal = xd.hypot(zd);
            if horizontal.abs() > 1.0e-5 || yd.abs() > 1.0e-5 {
                let target_pitch = -(yd.atan2(horizontal)) as f32 * RAD_TO_DEG;
                rotation.1 =
                    rotate_towards(rotation.1, target_pitch, look_control.x_max_rot_angle());
            }
            if zd.abs() > 1.0e-5 || xd.abs() > 1.0e-5 {
                let target_yaw = zd.atan2(xd) as f32 * RAD_TO_DEG - DEGREE_90;
                self.set_y_head_rot(rotate_towards(
                    self.y_head_rot(),
                    target_yaw,
                    look_control.y_max_rot_speed(),
                ));
            }
        } else {
            self.set_y_head_rot(rotate_towards(self.y_head_rot(), self.y_body_rot(), 10.0));
        }

        self.set_rotation(rotation);
        self.clamp_head_rotation_to_body_when_pathing();
    }

    /// Vanilla `Ghast.GhastMoveControl.tick`.
    fn tick_ghast_move_control(&self) {
        if self.is_on_still_timeout() {
            // Vanilla: `operation = WAIT; mob.stopInPlace();`.
            self.mob_base().controls().lock().move_control.set_wait();
            self.stop_in_place();
            return;
        }

        let (operation, wanted_position) = {
            let controls = self.mob_base().controls().lock();
            (
                controls.move_control.operation(),
                controls.move_control.wanted_position(),
            )
        };
        if operation != MoveControlOperation::MoveTo {
            return;
        }

        let mut float_duration = self.state.lock().float_duration;
        float_duration -= 1;
        if float_duration > 0 {
            self.state.lock().float_duration = float_duration;
            return;
        }
        let next_duration = float_duration + rand::random_range(0..MOVE_FLOAT_DURATION_RANGE)
            + MOVE_FLOAT_DURATION_BASE;
        self.state.lock().float_duration = next_duration;

        let travel = wanted_position - self.position();
        let Some(direction) = travel.try_normalize() else {
            self.mob_base().controls().lock().move_control.set_wait();
            return;
        };
        if travel.length_squared() < MOVE_REACH_DISTANCE_SQR {
            self.mob_base().controls().lock().move_control.set_wait();
            return;
        }

        let flying_speed = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::FLYING_SPEED);
        self.set_velocity(
            self.velocity() + direction * (flying_speed * f64::from(FLYING_SPEED_MULTIPLIER)),
        );
    }

    /// Vanilla `FlyingMoveControl<>(this, 180, true)` (the baby control).
    fn tick_baby_flying_move_control(&self) {
        let move_control = {
            let mut controls = self.mob_base().controls().lock();
            let move_control = controls.move_control;
            if matches!(move_control.operation(), MoveControlOperation::MoveTo) {
                controls.move_control.set_wait();
            }
            move_control
        };

        let (_, current_pitch) = self.rotation();
        let (movement_speed, flying_speed) = {
            let attributes = self.attributes().lock();
            (
                attributes.required_value(vanilla_attributes::MOVEMENT_SPEED),
                attributes.required_value(vanilla_attributes::FLYING_SPEED),
            )
        };
        let output = BABY_FLYING_MOVE_CONTROL.tick(FlyingMoveControlInput {
            operation: move_control.operation(),
            wanted_position: move_control.wanted_position(),
            position: self.position(),
            y_rot: self.rotation().0,
            x_rot: current_pitch,
            on_ground: self.on_ground(),
            speed_modifier: move_control.speed_modifier(),
            movement_speed,
            flying_speed,
        });

        self.set_rotation((output.y_rot, output.x_rot));
        self.set_mob_speed(output.speed);
        self.set_no_gravity(output.no_gravity);
        let input = self.travel_input();
        self.set_travel_input(LivingTravelInput::new(
            input.sideways(),
            output.yya,
            output.zza,
        ));
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
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }

    /// Vanilla `HappyGhast.BABY_DIMENSIONS`.
    fn baby_dimensions() -> EntityDimensions {
        let scaled = vanilla_entities::HAPPY_GHAST.dimensions.scale(BABY_SCALE);
        // Vanilla `withEyeHeight(0.46875F)` keeps the scaled width/height and
        // replaces the eye height.
        EntityDimensions::new_with_attachments(
            scaled.width,
            scaled.height,
            BABY_EYE_HEIGHT,
            scaled.attachments,
        )
    }
}

impl Entity for HappyGhastEntity {
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
        // Vanilla `HappyGhast.tick` server-side additions.
        self.tick_happy_ghast_server_state();
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            return Self::baby_dimensions().scale(scale);
        }
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

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    /// Vanilla `HappyGhast.playStepSound` is empty.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    /// Vanilla `HappyGhast.onClimbable` is `false`.
    fn on_climbable(&self) -> bool {
        false
    }

    /// Vanilla `HappyGhast.checkFallDamage` is empty.
    fn check_fall_damage(
        self: Arc<Self>,
        _vertical_movement: f64,
        _on_ground: bool,
        _on_state: BlockStateId,
        _pos: BlockPos,
        _world: &Arc<World>,
    ) {
    }

    /// Vanilla `HappyGhast.canBeCollidedWith`.
    fn can_be_collided_with(&self, other: Option<&dyn Entity>) -> bool {
        if AgeableMob::is_baby(self) || !LivingEntity::is_alive(self) {
            return false;
        }

        // VANILLA CLIENT-ONLY: the `isClientSide() && other instanceof Player &&
        // other.position().y >= maxY` branch is client rendering.
        let ghast_on_ghast = self.is_vehicle()
            && other.is_some_and(|other| other.entity_type() == &vanilla_entities::HAPPY_GHAST);
        ghast_on_ghast || self.is_on_still_timeout()
    }

    /// Vanilla `HappyGhast.isFlyingVehicle`.
    fn is_flying_vehicle(&self) -> bool {
        !AgeableMob::is_baby(self)
    }

    /// Vanilla `HappyGhast.canAddPassenger`: at most four direct passengers.
    fn can_add_passenger(&self, _passenger: &dyn Entity) -> bool {
        self.passengers().len() < MAX_PASSENGERS
    }

    /// Vanilla `HappyGhast.getControllingPassenger`.
    fn controlling_passenger(&self) -> Option<SharedEntity> {
        if self.is_wearing_body_armor()
            && !self.is_on_still_timeout()
            && let Some(passenger) = self.first_passenger()
            && passenger.as_player().is_some()
        {
            return Some(passenger);
        }

        self.controlling_passenger_mob()
    }

    /// Vanilla `HappyGhast.notifyLeashHolder`.
    fn notify_leash_holder(&self, leashable: &dyn Entity) {
        // GAP(happy-ghast-quad-leash-support): vanilla tests
        // `entity.supportQuadLeash()`; Steel has no such hook, so a leashed happy
        // ghast (the only quad-leash entity in Steel) is the quad case.
        if leashable.entity_type() == &vanilla_entities::HAPPY_GHAST {
            self.state.lock().leash_holder_time = LEASH_HOLDER_TIME;
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("still_timeout", self.server_still_timeout());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_server_still_timeout(nbt.int("still_timeout").unwrap_or(0));
    }
}

impl LivingEntity for HappyGhastEntity {
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

    /// Vanilla `HappyGhast.getSoundVolume`.
    fn sound_volume(&self) -> f32 {
        if AgeableMob::is_baby(self) { 1.0 } else { 4.0 }
    }

    /// Vanilla `HappyGhast.getHurtSound`.
    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_GHASTLING_HURT
        } else {
            &sound_events::ENTITY_HAPPY_GHAST_HURT
        })
    }

    /// Vanilla `HappyGhast.getDeathSound`.
    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_GHASTLING_DEATH
        } else {
            &sound_events::ENTITY_HAPPY_GHAST_DEATH
        })
    }

    /// Vanilla `HappyGhast.getVoicePitch`.
    fn voice_pitch(&self) -> f32 {
        1.0
    }

    /// Vanilla `HappyGhast.getAgeScale`.
    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            BABY_SCALE
        } else {
            1.0
        }
    }

    /// Vanilla `HappyGhast.sanitizeScale`: the scale is capped at 1.0.
    fn get_scale(&self) -> f32 {
        let scale = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::SCALE)
            .unwrap_or(1.0) as f32;
        scale.min(1.0)
    }

    /// Vanilla `HappyGhast.canBreatheUnderwater`: babies always can; adults fall
    /// back to `LivingEntity.canBreatheUnderwater()`, which is `false`.
    fn can_breathe_underwater(&self) -> bool {
        AgeableMob::is_baby(self)
    }

    /// Vanilla `HappyGhast.canUseSlot`: the body slot needs a live adult, every
    /// other slot keeps `LivingEntity.canUseSlot`'s `true`.
    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        if slot == EquipmentSlot::Body {
            return LivingEntity::is_alive(self) && !AgeableMob::is_baby(self);
        }
        true
    }

    /// Vanilla `HappyGhast.canDispenserEquipIntoSlot`: only the body slot.
    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        slot == EquipmentSlot::Body
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        // GAP(happy-ghast-precise-position): vanilla sets the precise-position
        // requirement from `isOnStillTimeout()` here.
        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        self.continuous_heal();
        result
    }

    /// Vanilla `HappyGhast.travel` -> `travelFlying(input, speed, speed, speed)`.
    fn travel(&self, entity: &SharedEntity, input: DVec3) -> Option<MoveResult> {
        let speed = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::FLYING_SPEED) as f32
            * FLYING_SPEED_MULTIPLIER;

        if self.is_in_water() {
            self.move_relative(FLYING_WATER_SPEED, input);
            let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
            self.set_velocity(self.velocity() * FLYING_WATER_DRAG);
            return Some(result);
        }

        if self.is_in_lava() {
            self.move_relative(FLYING_LAVA_SPEED, input);
            let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
            self.set_velocity(self.velocity() * FLYING_LAVA_DRAG);
            return Some(result);
        }

        self.move_relative(speed, input);
        let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
        self.set_velocity(self.velocity() * FLYING_AIR_DRAG);
        Some(result)
    }

    /// Vanilla `HappyGhast.getRiddenInput`.
    fn ridden_input(&self, controller: &Player, _self_input: DVec3) -> DVec3 {
        let travel_input = LivingEntity::travel_input(controller);
        let strafe = travel_input.sideways();
        let forward_input = travel_input.forward();
        let mut forward = 0.0_f32;
        let mut up = 0.0_f32;

        if forward_input != 0.0 {
            let (_, pitch) = controller.rotation();
            let pitch_radians = f64::from(pitch * steel_math::DEG_TO_RAD);
            let mut forward_look = trig::cos(pitch_radians);
            let mut up_look = -trig::sin(pitch_radians);
            if forward_input < 0.0 {
                forward_look *= -0.5;
                up_look *= -0.5;
            }
            up = up_look;
            forward = forward_look;
        }

        if LivingEntity::is_jumping(controller) {
            up += 0.5;
        }

        let flying_speed = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::FLYING_SPEED) as f32;
        DVec3::new(f64::from(strafe), f64::from(up), f64::from(forward))
            * f64::from(RIDDEN_INPUT_SCALE * flying_speed)
    }

    /// Vanilla `HappyGhast.tickRidden`.
    fn tick_ridden(&self, controller: &Player, _ridden_input: DVec3) {
        let (controller_yaw, controller_pitch) = controller.rotation();
        let (current_yaw, _) = self.rotation();
        let target_yaw = controller_yaw;
        let yaw = current_yaw + wrap_degrees(target_yaw - current_yaw) * RIDDEN_TURN_FACTOR;
        self.set_rotation((yaw, controller_pitch * RIDDEN_PITCH_FACTOR));
        self.base.set_old_yaw_to_current();
        self.set_y_body_rot(yaw);
        self.set_y_head_rot(yaw);
    }
}

impl AgeableMob for HappyGhastEntity {
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

    /// Vanilla `HappyGhast.ageBoundaryReached`: swap the controls, navigation, goals
    /// and brain between the baby and adult setups.
    fn age_boundary_changed(&self, baby: bool) {
        if baby {
            self.baby_ghast_setup();
        } else {
            self.adult_ghast_setup();
        }
        self.refresh_dimensions();
    }

    /// Vanilla `HappyGhast.getBreedOffspring`.
    fn get_breed_offspring(
        &self,
        world: &Arc<World>,
        _partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        // Vanilla uses `EntityTypes.HAPPY_GHAST.create(level, BREEDING)`; the
        // breeding path applies the spawn reason.
        ENTITIES.create(
            &vanilla_entities::HAPPY_GHAST,
            next_entity_id(),
            self.position(),
            Arc::downgrade(world),
        )
    }
}

impl Animal for HappyGhastEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `HappyGhast.isFood` uses `ItemTags.HAPPY_GHAST_FOOD`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::HAPPY_GHAST_FOOD)
    }

    /// Vanilla `HappyGhast.canFallInLove` is always `false`.
    fn can_fall_in_love(&self) -> bool {
        false
    }
}

impl Mob for HappyGhastEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    /// Vanilla `HappyGhast.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        1
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `HappyGhast.customServerAiStep` brain half: only babies tick their
    /// brain, and `HappyGhastAi.updateActivity` runs right after it.
    fn brain_tick(&self, entity: &SharedEntity) {
        if !AgeableMob::is_baby(self) {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
    }

    /// Vanilla `HappyGhast.customServerAiStep` tail: `checkRestriction`, then the
    /// `Animal` love-state reset.
    fn custom_server_ai_step(&self) {
        self.check_restriction();
        Animal::custom_server_ai_step_animal(self);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `HappyGhast` swaps `GhastMoveControl` for `FlyingMoveControl` on babies.
    fn tick_move_control(&self) {
        if AgeableMob::is_baby(self) {
            self.tick_baby_flying_move_control();
        } else {
            self.tick_ghast_move_control();
        }
    }

    /// Vanilla `HappyGhast` swaps `HappyGhastLookControl` for a plain `LookControl`.
    fn tick_look_control(&self) {
        if AgeableMob::is_baby(self) {
            self.tick_plain_look_control();
        } else {
            self.tick_happy_ghast_look_control();
        }
    }

    /// Vanilla `HappyGhast.getAmbientSound`.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_GHASTLING_AMBIENT
        } else {
            &sound_events::ENTITY_HAPPY_GHAST_AMBIENT
        })
    }

    /// Vanilla `HappyGhast.getAmbientSoundInterval`: six times slower while ridden.
    ///
    /// `Mob.getAmbientSoundInterval()` is 80 in vanilla; the shared `Mob` default
    /// cannot be called from its own override, so the constant is inlined.
    fn ambient_sound_interval(&self) -> i32 {
        if self.is_vehicle() {
            MOB_AMBIENT_SOUND_INTERVAL * 6
        } else {
            MOB_AMBIENT_SOUND_INTERVAL
        }
    }

    /// Vanilla `HappyGhast.mobInteract`.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        if AgeableMob::is_baby(self) {
            return Animal::mob_interact_animal(self, player, hand);
        }

        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };
        if !item_stack.is_empty() {
            // Vanilla `ItemStack.interactLivingEntity`; Steel's item behavior hook
            // needs a mutable stack, so the shared equippable path is used (the
            // harness is the only body-slot item this mob accepts).
            let interaction_result =
                LivingEntity::interact_living_entity_with_equippable(self, player, hand);
            if interaction_result.consumes_action() {
                return interaction_result;
            }
        }

        if self.is_wearing_body_armor() && !player.is_secondary_use_active() {
            self.do_player_ride(player);
            return InteractionResult::Success;
        }

        Animal::mob_interact_animal(self, player, hand)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `AgeableMob.finalizeSpawn`: a roll may turn the spawn into a baby.
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for HappyGhastEntity {
    /// Vanilla `HappyGhast.shouldStayCloseToLeashHolder` is `false`.
    fn should_stay_close_to_leash_holder(&self) -> bool {
        false
    }

    /// Vanilla `HappyGhast.getWalkTargetValue`: empty blocks above ground are best.
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        let Some(world) = self.level() else {
            return 0.0;
        };
        if !world.get_block_state(pos).is_air() {
            return 0.0;
        }
        if world.get_block_state(pos.below()).is_air()
            && !world.get_block_state(pos.below().below()).is_air()
        {
            10.0
        } else {
            5.0
        }
    }
}

/// Vanilla `Mth.wrapDegrees90`.
fn wrap_degrees90(mut angle: f32) -> f32 {
    angle %= DEGREE_360;
    if angle >= DEGREE_90 {
        angle -= DEGREE_180;
    }
    if angle < -DEGREE_90 {
        angle += DEGREE_180;
    }
    angle
}

/// Vanilla `Mth.rotateTowards`.
fn rotate_towards(from_angle: f32, to_angle: f32, max_rot: f32) -> f32 {
    let diff = wrap_degrees(to_angle - from_angle).clamp(-max_rot, max_rot);
    wrap_degrees(from_angle + diff)
}

/// Vanilla `HappyGhast.HappyGhastFloatGoal`: `FloatGoal` that refuses to start
/// while the ghast must stay still.
struct HappyGhastFloatGoal {
    inner: FloatGoal,
}

impl HappyGhastFloatGoal {
    fn new(mob_base: &MobBase) -> Self {
        Self {
            inner: FloatGoal::new(mob_base),
        }
    }
}

impl Goal for HappyGhastFloatGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !HappyGhastEntity::still_timeout_of(mob) && self.inner.can_use(mob)
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `TemptGoal.ForNonPathfinders` for the happy ghast.
///
/// The item predicate reads the ghast's age and body armour, so it cannot be a
/// `'static` closure like the shared [`crate::entity::ai::goal::TemptGoal`]; the
/// navigation half is vanilla's `ForNonPathfinders` override (move-control wanted
/// positions instead of path navigation).
struct HappyGhastTemptGoal {
    player: Option<Arc<Player>>,
    player_position: DVec3,
    player_yaw: f32,
    player_pitch: f32,
    speed_modifier: f64,
    calm_down: i32,
    is_running: bool,
    stop_distance: f64,
}

impl HappyGhastTemptGoal {
    const fn new(speed_modifier: f64, stop_distance: f64) -> Self {
        Self {
            player: None,
            player_position: DVec3::ZERO,
            player_yaw: 0.0,
            player_pitch: 0.0,
            speed_modifier,
            calm_down: 0,
            is_running: false,
            stop_distance,
        }
    }

    /// Vanilla `HappyGhast.registerGoals` item predicate.
    fn is_tempted_item(ghast: &HappyGhastEntity, item_stack: &ItemStack) -> bool {
        if !ghast.is_wearing_body_armor() && !AgeableMob::is_baby(ghast) {
            REGISTRY
                .items
                .is_in_tag(item_stack.item(), &ItemTag::HAPPY_GHAST_TEMPT_ITEMS)
        } else {
            REGISTRY
                .items
                .is_in_tag(item_stack.item(), &ItemTag::HAPPY_GHAST_FOOD)
        }
    }

    fn ghast(mob: &dyn PathfinderMob) -> Option<&HappyGhastEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<HappyGhastEntity>()
    }

    fn should_follow(ghast: &HappyGhastEntity, player: &dyn LivingEntity) -> bool {
        player.is_holding(&mut |item_stack| Self::is_tempted_item(ghast, item_stack))
    }

    /// Vanilla `TemptGoal.targetingConditions`.
    fn targeting_conditions(range: f64) -> TargetingConditions {
        TargetingConditions::for_non_combat()
            .ignore_line_of_sight()
            .range(range)
    }

    /// Vanilla `TemptGoal.canScare` for `ForNonPathfinders` (constructed with
    /// `canScare = false`).
    const fn can_scare(&self) -> bool {
        false
    }

    /// Vanilla `TemptGoal.canContinueToUse`'s `canScare()` block.
    fn update_player_scare_state(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(player) = &self.player else {
            return false;
        };

        if mob.position().distance_squared(player.position()) < TEMPT_SCARE_DISTANCE_SQR {
            if player.position().distance_squared(self.player_position) > TEMPT_SCARE_MOVE_SQR {
                return false;
            }

            let (yaw, pitch) = player.rotation();
            if (pitch - self.player_pitch).abs() > TEMPT_SCARE_ROTATION
                || (yaw - self.player_yaw).abs() > TEMPT_SCARE_ROTATION
            {
                return false;
            }
        } else {
            self.player_position = player.position();
        }

        let (yaw, pitch) = player.rotation();
        self.player_yaw = yaw;
        self.player_pitch = pitch;
        true
    }
}

impl Goal for HappyGhastTemptGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.calm_down > 0 {
            self.calm_down -= 1;
            return false;
        }

        let Some(ghast) = Self::ghast(mob) else {
            return false;
        };
        let Some(world) = mob.level() else {
            return false;
        };
        let range = mob
            .attributes()
            .lock()
            .required_value(vanilla_attributes::TEMPT_RANGE);
        let targeting_conditions = Self::targeting_conditions(range);
        self.player = world.nearest_player(mob.position(), range, |player| {
            targeting_conditions.test(world.as_ref(), Some(mob), player)
                && Self::should_follow(ghast, player)
        });
        self.player.is_some()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.can_scare() && !self.update_player_scare_state(mob) {
            return false;
        }

        self.can_use(mob)
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        if let Some(player) = &self.player {
            self.player_position = player.position();
        }
        self.is_running = true;
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.player = None;
        self.calm_down = TEMPT_CALM_DOWN_TICKS;
        self.is_running = false;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(player) = &self.player else {
            return;
        };

        let player_position = player.position();
        let eye_position = DVec3::new(player_position.x, player.get_eye_y(), player_position.z);
        mob.mob_base().controls().lock().look_control.set_look_at(
            eye_position,
            mob.max_head_y_rot() + 20.0,
            mob.max_head_x_rot(),
        );

        if mob.position().distance_squared(player_position)
            < self.stop_distance * self.stop_distance
        {
            // Vanilla `ForNonPathfinders.stopNavigation`.
            mob.mob_base().controls().lock().move_control.set_wait();
            return;
        }

        // Vanilla `ForNonPathfinders.navigateTowards`.
        let target = (eye_position - mob.position())
            .scale(rand::random::<f64>())
            .add(mob.position());
        mob.mob_base()
            .controls()
            .lock()
            .move_control
            .set_wanted_position(target, self.speed_modifier);
    }
}

/// Vanilla `Ghast.RandomFloatAroundGoal(this, 16)`.
struct HappyGhastRandomFloatGoal {
    distance_to_blocks: i32,
}

impl HappyGhastRandomFloatGoal {
    const fn new(distance_to_blocks: i32) -> Self {
        Self { distance_to_blocks }
    }

    /// Vanilla `Ghast.RandomFloatAroundGoal.getSuitableFlyToPosition`.
    fn suitable_fly_to_position(&self, mob: &dyn PathfinderMob) -> DVec3 {
        let center = mob.position();
        let Some(world) = mob.level() else {
            return center;
        };

        let mut result: Option<DVec3> = None;
        for _ in 0..FLOAT_ATTEMPTS {
            let candidate = choose_random_position_with_restriction(mob, center);
            let Some(candidate) = candidate else {
                continue;
            };
            result = Some(candidate);
            if is_good_target(world.as_ref(), candidate, self.distance_to_blocks) {
                return candidate;
            }
        }

        let mut result = result.unwrap_or_else(|| choose_random_position(center));
        let pos = BlockPos::containing(result.x, result.y, result.z);
        let height = world.heightmap_pos(HeightmapType::MotionBlocking, pos).y();
        if height < pos.y() && height > world.get_min_y() {
            result.y = mob.position().y - (mob.position().y - result.y).abs();
        }
        result
    }
}

impl Goal for HappyGhastRandomFloatGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let (operation, wanted_position) = {
            let controls = mob.mob_base().controls().lock();
            (
                controls.move_control.operation(),
                controls.move_control.wanted_position(),
            )
        };
        if operation != MoveControlOperation::MoveTo {
            return true;
        }

        let distance_sqr = mob.position().distance_squared(wanted_position);
        distance_sqr < FLOAT_MIN_DISTANCE_SQR || distance_sqr > FLOAT_MAX_DISTANCE_SQR
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let target = self.suitable_fly_to_position(mob);
        mob.mob_base()
            .controls()
            .lock()
            .move_control
            .set_wanted_position(target, 1.0);
    }
}

/// Vanilla `Ghast.RandomFloatAroundGoal.chooseRandomPosition`.
fn choose_random_position(center: DVec3) -> DVec3 {
    center
        + DVec3::new(
            (f64::from(rand::random::<f32>()) * 2.0 - 1.0) * FLOAT_SPREAD,
            (f64::from(rand::random::<f32>()) * 2.0 - 1.0) * FLOAT_SPREAD,
            (f64::from(rand::random::<f32>()) * 2.0 - 1.0) * FLOAT_SPREAD,
        )
}

/// Vanilla `Ghast.RandomFloatAroundGoal.chooseRandomPositionWithRestriction`.
fn choose_random_position_with_restriction(
    mob: &dyn PathfinderMob,
    center: DVec3,
) -> Option<DVec3> {
    let target = choose_random_position(center);
    if mob.has_home() && !mob.is_within_home_vec(target) {
        return None;
    }
    Some(target)
}

/// Vanilla `Ghast.RandomFloatAroundGoal.isGoodTarget`.
fn is_good_target(world: &World, target: DVec3, distance_to_blocks: i32) -> bool {
    if distance_to_blocks <= 0 {
        return true;
    }

    let pos = BlockPos::containing(target.x, target.y, target.z);
    if !world.get_block_state(pos).is_air() {
        return false;
    }

    // Vanilla iterates `Direction.values()`; the probe is order-independent.
    for (dx, dy, dz) in BLOCK_DIRECTIONS {
        for distance in 1..distance_to_blocks {
            let offset = BlockPos::new(
                pos.x() + dx * distance,
                pos.y() + dy * distance,
                pos.z() + dz * distance,
            );
            if !world.get_block_state(offset).is_air() {
                return true;
            }
        }
    }

    false
}
