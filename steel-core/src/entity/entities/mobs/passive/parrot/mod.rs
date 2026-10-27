//! Vanilla `Parrot` (`world/entity/animal/parrot/Parrot.java`) and its
//! `ShoulderRidingEntity`/`TamableAnimal` bases: 5 synced variants, tameable
//! sit/owner state, mob-sound imitation, cookie poisoning and party-parrot
//! dancing.
//!
//! Steel has no `ShoulderRidingEntity`/`TamableAnimal` classes; their state is
//! expressed through the shared `Tameable`/`ShoulderRiding` capability traits,
//! which the tameable goals (`SitWhenOrderedToGoal`, `FollowOwnerGoal`) read.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_math::fast_floor;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::PARROT_VARIANT;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::entity_variant::ParrotVariant;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::ParrotEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_block_tags,
    vanilla_blocks, vanilla_damage_types, vanilla_entities, vanilla_mob_effects,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::{Difficulty, InteractionHand};
use steel_utils::{BlockPos, BlockStateId, ChunkPos, DowncastType, DowncastTypeKey, UuidExt as _};
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::ai::control::{FlyingMoveControl, tick_flying_move_control};
use crate::entity::ai::goal::{
    FloatGoal, FollowMobGoal, FollowOwnerGoal, Goal, GoalControls, LookAtPlayerGoal, PanicGoal,
    ShoulderRiding, SitWhenOrderedToGoal, Tameable,
};
use crate::entity::ai::navigation::NavigationKind;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::leash::Leashable as _;
use crate::entity::{
    AgeableMob, AgeableMobBase, AgeableMobGroupData, Animal, AnimalBase, Entity, EntityBase,
    EntityBaseLoad, EntityEventSource, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, PathfinderMob, RemovalReason,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::player::advancement::triggers::raise_tame_animal;
use crate::world::{LevelReader, World};

/// Vanilla `ShoulderRidingEntity.RIDE_COOLDOWN`.
const RIDE_COOLDOWN: i32 = 100;
/// Vanilla `TamableAnimal` synced flag bits.
const TAMABLE_SITTING_FLAG: i8 = 1;
const TAMABLE_TAME_FLAG: i8 = 4;
/// Vanilla `TamableAnimal.TELEPORT_WHEN_DISTANCE_IS_SQ`.
const TELEPORT_WHEN_DISTANCE_SQ: f64 = 144.0;
/// Vanilla `Parrot.aiStep` imitation chance divisor.
const IMITATE_CHANCE_DIVISOR: i32 = 400;
/// Vanilla `Parrot.imitateNearbyMobs` search inflation.
const IMITATE_NEARBY_RANGE: f64 = 20.0;
const IMITATE_SOUND_VOLUME: f32 = 0.7;
const IMITATE_ROLL_SUCCESS_CHANCE: i32 = 2;
/// Vanilla `Parrot.getAmbient` imitation chance divisor.
const AMBIENT_IMITATE_CHANCE_DIVISOR: i32 = 1000;
/// Pitch jitter used by `Parrot.getPitch`.
const VOICE_PITCH_JITTER: f32 = 0.2;
/// Vanilla `Parrot.aiStep` jukebox distance gate.
const PARTY_PARROT_RANGE: f64 = 3.46;
/// Vanilla `Parrot.getSoundVolume` and step-sound parameters.
const PARROT_SOUND_VOLUME: f32 = 1.0;
const STEP_SOUND_VOLUME: f32 = 0.15;
const STEP_SOUND_PITCH: f32 = 1.0;
/// Vanilla `Parrot.mobInteract` taming roll.
const TAMING_SUCCESS_CHANCE: i32 = 10;
/// Vanilla `MobEffects.POISON` duration applied by a poisonous parrot food.
const POISON_DURATION: i32 = 900;
/// Vanilla `Parrot` flap animation tuning (`calculateFlapping`).
const FLAP_AIRBORNE_GAIN: f32 = 4.0;
const FLAP_GROUNDED_LOSS: f32 = -1.0;
const FLAP_SPEED_SCALE: f32 = 0.3;
const MIN_FLAPPING_STRENGTH: f32 = 1.0;
const FLAPPING_STRENGTH_DECAY: f32 = 0.9;
const FLAP_ROTATION_SCALE: f32 = 2.0;
const FLAP_FALL_DRAG_Y: f64 = 0.6;
/// Divisor turning flap speed into the next flap distance threshold.
const NEXT_FLAP_SPEED_DIVISOR: f32 = 2.0;
/// Vanilla `FlyingMoveControl(this, 10, false)` (`Parrot.java:138`).
const FLYING_MOVE_CONTROL: FlyingMoveControl = FlyingMoveControl::new(10, false);
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `RandomStrollGoal` defaults used by `Parrot.ParrotWanderGoal`.
const WANDER_INTERVAL: i32 = 120;
const WANDER_PROBABILITY: f32 = 0.001;
const NO_ACTION_TIME_LIMIT: i32 = 100;
/// Vanilla `Parrot.ParrotWanderGoal.getTreePos` search bounds.
const TREE_SEARCH_HORIZONTAL: i32 = 3;
const TREE_SEARCH_VERTICAL: i32 = 6;
/// Vanilla `LandRandomPos.getPos(this.mob, 15, 15)` water fallback bounds.
const WATER_LAND_HORIZONTAL: f64 = 15.0;
const WATER_LAND_VERTICAL: f64 = 15.0;
/// Vanilla `AirAndWaterRandomPos` horizontal bound.
const FLYING_RANDOM_HORIZONTAL: i32 = 8;
/// Aerial stroll targets stay this far above the parrot's hover height.
const AERIAL_MIN_HEIGHT: f64 = 1.0;
const AERIAL_MAX_HEIGHT: f64 = 3.0;
/// Distance at which a steered stroll goal considers its target reached.
const ARRIVAL_DISTANCE_SQ: f64 = 1.0;
/// Vanilla `Parrot` goal priorities and modifiers.
const PANIC_SPEED: f64 = 1.25;
const LOOK_AT_PLAYER_DISTANCE: f64 = 8.0;
const FOLLOW_OWNER_SPEED: f64 = 1.0;
const FOLLOW_OWNER_START_DISTANCE: f32 = 5.0;
const FOLLOW_OWNER_STOP_DISTANCE: f32 = 1.0;
const WANDER_SPEED: f64 = 1.0;
const FOLLOW_MOB_SPEED: f64 = 1.0;
const FOLLOW_MOB_STOP_DISTANCE: f32 = 3.0;
const FOLLOW_MOB_AREA: f32 = 7.0;

/// Vanilla `Parrot.MOB_SOUND_MAP`: entity types a parrot may imitate.
static IMITATED_SOUNDS: [(EntityTypeRef, SoundEventRef); 42] = [
    (
        &vanilla_entities::BLAZE,
        &sound_events::ENTITY_PARROT_IMITATE_BLAZE,
    ),
    (
        &vanilla_entities::BOGGED,
        &sound_events::ENTITY_PARROT_IMITATE_BOGGED,
    ),
    (
        &vanilla_entities::BREEZE,
        &sound_events::ENTITY_PARROT_IMITATE_BREEZE,
    ),
    (
        &vanilla_entities::CAMEL_HUSK,
        &sound_events::ENTITY_PARROT_IMITATE_CAMEL_HUSK,
    ),
    (
        &vanilla_entities::CAVE_SPIDER,
        &sound_events::ENTITY_PARROT_IMITATE_SPIDER,
    ),
    (
        &vanilla_entities::CREAKING,
        &sound_events::ENTITY_PARROT_IMITATE_CREAKING,
    ),
    (
        &vanilla_entities::CREEPER,
        &sound_events::ENTITY_PARROT_IMITATE_CREEPER,
    ),
    (
        &vanilla_entities::DROWNED,
        &sound_events::ENTITY_PARROT_IMITATE_DROWNED,
    ),
    (
        &vanilla_entities::ELDER_GUARDIAN,
        &sound_events::ENTITY_PARROT_IMITATE_ELDER_GUARDIAN,
    ),
    (
        &vanilla_entities::ENDER_DRAGON,
        &sound_events::ENTITY_PARROT_IMITATE_ENDER_DRAGON,
    ),
    (
        &vanilla_entities::ENDERMITE,
        &sound_events::ENTITY_PARROT_IMITATE_ENDERMITE,
    ),
    (
        &vanilla_entities::EVOKER,
        &sound_events::ENTITY_PARROT_IMITATE_EVOKER,
    ),
    (
        &vanilla_entities::GHAST,
        &sound_events::ENTITY_PARROT_IMITATE_GHAST,
    ),
    (
        &vanilla_entities::HAPPY_GHAST,
        &sound_events::INTENTIONALLY_EMPTY,
    ),
    (
        &vanilla_entities::GUARDIAN,
        &sound_events::ENTITY_PARROT_IMITATE_GUARDIAN,
    ),
    (
        &vanilla_entities::HOGLIN,
        &sound_events::ENTITY_PARROT_IMITATE_HOGLIN,
    ),
    (
        &vanilla_entities::HUSK,
        &sound_events::ENTITY_PARROT_IMITATE_HUSK,
    ),
    (
        &vanilla_entities::ILLUSIONER,
        &sound_events::ENTITY_PARROT_IMITATE_ILLUSIONER,
    ),
    (
        &vanilla_entities::MAGMA_CUBE,
        &sound_events::ENTITY_PARROT_IMITATE_MAGMA_CUBE,
    ),
    (
        &vanilla_entities::PARCHED,
        &sound_events::ENTITY_PARROT_IMITATE_PARCHED,
    ),
    (
        &vanilla_entities::PHANTOM,
        &sound_events::ENTITY_PARROT_IMITATE_PHANTOM,
    ),
    (
        &vanilla_entities::PIGLIN,
        &sound_events::ENTITY_PARROT_IMITATE_PIGLIN,
    ),
    (
        &vanilla_entities::PIGLIN_BRUTE,
        &sound_events::ENTITY_PARROT_IMITATE_PIGLIN_BRUTE,
    ),
    (
        &vanilla_entities::PILLAGER,
        &sound_events::ENTITY_PARROT_IMITATE_PILLAGER,
    ),
    (
        &vanilla_entities::RAVAGER,
        &sound_events::ENTITY_PARROT_IMITATE_RAVAGER,
    ),
    (
        &vanilla_entities::SHULKER,
        &sound_events::ENTITY_PARROT_IMITATE_SHULKER,
    ),
    (
        &vanilla_entities::SILVERFISH,
        &sound_events::ENTITY_PARROT_IMITATE_SILVERFISH,
    ),
    (
        &vanilla_entities::SKELETON,
        &sound_events::ENTITY_PARROT_IMITATE_SKELETON,
    ),
    (
        &vanilla_entities::SLIME,
        &sound_events::ENTITY_PARROT_IMITATE_SLIME,
    ),
    (
        &vanilla_entities::SPIDER,
        &sound_events::ENTITY_PARROT_IMITATE_SPIDER,
    ),
    (
        &vanilla_entities::STRAY,
        &sound_events::ENTITY_PARROT_IMITATE_STRAY,
    ),
    (
        &vanilla_entities::VEX,
        &sound_events::ENTITY_PARROT_IMITATE_VEX,
    ),
    (
        &vanilla_entities::VINDICATOR,
        &sound_events::ENTITY_PARROT_IMITATE_VINDICATOR,
    ),
    (
        &vanilla_entities::WARDEN,
        &sound_events::ENTITY_PARROT_IMITATE_WARDEN,
    ),
    (
        &vanilla_entities::WITCH,
        &sound_events::ENTITY_PARROT_IMITATE_WITCH,
    ),
    (
        &vanilla_entities::WITHER,
        &sound_events::ENTITY_PARROT_IMITATE_WITHER,
    ),
    (
        &vanilla_entities::WITHER_SKELETON,
        &sound_events::ENTITY_PARROT_IMITATE_WITHER_SKELETON,
    ),
    (
        &vanilla_entities::ZOGLIN,
        &sound_events::ENTITY_PARROT_IMITATE_ZOGLIN,
    ),
    (
        &vanilla_entities::ZOMBIE,
        &sound_events::ENTITY_PARROT_IMITATE_ZOMBIE,
    ),
    (
        &vanilla_entities::ZOMBIE_HORSE,
        &sound_events::ENTITY_PARROT_IMITATE_ZOMBIE_HORSE,
    ),
    (
        &vanilla_entities::ZOMBIE_NAUTILUS,
        &sound_events::ENTITY_PARROT_IMITATE_ZOMBIE_NAUTILUS,
    ),
    (
        &vanilla_entities::ZOMBIE_VILLAGER,
        &sound_events::ENTITY_PARROT_IMITATE_ZOMBIE_VILLAGER,
    ),
];

/// Returns the sound a parrot imitates for `entity_type` (`Parrot.getImitatedSound`).
fn imitated_sound(entity_type: EntityTypeRef) -> Option<SoundEventRef> {
    IMITATED_SOUNDS
        .iter()
        .find_map(|(candidate, sound)| (*candidate == entity_type).then_some(*sound))
}

/// Returns vanilla `Parrot.getPitch`: a randomized 1.0 +/- 0.1 voice pitch.
fn parrot_pitch() -> f32 {
    (rand::random::<f32>() - rand::random::<f32>()) * VOICE_PITCH_JITTER + 1.0
}

/// Vanilla `Mth.reducedTickDelay`.
const fn reduced_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
}

/// Vanilla `Parrot` runtime state that vanilla keeps in plain fields
/// (`orderedToSit`, `rideCooldownCounter`, `partyParrot`, jukebox and the flap
/// animation fields).
#[derive(Debug, Clone, Copy)]
struct ParrotState {
    ordered_to_sit: bool,
    ride_cooldown_counter: i32,
    party_parrot: bool,
    jukebox: Option<BlockPos>,
    flap: f32,
    flap_speed: f32,
    o_flap: f32,
    o_flap_speed: f32,
    flapping: f32,
    next_flap: f32,
}

impl ParrotState {
    const fn new() -> Self {
        Self {
            ordered_to_sit: false,
            ride_cooldown_counter: 0,
            party_parrot: false,
            jukebox: None,
            flap: 0.0,
            flap_speed: 0.0,
            o_flap: 0.0,
            o_flap_speed: 0.0,
            flapping: 1.0,
            next_flap: 1.0,
        }
    }
}

/// Vanilla `Parrot.ParrotWanderGoal.getTreePos`: the nearest perch under leaves
/// or logs.
fn tree_position(mob: &dyn PathfinderMob) -> Option<DVec3> {
    let world = mob.level()?;
    let origin = mob.block_position();
    let position = mob.position();
    let origin_x = fast_floor(position.x);
    let origin_y = fast_floor(position.y);
    let origin_z = fast_floor(position.z);

    for y in (origin_y - TREE_SEARCH_VERTICAL)..=(origin_y + TREE_SEARCH_VERTICAL) {
        for x in (origin_x - TREE_SEARCH_HORIZONTAL)..=(origin_x + TREE_SEARCH_HORIZONTAL) {
            for z in (origin_z - TREE_SEARCH_HORIZONTAL)..=(origin_z + TREE_SEARCH_HORIZONTAL) {
                let pos = BlockPos::new(x, y, z);
                if pos == origin {
                    continue;
                }

                let below = world.get_block_state(pos.below()).get_block();
                let can_sit_on = below.has_tag(&vanilla_block_tags::BlockTag::LEAVES)
                    || below.has_tag(&vanilla_block_tags::BlockTag::LOGS);
                if !can_sit_on {
                    continue;
                }

                if world.get_block_state(pos).is_air()
                    && world.get_block_state(pos.above()).is_air()
                {
                    return Some(DVec3::new(
                        f64::from(x) + 0.5,
                        f64::from(y),
                        f64::from(z) + 0.5,
                    ));
                }
            }
        }
    }

    None
}

/// Vanilla `WaterAvoidingRandomFlyingGoal.getPosition` (hover-biased).
///
/// GAP: vanilla uses `HoverRandomPos`/`AirAndWaterRandomPos`; Steel has no port
/// of those helpers, so this uses an equivalent directional hover pick.
fn aerial_position(mob: &dyn PathfinderMob) -> DVec3 {
    let position = mob.position();
    let (yaw, _) = mob.rotation();
    let yaw = f64::from(yaw).to_radians();
    let (sin, cos) = yaw.sin_cos();
    let direction_x = -sin;
    let direction_z = cos;

    let distance = rand::random_range(3.0..=f64::from(FLYING_RANDOM_HORIZONTAL));
    let height = rand::random_range(AERIAL_MIN_HEIGHT..=AERIAL_MAX_HEIGHT);
    DVec3::new(
        position.x + direction_x * distance,
        position.y + height,
        position.z + direction_z * distance,
    )
}

/// Vanilla `LandRandomPos.getPos(mob, 15, 15)` water fallback.
///
/// GAP: Steel's `land_random_pos` helper is module-private in `ai/goal`, so this
/// repeats the plain "standable block within range" search locally.
fn water_position(mob: &dyn PathfinderMob) -> Option<DVec3> {
    let world = mob.level()?;
    let position = mob.position();

    for _ in 0..10 {
        let dx = rand::random_range(-WATER_LAND_HORIZONTAL..=WATER_LAND_HORIZONTAL);
        let dy = rand::random_range(-WATER_LAND_VERTICAL..=WATER_LAND_VERTICAL);
        let dz = rand::random_range(-WATER_LAND_HORIZONTAL..=WATER_LAND_HORIZONTAL);
        let candidate = position + DVec3::new(dx, dy, dz);
        let pos = BlockPos::containing(candidate.x, candidate.y, candidate.z);
        if !world.has_full_chunk(ChunkPos::from_block_pos(pos)) {
            continue;
        }
        if !world.get_block_state(pos).is_air() || world.get_block_state(pos.below()).is_air() {
            continue;
        }
        if mob.get_walk_target_value(pos) < 0.0 {
            continue;
        }

        return Some(DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        ));
    }

    None
}

/// Vanilla `Parrot` entity with synced variant and tame/owner state.
#[entity_behavior(class = "Parrot")]
pub struct ParrotEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    parrot_state: SyncMutex<ParrotState>,
    entity_data: SyncMutex<ParrotEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ParrotEntity`.
unsafe impl DowncastType for ParrotEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/parrot");
}

impl ParrotEntity {
    /// Creates a new parrot at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a parrot from persisted base entity state.
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
        // Vanilla `Parrot.createNavigation` -> `FlyingPathNavigation` with door
        // opening disabled and floating enabled (`Parrot.java:183`).
        {
            let mut navigation = mob_base.navigation().lock();
            navigation.set_kind(NavigationKind::Fly);
            navigation.set_can_open_doors(false);
            navigation.set_can_float(true);
        }
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        {
            // Vanilla `Parrot` constructor path costs.
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::FireInNeighbor, -1.0);
            malus.set(PathType::Fire, -1.0);
            malus.set(PathType::Cocoa, -1.0);
        }

        let mut entity_data = ParrotEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Keep vanilla `Parrot.registerGoals` priorities and speeds in order.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, PanicGoal::new(PANIC_SPEED));
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, LookAtPlayerGoal::new(LOOK_AT_PLAYER_DISTANCE));
            goal_selector.add_goal(2, SitWhenOrderedToGoal::new());
            goal_selector.add_goal(
                2,
                FollowOwnerGoal::new(
                    FOLLOW_OWNER_SPEED,
                    FOLLOW_OWNER_START_DISTANCE,
                    FOLLOW_OWNER_STOP_DISTANCE,
                ),
            );
            goal_selector.add_goal(2, ParrotWanderGoal::new());
            goal_selector.add_goal(3, LandOnOwnersShoulderGoal::new());
            // Vanilla `FollowMobGoal` default predicate: never follow the same
            // class, i.e. never follow another parrot.
            goal_selector.add_goal(
                3,
                FollowMobGoal::new(
                    FOLLOW_MOB_SPEED,
                    FOLLOW_MOB_STOP_DISTANCE,
                    FOLLOW_MOB_AREA,
                    |_mob, candidate| candidate.entity_type() != &vanilla_entities::PARROT,
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
            parrot_state: SyncMutex::new(ParrotState::new()),
            entity_data: SyncMutex::new(entity_data),
        }
    }

    /// Sets the synced parrot variant.
    pub fn set_variant(&self, variant: ParrotVariant) {
        self.entity_data.lock().variant.set(variant.id());
    }

    /// Returns the synced parrot variant (`Parrot.getVariant`).
    #[must_use]
    pub fn variant(&self) -> ParrotVariant {
        let id = *self.entity_data.lock().variant.get();
        ParrotVariant::by_id(id)
    }

    /// Vanilla `TamableAnimal.getOwnerReference`: the owner UUID, when one is set.
    #[must_use]
    pub fn owner_uuid(&self) -> Option<Uuid> {
        *self.entity_data.lock().tamable_animal().owneruuid.get()
    }

    /// Vanilla `TamableAnimal.setOwner`.
    pub fn set_owner_uuid(&self, owner: Option<Uuid>) {
        self.entity_data
            .lock()
            .tamable_animal_mut()
            .owneruuid
            .set(owner);
    }

    /// Vanilla `TamableAnimal.getOwner`: the live player when it is in this world.
    #[must_use]
    pub fn owner(&self) -> Option<SharedEntity> {
        let uuid = self.owner_uuid()?;
        let world = self.level()?;
        world
            .players
            .get_by_uuid(&uuid)
            .map(|player| -> SharedEntity { player })
    }

    /// The live owner's position, when the owner is loaded.
    fn owner_position(&self) -> Option<DVec3> {
        Some(self.owner()?.position())
    }

    /// Vanilla `TamableAnimal.isTame`.
    #[must_use]
    pub fn is_tame(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & TAMABLE_TAME_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setTame`.
    pub fn set_tame(&self, tame: bool) {
        set_tamable_flag(&self.entity_data, TAMABLE_TAME_FLAG, tame);
    }

    /// Vanilla `TamableAnimal.isOrderedToSit`.
    #[must_use]
    pub fn is_ordered_to_sit(&self) -> bool {
        self.parrot_state.lock().ordered_to_sit
    }

    /// Vanilla `TamableAnimal.setOrderedToSit`.
    pub fn set_ordered_to_sit(&self, ordered_to_sit: bool) {
        self.parrot_state.lock().ordered_to_sit = ordered_to_sit;
    }

    /// Vanilla `TamableAnimal.isInSittingPose`.
    #[must_use]
    pub fn is_in_sitting_pose(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & TAMABLE_SITTING_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setInSittingPose`.
    pub fn set_in_sitting_pose(&self, sitting: bool) {
        set_tamable_flag(&self.entity_data, TAMABLE_SITTING_FLAG, sitting);
    }

    /// Vanilla `TamableAnimal.isOwnedBy`.
    #[must_use]
    pub fn is_owned_by(&self, player: &Player) -> bool {
        self.owner_uuid() == Some(player.uuid())
    }

    /// Vanilla `TamableAnimal.unableToMoveToOwner`.
    #[must_use]
    pub fn unable_to_move_to_owner(&self) -> bool {
        self.is_ordered_to_sit()
            || self.is_passenger()
            || self.may_be_leashed()
            || self
                .owner_uuid()
                .and_then(|uuid| self.level()?.players.get_by_uuid(&uuid))
                .is_some_and(|owner| owner.is_spectator())
    }

    /// Vanilla `TamableAnimal.shouldTryTeleportToOwner`.
    #[must_use]
    pub fn should_try_teleport_to_owner(&self) -> bool {
        self.owner_position().is_some_and(|owner_pos| {
            self.position().distance_squared(owner_pos) >= TELEPORT_WHEN_DISTANCE_SQ
        })
    }

    /// Vanilla `TamableAnimal.tryToTeleportToOwner`.
    pub fn try_to_teleport_to_owner(&self) {
        let Some(owner_pos) = self.owner_position() else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla `teleportToAroundBlockPos`: up to 10 tries at least two blocks away.
        for _ in 0..10 {
            let dx = rand::random_range(-3i32..=3);
            let dz = rand::random_range(-3i32..=3);
            if dx.abs() < 2 && dz.abs() < 2 {
                continue;
            }

            let dy = rand::random_range(-1i32..=1);
            if self.random_teleport(
                &world,
                owner_pos.x + f64::from(dx) + 0.5,
                owner_pos.y + f64::from(dy),
                owner_pos.z + f64::from(dz) + 0.5,
                false,
            ) {
                return;
            }
        }
    }

    /// Vanilla `TamableAnimal.tame`.
    ///
    /// Raises `TAME_ANIMAL` for the taming player, matching vanilla.
    pub fn tame(&self, player: &Player) {
        self.set_tame(true);
        self.set_owner_uuid(Some(player.uuid()));
        raise_tame_animal(player, self);
    }

    /// Vanilla `Parrot.isFlying`.
    #[must_use]
    pub fn is_flying(&self) -> bool {
        !self.on_ground()
    }

    /// Vanilla `Parrot.isPartyParrot`.
    #[must_use]
    pub fn is_party_parrot(&self) -> bool {
        self.parrot_state.lock().party_parrot
    }

    /// Vanilla `Parrot.setRecordPlayingNearby`, which the level calls when a
    /// jukebox starts or stops playing next to this parrot.
    ///
    /// GAP: Steel's `World`/`JukeboxBlockEntity` never call this hook yet, so
    /// dancing stays off until the level-side record notification lands.
    pub fn set_record_playing_nearby(&self, jukebox: BlockPos, is_playing: bool) {
        let mut state = self.parrot_state.lock();
        state.jukebox = Some(jukebox);
        state.party_parrot = is_playing;
    }

    /// Vanilla `ShoulderRidingEntity.canSitOnShoulder`.
    #[must_use]
    pub fn can_sit_on_shoulder(&self) -> bool {
        self.parrot_state.lock().ride_cooldown_counter > RIDE_COOLDOWN
    }

    /// Vanilla `ShoulderRidingEntity.setEntityOnShoulder`.
    ///
    /// Serializes this parrot as vanilla `Entity.saveWithoutId` + `id` and hands
    /// it to the owner; the parrot is discarded once the owner stores it.
    pub fn set_entity_on_shoulder(&self, owner: &SharedEntity) -> bool {
        let Some(player) = owner.as_player() else {
            return false;
        };
        let Some(tag) = self.nbt_for_passenger_save() else {
            return false;
        };
        if player.set_entity_on_shoulder(tag) {
            self.set_removed(RemovalReason::Discarded);
            true
        } else {
            false
        }
    }

    /// Copies resolved mob-effect display state into the synced entity data.
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

        // Sync base entity flags from resolved effect display state in one place.
        self.entity_data.set_base_invisible_flag(display.invisible);
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }

    /// Vanilla `Parrot.imitateNearbyMobs`.
    fn imitate_nearby_mobs(&self) -> bool {
        if !Entity::is_alive(self)
            || self.is_silent()
            || rand::random_range(0..IMITATE_ROLL_SUCCESS_CHANCE) != 0
        {
            return false;
        }

        let Some(world) = self.level() else {
            return false;
        };
        let search_box = self.bounding_box().inflate(IMITATE_NEARBY_RANGE);
        let candidates = world.get_entities_in_aabb_matching(&search_box, |entity| {
            imitated_sound(entity.entity_type()).is_some()
        });
        if candidates.is_empty() {
            return false;
        }

        let chosen = Arc::clone(&candidates[rand::random_range(0..candidates.len())]);
        if chosen.is_silent() {
            return false;
        }

        let sound =
            imitated_sound(chosen.entity_type()).unwrap_or(&sound_events::ENTITY_PARROT_AMBIENT);
        world.play_sound_at(
            sound,
            SoundSource::Neutral,
            self.position(),
            IMITATE_SOUND_VOLUME,
            parrot_pitch(),
            None,
        );
        true
    }

    /// Vanilla `Parrot.aiStep` jukebox bookkeeping for the dancing animation.
    fn update_party_parrot(&self) {
        let jukebox = self.parrot_state.lock().jukebox;
        let still_valid = match (jukebox, self.level()) {
            (Some(jukebox), Some(world)) => {
                jukebox.closer_to_center_than(self.position(), PARTY_PARROT_RANGE)
                    && world.get_block_state(jukebox).get_block() == &vanilla_blocks::JUKEBOX
            }
            _ => false,
        };

        if still_valid {
            return;
        }

        let mut state = self.parrot_state.lock();
        state.party_parrot = false;
        state.jukebox = None;
    }

    /// Vanilla `Parrot.calculateFlapping`.
    fn calculate_flapping(&self) {
        let airborne = !self.on_ground() && !self.is_passenger();
        {
            let mut state = self.parrot_state.lock();
            let delta = if airborne {
                FLAP_AIRBORNE_GAIN
            } else {
                FLAP_GROUNDED_LOSS
            };
            state.o_flap = state.flap;
            state.o_flap_speed = state.flap_speed;
            state.flap_speed = (state.flap_speed + delta * FLAP_SPEED_SCALE).clamp(0.0, 1.0);
            if airborne && state.flapping < MIN_FLAPPING_STRENGTH {
                state.flapping = MIN_FLAPPING_STRENGTH;
            }
            state.flapping *= FLAPPING_STRENGTH_DECAY;
        }

        if !self.on_ground() {
            let movement = self.velocity();
            if movement.y < 0.0 {
                self.set_velocity(DVec3::new(
                    movement.x,
                    movement.y * FLAP_FALL_DRAG_Y,
                    movement.z,
                ));
            }
        }

        let mut state = self.parrot_state.lock();
        state.flap += state.flapping * FLAP_ROTATION_SCALE;
    }
}

/// Sets one vanilla `TamableAnimal.DATA_FLAGS_ID` bit.
fn set_tamable_flag(entity_data: &SyncMutex<ParrotEntityData>, flag: i8, value: bool) {
    let mut entity_data = entity_data.lock();
    let current = *entity_data.tamable_animal().flags.get();
    let next = if value {
        current | flag
    } else {
        current & !flag
    };
    entity_data.tamable_animal_mut().flags.set(next);
}

impl Tameable for ParrotEntity {
    fn owner(&self) -> Option<SharedEntity> {
        ParrotEntity::owner(self)
    }

    fn is_tame(&self) -> bool {
        ParrotEntity::is_tame(self)
    }

    fn is_ordered_to_sit(&self) -> bool {
        ParrotEntity::is_ordered_to_sit(self)
    }

    fn set_in_sitting_pose(&self, sitting: bool) {
        ParrotEntity::set_in_sitting_pose(self, sitting);
    }

    fn is_in_sitting_pose(&self) -> bool {
        ParrotEntity::is_in_sitting_pose(self)
    }

    fn unable_to_move_to_owner(&self) -> bool {
        ParrotEntity::unable_to_move_to_owner(self)
    }

    fn should_try_teleport_to_owner(&self) -> bool {
        ParrotEntity::should_try_teleport_to_owner(self)
    }

    fn try_to_teleport_to_owner(&self) {
        ParrotEntity::try_to_teleport_to_owner(self);
    }

    fn wants_to_attack(&self, _target: &SharedEntity, _owner: &SharedEntity) -> bool {
        // Vanilla `TamableAnimal.wantsToAttack` returns true unconditionally, and
        // `Parrot` adds no override.
        true
    }
}

impl ShoulderRiding for ParrotEntity {
    fn can_sit_on_shoulder(&self) -> bool {
        ParrotEntity::can_sit_on_shoulder(self)
    }

    fn set_entity_on_shoulder(&self, owner: &SharedEntity) -> bool {
        ParrotEntity::set_entity_on_shoulder(self, owner)
    }
}

impl Entity for ParrotEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(variant) = item_stack.get(PARROT_VARIANT) {
            self.set_variant(*variant);
        }
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn tick(self: Arc<Self>) {
        {
            let mut state = self.parrot_state.lock();
            state.ride_cooldown_counter += 1;
        }

        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        // Vanilla `Parrot.canBeABaby()` is false, so there are no baby
        // dimensions and only the generic scale applies.
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(
            &sound_events::ENTITY_PARROT_STEP,
            STEP_SOUND_VOLUME,
            STEP_SOUND_PITCH,
        );
    }

    fn is_flapping(&self) -> bool {
        self.base().movement_progress().fly_dist() > self.parrot_state.lock().next_flap
    }

    fn on_flap(&self) {
        self.play_sound(
            &sound_events::ENTITY_PARROT_FLY,
            STEP_SOUND_VOLUME,
            STEP_SOUND_PITCH,
        );

        let fly_dist = self.base().movement_progress().fly_dist();
        let mut state = self.parrot_state.lock();
        state.next_flap = fly_dist + state.flap_speed / NEXT_FLAP_SPEED_DIVISOR;
    }

    fn check_fall_damage(
        self: Arc<Self>,
        _vertical_movement: f64,
        _on_ground: bool,
        _on_state: BlockStateId,
        _pos: BlockPos,
        _world: &Arc<World>,
    ) {
        // Vanilla `Parrot.checkFallDamage` is intentionally empty.
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn is_tame_owned_by(&self, owner: &dyn LivingEntity) -> bool {
        self.is_tame() && self.owner_uuid() == Some(owner.uuid())
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_animal(nbt);
        // Vanilla gates age saving on `canBeABaby()`, which is false for parrots.
        nbt.insert("Variant", self.variant().id());
        if let Some(owner) = self.owner_uuid() {
            nbt.insert("Owner", NbtTag::IntArray(owner.to_int_array().to_vec()));
        }
        nbt.insert("Sitting", i8::from(self.is_ordered_to_sit()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_animal(nbt);

        self.set_variant(
            nbt.int("Variant")
                .map_or(ParrotVariant::default(), ParrotVariant::by_id),
        );

        let owner = nbt
            .int_array("Owner")
            .and_then(|owner| Uuid::from_int_array(&owner));
        self.set_owner_uuid(owner);
        self.set_tame(owner.is_some());

        let ordered_to_sit = nbt.byte("Sitting").is_some_and(|value| value != 0);
        self.set_ordered_to_sit(ordered_to_sit);
        self.set_in_sitting_pose(ordered_to_sit);
    }
}

impl LivingEntity for ParrotEntity {
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
        PARROT_SOUND_VOLUME
    }

    fn voice_pitch(&self) -> f32 {
        parrot_pitch()
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PARROT_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PARROT_DEATH)
    }

    fn do_push(&self, entity: &SharedEntity) {
        // Vanilla `Parrot.doPush` never pushes players.
        if entity.as_player().is_some() {
            return;
        }

        entity.push_entity(self.as_entity_event_source());
    }

    fn before_actually_hurt(&self, _source: &DamageSource, _amount: f32) {
        // Vanilla `Parrot.hurtServer` drops the sitting order on any damage, then
        // defers to `LivingEntity.beforeActuallyHurt` (the animal love reset).
        self.set_ordered_to_sit(false);
        if let Some(animal) = self.as_animal() {
            animal.reset_love();
        }
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.update_party_parrot();

        let result = Mob::mob_ai_step(self, entity);

        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        self.calculate_flapping();
        result
    }
}

impl AgeableMob for ParrotEntity {
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

    fn age_boundary_changed(&self, _baby: bool) {
        self.refresh_dimensions();
    }

    fn is_baby(&self) -> bool {
        // Vanilla `Parrot.canBeABaby()` is always false.
        false
    }

    fn set_baby(&self, _baby: bool) {
        // Vanilla `AgeableMob.setBaby` is gated on `canBeABaby()`.
    }

    fn get_breed_offspring(
        &self,
        _world: &Arc<World>,
        _partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        // Vanilla `Parrot.getBreedOffspring` always returns null.
        None
    }
}

impl Animal for ParrotEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn can_mate(&self, _partner: &dyn Animal) -> bool {
        // Vanilla `Parrot.canMate` is always false.
        false
    }

    fn is_food(&self, _item_stack: &ItemStack) -> bool {
        // Vanilla `Parrot.isFood` is always false; parrots cannot breed.
        false
    }

    /// Vanilla `Parrot.checkParrotSpawnRules`.
    fn check_animal_spawn_rules(
        level: &dyn LevelReader,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        level
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&BlockTag::PARROTS_SPAWNABLE_ON)
            && Self::is_bright_enough_to_spawn(level, pos)
    }
}

impl Mob for ParrotEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn tick_move_control(&self) {
        // Vanilla `FlyingMoveControl(this, 10, false)`: turning, pitching, the
        // flying-speed write and the gravity flag all come from the shared
        // implementation, which keeps the forward travel input at the flying speed.
        tick_flying_move_control(self, FLYING_MOVE_CONTROL);
    }

    fn custom_server_ai_step(&self) {
        Animal::custom_server_ai_step_animal(self);
        if rand::random_range(0..IMITATE_CHANCE_DIVISOR) == 0 {
            self.imitate_nearby_mobs();
        }
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        let world = self.level()?;
        if world.difficulty() != Difficulty::Peaceful
            && rand::random_range(0..AMBIENT_IMITATE_CHANCE_DIVISOR) == 0
        {
            return Some(IMITATED_SOUNDS[rand::random_range(0..IMITATED_SOUNDS.len())].1);
        }

        Some(&sound_events::ENTITY_PARROT_AMBIENT)
    }

    fn mob_can_be_leashed(&self) -> bool {
        // Vanilla `TamableAnimal.canBeLeashed` is always true.
        true
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_variant(ParrotVariant::VALUES[rand::random_range(0..ParrotVariant::VALUES.len())]);
        // Vanilla `Parrot.finalizeSpawn` defaults to `AgeableMobGroupData(false)`.
        let group_data = group_data.or(Some(SpawnGroupData::AgeableMob(
            AgeableMobGroupData::with_should_spawn_baby(false),
        )));
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if !self.is_tame()
            && REGISTRY
                .items
                .is_in_tag(item_stack.item(), &ItemTag::PARROT_FOOD)
        {
            Mob::use_player_item(self, player, hand);
            if !self.is_silent() {
                let pitch =
                    1.0 + (rand::random::<f32>() - rand::random::<f32>()) * VOICE_PITCH_JITTER;
                self.play_sound(&sound_events::ENTITY_PARROT_EAT, PARROT_SOUND_VOLUME, pitch);
            }

            if rand::random_range(0..TAMING_SUCCESS_CHANCE) == 0 {
                self.tame(player);
                self.broadcast_entity_event(EntityStatus::TamingSucceeded);
            } else {
                self.broadcast_entity_event(EntityStatus::TamingFailed);
            }

            return InteractionResult::Success;
        }

        if !REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::PARROT_POISONOUS_FOOD)
        {
            if !self.is_flying() && self.is_tame() && self.is_owned_by(player) {
                self.set_ordered_to_sit(!self.is_ordered_to_sit());
                return InteractionResult::Success;
            }

            return Animal::mob_interact_animal(self, player, hand);
        }

        Mob::use_player_item(self, player, hand);
        let _ = self.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::POISON,
            POISON_DURATION,
            0,
        ));

        if (player.has_infinite_materials() || !self.is_invulnerable())
            && let Some(world) = self.level()
        {
            let source = world.get_entity_by_id(player.id()).map_or_else(
                || DamageSource::environment(&vanilla_damage_types::PLAYER_ATTACK),
                |attacker| DamageSource::direct(&vanilla_damage_types::PLAYER_ATTACK, attacker),
            );
            let _ = self.hurt(&world, &source, f32::MAX);
        }

        InteractionResult::Success
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for ParrotEntity {}

/// Vanilla `Parrot.ParrotWanderGoal` (a private vanilla inner class).
struct ParrotWanderGoal {
    wanted_position: Option<DVec3>,
}

impl ParrotWanderGoal {
    const fn new() -> Self {
        Self {
            wanted_position: None,
        }
    }

    /// Vanilla `Parrot.ParrotWanderGoal.getPosition`.
    fn pick_position(mob: &dyn PathfinderMob) -> DVec3 {
        if mob.is_in_water()
            && let Some(position) = water_position(mob)
        {
            return position;
        }

        if rand::random::<f32>() >= WANDER_PROBABILITY
            && let Some(position) = tree_position(mob)
        {
            return position;
        }

        aerial_position(mob)
    }
}

impl Goal for ParrotWanderGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if mob.controlling_passenger().is_some() {
            return false;
        }
        if mob.no_action_time() >= NO_ACTION_TIME_LIMIT {
            return false;
        }
        if rand::random_range(0..reduced_tick_delay(WANDER_INTERVAL)) != 0 {
            return false;
        }

        self.wanted_position = Some(Self::pick_position(mob));
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(wanted) = self.wanted_position else {
            return false;
        };
        if mob.controlling_passenger().is_some() {
            return false;
        }

        mob.distance_to_sqr(wanted) > ARRIVAL_DISTANCE_SQ
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(wanted) = self.wanted_position {
            // GAP: vanilla walks a `FlyingPathNavigation` path; Steel steers the
            // flying move control directly.
            mob.set_wanted_position(wanted, WANDER_SPEED);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.wanted_position = None;
        mob.mob_base().navigation().lock().stop();
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if let Some(wanted) = self.wanted_position {
            mob.set_wanted_position(wanted, WANDER_SPEED);
        }
    }
}

/// Vanilla `LandOnOwnersShoulderGoal`.
struct LandOnOwnersShoulderGoal {
    sitting_on_shoulder: bool,
}

impl LandOnOwnersShoulderGoal {
    const fn new() -> Self {
        Self {
            sitting_on_shoulder: false,
        }
    }
}

impl Goal for LandOnOwnersShoulderGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(shoulder_riding) = mob.as_shoulder_riding() else {
            return false;
        };
        let Some(owner) = shoulder_riding.owner() else {
            return false;
        };
        let Some(player) = owner.as_player() else {
            return false;
        };

        let owner_can_be_sat_on = !player.is_spectator()
            && !player.is_flying_player()
            && !player.is_in_water()
            && !player.is_in_powder_snow();

        !shoulder_riding.is_ordered_to_sit()
            && owner_can_be_sat_on
            && shoulder_riding.can_sit_on_shoulder()
    }

    fn is_interruptable(&self) -> bool {
        !self.sitting_on_shoulder
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.sitting_on_shoulder = false;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if self.sitting_on_shoulder {
            return;
        }

        let Some(shoulder_riding) = mob.as_shoulder_riding() else {
            return;
        };
        if shoulder_riding.is_in_sitting_pose() || mob.is_leashed() {
            return;
        }

        let Some(owner) = shoulder_riding.owner() else {
            return;
        };
        if !mob.bounding_box().intersects(owner.bounding_box()) {
            return;
        }

        self.sitting_on_shoulder = shoulder_riding.set_entity_on_shoulder(&owner);
    }
}
