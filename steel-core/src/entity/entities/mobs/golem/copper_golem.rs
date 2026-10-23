//! Vanilla `CopperGolem` (`net.minecraft.world.entity.animal.golem.CopperGolem`) with the
//! `CopperGolemAi` brain.
//!
//! ## Vanilla state with no Steel counterpart
//!
//! - GAP(copper-golem): chest ferrying — `TransportItemsBetweenContainers` is a movement skeleton
//!   without container IO or its `OnTargetReachedInteraction`/`onStartTravelling`/
//!   `shouldQueueForTarget` callbacks, so no items move and the golem never enters
//!   `GETTING_*`/`DROPPING_*` states from the behavior. Needs container read/write access on the
//!   behavior layer.
//! - GAP(copper-golem): `ContainerUser` (`startOpen`/`stopOpen`/`hasContainerOpen`) — Steel's
//!   container openers are player-typed (`ContainerOpenersCounter::start_open(&Player)`), so a
//!   golem cannot be counted as a viewer.
//! - GAP(copper-golem): statue custom name — `CopperGolemStatueBlockEntity` has no
//!   `create_statue`; the conversion itself (block placement, equipment drop, discard) is
//!   implemented here.
//! - Client-only animation states (`idleAnimationState` and the four interaction animation
//!   states, `setupAnimationStates`) have no server-side effect.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Pose};
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::CopperGolemEntityData;
use steel_registry::vanilla_game_rules::ENTITY_DROPS;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    level_events, sound_events, vanilla_attributes, vanilla_blocks, vanilla_entities,
    vanilla_game_events, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::interact_with_door::InteractWithDoor;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::transport_items_between_containers::TransportItemsBetweenContainers;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::path::PathType;
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, RemovalReason, SharedEntity,
    SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `CopperGolem.IGNORE_WEATHERING_TICK`.
const IGNORE_WEATHERING_TICK: i64 = -2;
/// Vanilla `CopperGolem.UNSET_WEATHERING_TICK`.
const UNSET_WEATHERING_TICK: i64 = -1;
/// Vanilla `CopperGolem.WEATHERING_TICK_FROM`.
const WEATHERING_TICK_FROM: i32 = 504_000;
/// Vanilla `CopperGolem.WEATHERING_TICK_TO`.
const WEATHERING_TICK_TO: i32 = 552_000;
/// Vanilla `CopperGolem.TURN_TO_STATUE_CHANCE`.
const TURN_TO_STATUE_CHANCE: f32 = 0.0058;
/// Vanilla `CopperGolem.SPAWN_COOLDOWN_MIN`.
const SPAWN_COOLDOWN_MIN: i32 = 60;
/// Vanilla `CopperGolem.SPAWN_COOLDOWN_MAX`: exclusive upper bound of `nextInt(60, 100)`.
const SPAWN_COOLDOWN_MAX: i32 = 100;
/// Vanilla `CopperGolemAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 1.5;
/// Vanilla `CopperGolemAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
/// Vanilla `CopperGolemAi.TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS`.
const TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS: i32 = 32;
/// Vanilla `CopperGolemAi.TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS`.
const TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS: i32 = 8;
/// Vanilla `CopperGolemAi` look-target distance.
const MAX_LOOK_DISTANCE: f32 = 6.0;
/// Vanilla `SetEntityLookTargetSometimes.create(EntityTypes.PLAYER, 6.0F, UniformInt.of(40, 80))`.
const LOOK_INTERVAL: steel_utils::value_providers::UniformIntProvider =
    steel_utils::value_providers::UniformIntProvider {
        min_inclusive: 40,
        max_inclusive: 80,
    };
/// Vanilla `CopperGolemAi` random-stroll distances.
const STROLL_HORIZONTAL_DISTANCE: i32 = 2;
const STROLL_VERTICAL_DISTANCE: i32 = 2;
/// Vanilla `CopperGolemAi` idle `DoNothing` bounds.
const IDLE_MIN_DURATION: i32 = 30;
const IDLE_MAX_DURATION: i32 = 60;
/// Vanilla `CopperGolem` navigation path length.
const NAVIGATION_REQUIRED_PATH_LENGTH: f32 = 48.0;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `CopperGolem.EQUIPMENT_SLOT_ANTENNA`: the antenna lives in the saddle slot.
pub const EQUIPMENT_SLOT_ANTENNA: EquipmentSlot = EquipmentSlot::Saddle;

/// Vanilla `WeatheringCopper.WeatherState` for a copper golem's synced oxidation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatheringState {
    Unaffected,
    Exposed,
    Weathered,
    Oxidized,
}

impl WeatheringState {
    /// Synced id (`WeatheringCopper.WeatherState` ordinal).
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Unaffected => 0,
            Self::Exposed => 1,
            Self::Weathered => 2,
            Self::Oxidized => 3,
        }
    }

    /// Resolves a synced id; out-of-range values fall back to `UNAFFECTED`, matching
    /// `WeatheringCopper.WeatherState`'s by-id mapping.
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::Exposed,
            2 => Self::Weathered,
            3 => Self::Oxidized,
            _ => Self::Unaffected,
        }
    }

    /// Vanilla `WeatheringCopper.WeatherState.next`: the next oxidation step, saturated.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Unaffected => Self::Exposed,
            Self::Exposed => Self::Weathered,
            Self::Weathered | Self::Oxidized => Self::Oxidized,
        }
    }

    /// Vanilla `WeatheringCopper.WeatherState.previous`: the previous oxidation step, saturated.
    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Unaffected | Self::Exposed => Self::Unaffected,
            Self::Weathered => Self::Exposed,
            Self::Oxidized => Self::Weathered,
        }
    }

    /// Vanilla `WeatheringCopper.WeatherState.getSerializedName`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unaffected => "unaffected",
            Self::Exposed => "exposed",
            Self::Weathered => "weathered",
            Self::Oxidized => "oxidized",
        }
    }

    /// Parses the NBT name written by [`Self::name`].
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "unaffected" => Some(Self::Unaffected),
            "exposed" => Some(Self::Exposed),
            "weathered" => Some(Self::Weathered),
            "oxidized" => Some(Self::Oxidized),
            _ => None,
        }
    }

    /// Sound level for this oxidation state, mirroring `CopperGolemOxidationLevels`.
    ///
    /// `UNAFFECTED` and `EXPOSED` share the base sounds in vanilla.
    #[must_use]
    pub const fn sounds(self) -> CopperGolemSounds {
        match self {
            Self::Unaffected | Self::Exposed => CopperGolemSounds {
                spin: &sound_events::ENTITY_COPPER_GOLEM_SPIN,
                hurt: &sound_events::ENTITY_COPPER_GOLEM_HURT,
                death: &sound_events::ENTITY_COPPER_GOLEM_DEATH,
                step: &sound_events::ENTITY_COPPER_GOLEM_STEP,
            },
            Self::Weathered => CopperGolemSounds {
                spin: &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_SPIN,
                hurt: &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_HURT,
                death: &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_DEATH,
                step: &sound_events::ENTITY_COPPER_GOLEM_WEATHERED_STEP,
            },
            Self::Oxidized => CopperGolemSounds {
                spin: &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_SPIN,
                hurt: &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_HURT,
                death: &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_DEATH,
                step: &sound_events::ENTITY_COPPER_GOLEM_OXIDIZED_STEP,
            },
        }
    }
}

/// The four sounds a `CopperGolemOxidationLevel` selects.
#[derive(Debug, Clone, Copy)]
pub struct CopperGolemSounds {
    /// `CopperGolemOxidationLevel.spinHeadSound`.
    pub spin: SoundEventRef,
    /// `CopperGolemOxidationLevel.hurtSound`.
    pub hurt: SoundEventRef,
    /// `CopperGolemOxidationLevel.deathSound`.
    pub death: SoundEventRef,
    /// `CopperGolemOxidationLevel.stepSound`.
    pub step: SoundEventRef,
}

/// Vanilla `CopperGolemState`, the synced interaction animation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopperGolemState {
    Idle,
    GettingItem,
    GettingNoItem,
    DroppingItem,
    DroppingNoItem,
}

impl CopperGolemState {
    /// Synced id (`CopperGolemState` ordinal).
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::GettingItem => 1,
            Self::GettingNoItem => 2,
            Self::DroppingItem => 3,
            Self::DroppingNoItem => 4,
        }
    }

    /// Resolves a synced id; unknown values map to `IDLE` like vanilla's continuous by-id map.
    #[must_use]
    pub const fn from_id(id: i32) -> Self {
        match id {
            1 => Self::GettingItem,
            2 => Self::GettingNoItem,
            3 => Self::DroppingItem,
            4 => Self::DroppingNoItem,
            _ => Self::Idle,
        }
    }

    /// Vanilla `CopperGolemState.getSerializedName`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::GettingItem => "getting_item",
            Self::GettingNoItem => "getting_no_item",
            Self::DroppingItem => "dropping_item",
            Self::DroppingNoItem => "dropping_no_item",
        }
    }
}

/// A copper golem: a brain-driven golem that ferries items between copper chests and chests,
/// weathers over time and eventually turns into a statue.
#[entity_behavior(class = "CopperGolem")]
pub struct CopperGolemEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<CopperGolemEntityData>,
    brain: SyncMutex<Brain>,
    opened_chest_pos: SyncMutex<Option<BlockPos>>,
    last_lightning_bolt_uuid: SyncMutex<Option<uuid::Uuid>>,
    next_weathering_tick: SyncMutex<i64>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CopperGolemEntity`.
unsafe impl DowncastType for CopperGolemEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/copper_golem");
}

impl CopperGolemEntity {
    /// Creates a new copper golem at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a copper golem from persisted base entity state.
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

        {
            let mut navigation = mob_base.navigation().lock();
            navigation.set_required_path_length(
                NAVIGATION_REQUIRED_PATH_LENGTH,
                f64::from(entity_type.dimensions.width) + 16.0,
            );
            navigation.set_can_open_doors(true);
        }
        {
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::FireInNeighbor, 16.0);
            malus.set(PathType::DamagingInNeighbor, 16.0);
            malus.set(PathType::Fire, -1.0);
        }

        let mut entity_data = CopperGolemEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data
            .copper_golem_state
            .set(CopperGolemState::Idle.id());

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(Brain::default()),
            opened_chest_pos: SyncMutex::new(None),
            last_lightning_bolt_uuid: SyncMutex::new(None),
            next_weathering_tick: SyncMutex::new(UNSET_WEATHERING_TICK),
        };
        entity.set_persistence_required();
        {
            let mut brain = entity.brain.lock();
            *brain = make_brain();
            brain.register_memory(&mem::TRANSPORT_ITEMS_COOLDOWN_TICKS);
            brain.set_memory(
                &mem::TRANSPORT_ITEMS_COOLDOWN_TICKS,
                Some(rand::random_range(SPAWN_COOLDOWN_MIN..SPAWN_COOLDOWN_MAX)),
            );
        }
        entity
    }

    /// Returns the synced oxidation state.
    #[must_use]
    pub fn weather_state(&self) -> WeatheringState {
        WeatheringState::from_id(self.entity_data.lock().weather_state.get())
    }

    /// Sets the synced oxidation state.
    pub fn set_weather_state(&self, state: WeatheringState) {
        self.entity_data.lock().weather_state.set(state.id());
    }

    /// Returns the synced interaction state.
    #[must_use]
    pub fn state(&self) -> CopperGolemState {
        CopperGolemState::from_id(self.entity_data.lock().copper_golem_state.get())
    }

    /// Sets the synced interaction state.
    pub fn set_state(&self, state: CopperGolemState) {
        self.entity_data.lock().copper_golem_state.set(state.id());
    }

    /// Returns the tick at which the next weathering step is due.
    #[must_use]
    pub fn next_weathering_tick(&self) -> i64 {
        *self.next_weathering_tick.lock()
    }

    /// Sets the tick at which the next weathering step is due.
    pub fn set_next_weathering_tick(&self, tick: i64) {
        *self.next_weathering_tick.lock() = tick;
    }

    /// Returns the chest position this golem currently has open, if any.
    #[must_use]
    pub fn opened_chest_pos(&self) -> Option<BlockPos> {
        *self.opened_chest_pos.lock()
    }

    /// Records the chest this golem opened.
    pub fn set_opened_chest_pos(&self, pos: BlockPos) {
        *self.opened_chest_pos.lock() = Some(pos);
    }

    /// Clears the recorded open chest.
    pub fn clear_opened_chest_pos(&self) {
        *self.opened_chest_pos.lock() = None;
    }

    /// Vanilla `CopperGolem.spawn`: applies the given oxidation state and plays the spawn sound.
    pub fn spawn(&self, weather_state: WeatheringState) {
        self.set_weather_state(weather_state);
        self.play_spawn_sound();
    }

    /// Vanilla `CopperGolem.playSpawnSound`.
    pub fn play_spawn_sound(&self) {
        self.play_sound(&sound_events::ENTITY_COPPER_GOLEM_SPAWN, 1.0, 1.0);
    }

    /// Vanilla `CopperGolem.getSpinHeadSound` for the current oxidation state.
    #[must_use]
    pub fn spin_head_sound(&self) -> SoundEventRef {
        self.weather_state().sounds().spin
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

    /// Vanilla `CopperGolem.updateWeathering`.
    fn update_weathering(&self, game_time: i64) {
        let next = self.next_weathering_tick();
        if next == IGNORE_WEATHERING_TICK {
            return;
        }
        if next == UNSET_WEATHERING_TICK {
            self.set_next_weathering_tick(
                game_time
                    + i64::from(rand::random_range(
                        WEATHERING_TICK_FROM..=WEATHERING_TICK_TO,
                    )),
            );
            return;
        }

        let weather_state = self.weather_state();
        let is_fully_oxidized = weather_state == WeatheringState::Oxidized;
        if game_time >= next && !is_fully_oxidized {
            let new_state = weather_state.next();
            let is_new_state_fully_oxidized = new_state == WeatheringState::Oxidized;
            self.set_weather_state(new_state);
            self.set_next_weathering_tick(if is_new_state_fully_oxidized {
                0
            } else {
                next + i64::from(rand::random_range(
                    WEATHERING_TICK_FROM..=WEATHERING_TICK_TO,
                ))
            });
        }

        if is_fully_oxidized && self.can_turn_to_statue() {
            self.turn_to_statue();
        }
    }

    /// Vanilla `CopperGolem.canTurnToStatue`.
    fn can_turn_to_statue(&self) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        world.get_block_state(self.block_position()).is_air()
            && rand::random::<f32>() <= TURN_TO_STATUE_CHANCE
    }

    /// Vanilla `CopperGolem.turnToStatue`.
    fn turn_to_statue(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let pos = self.block_position();
        let (yaw, _) = self.base.rotation();
        let pose = match rand::random_range(0..4) {
            0 => Pose::Standing,
            1 => Pose::Sitting,
            2 => Pose::Running,
            _ => Pose::Star,
        };
        let state = vanilla_blocks::OXIDIZED_COPPER_GOLEM_STATUE
            .default_state()
            .set_value(&BlockStateProperties::COPPER_GOLEM_POSE, pose)
            .set_value(
                &BlockStateProperties::HORIZONTAL_FACING,
                Direction::from_yaw(yaw),
            );

        world.set_block(pos, state, UpdateFlags::UPDATE_ALL);

        if world.get_block_entity(pos).is_none() {
            return;
        }

        // GAP(copper-golem): `CopperGolemStatueBlockEntity.create_statue` does not exist yet, so
        // the golem's custom name is not copied onto the statue.
        self.drop_preserved_equipment();
        self.set_removed(RemovalReason::Discarded);
        self.play_sound(&sound_events::ENTITY_COPPER_GOLEM_BECOME_STATUE, 1.0, 1.0);

        if self.is_leashed() {
            if world.get_game_rule(&ENTITY_DROPS) {
                self.drop_leash();
            } else {
                self.remove_leash();
            }
        }
    }

    /// Vanilla `Mob.dropPreservedEquipment`: drops every preserved equipment slot at the
    /// entity's position.
    fn drop_preserved_equipment(&self) {
        for slot in EquipmentSlot::ALL {
            if !self.is_equipment_drop_preserved(slot) {
                continue;
            }
            let stack = {
                let mut equipment = self.living_base.equipment().lock();
                if equipment.get_ref(slot).is_empty() {
                    continue;
                }
                equipment.take(slot)
            };
            self.spawn_at_location(stack, 0.0);
        }
    }

    /// Vanilla `CopperGolem.readyForShearing`.
    #[must_use]
    pub fn ready_for_shearing(&self) -> bool {
        let equipment = self.living_base.equipment().lock();
        let stack = equipment.get_ref(EQUIPMENT_SLOT_ANTENNA);
        !stack.is_empty()
            && stack
                .item()
                .is_in_tag(&ItemTag::SHEARABLE_FROM_COPPER_GOLEM)
    }

    /// Vanilla `CopperGolem.shear`: plays the shear sound, drops the antenna and clears the slot.
    pub fn shear(&self) {
        self.play_sound(&sound_events::ENTITY_COPPER_GOLEM_SHEAR, 1.0, 1.0);
        let antenna = {
            let mut equipment = self.living_base.equipment().lock();
            let stack = equipment.get_ref(EQUIPMENT_SLOT_ANTENNA).clone();
            equipment.set(EQUIPMENT_SLOT_ANTENNA, ItemStack::empty());
            stack
        };
        if !antenna.is_empty() {
            self.spawn_at_location(antenna, 1.5);
        }
    }

    /// Vanilla `BehaviorUtils.throwItem` for the golem's held item.
    fn throw_held_item_at(&self, target: DVec3) {
        let held = {
            let mut equipment = self.living_base.equipment().lock();
            let stack = equipment.get_ref(EquipmentSlot::MainHand).clone();
            equipment.set(EquipmentSlot::MainHand, ItemStack::empty());
            stack
        };
        if held.is_empty() {
            return;
        }
        let Some(item_entity) = self.spawn_at_location(held, 0.0) else {
            return;
        };
        /// Vanilla `BehaviorUtils.throwItem`'s `throwVelocity`.
        const THROW_VELOCITY: f64 = 0.3;
        /// Vanilla `BehaviorUtils.throwItem`'s `handYDistanceFromEye`.
        const HAND_Y_DISTANCE_FROM_EYE: f64 = 0.3;
        let origin = self.position() + DVec3::new(0.0, HAND_Y_DISTANCE_FROM_EYE, 0.0);
        let mut velocity = target - origin;
        velocity = if velocity.length_squared() < 1.0e-5 {
            DVec3::ZERO
        } else {
            velocity.normalize() * THROW_VELOCITY
        };
        item_entity.set_velocity(velocity);
        item_entity.set_default_pickup_delay();
    }

    /// Vanilla `CopperGolem.mobInteract`'s shears/honeycomb/axe branches.
    fn interact_with_tools(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held = {
            let inventory = player.inventory.lock();
            inventory.get_item_in_hand(hand).clone()
        };
        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };

        if held.is(&vanilla_items::SHEARS) && self.ready_for_shearing() {
            self.shear();
            world.game_event(
                &vanilla_game_events::SHEAR,
                self.block_position(),
                &GameEventContext::new(None, None),
            );
            player
                .inventory
                .lock()
                .hurt_item_in_hand(hand, 1, player.has_infinite_materials());
            return InteractionResult::Success;
        }

        if held.is(&vanilla_items::HONEYCOMB)
            && self.next_weathering_tick() != IGNORE_WEATHERING_TICK
        {
            let pos = self.block_position();
            world.level_event(level_events::PARTICLES_WAX_ON, pos, 0, None);
            world.play_sound_at(
                &sound_events::ITEM_HONEYCOMB_WAX_ON,
                SoundSource::Blocks,
                self.position(),
                1.0,
                1.0,
                None,
            );
            self.set_next_weathering_tick(IGNORE_WEATHERING_TICK);
            self.use_player_item(player, hand);
            return InteractionResult::Success;
        }

        if held.item().is_in_tag(&ItemTag::AXES)
            && self.next_weathering_tick() == IGNORE_WEATHERING_TICK
        {
            world.play_sound_at(
                &sound_events::ITEM_AXE_SCRAPE,
                self.sound_source(),
                self.position(),
                1.0,
                1.0,
                None,
            );
            world.level_event(
                level_events::PARTICLES_WAX_OFF,
                self.block_position(),
                0,
                None,
            );
            self.set_next_weathering_tick(UNSET_WEATHERING_TICK);
            player
                .inventory
                .lock()
                .hurt_item_in_hand(hand, 1, player.has_infinite_materials());
            return InteractionResult::Success;
        }

        if held.item().is_in_tag(&ItemTag::AXES) {
            let weather_state = self.weather_state();
            if weather_state != WeatheringState::Unaffected {
                world.play_sound_at(
                    &sound_events::ITEM_AXE_SCRAPE,
                    self.sound_source(),
                    self.position(),
                    1.0,
                    1.0,
                    None,
                );
                world.level_event(
                    level_events::PARTICLES_SCRAPE,
                    self.block_position(),
                    0,
                    None,
                );
                self.set_next_weathering_tick(UNSET_WEATHERING_TICK);
                self.set_weather_state(weather_state.previous());
                player
                    .inventory
                    .lock()
                    .hurt_item_in_hand(hand, 1, player.has_infinite_materials());
                return InteractionResult::Success;
            }
        }

        InteractionResult::Pass
    }
}

/// Vanilla `CopperGolem.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 2] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::HURT_BY,
];

/// Vanilla `CopperGolem.BRAIN_PROVIDER.makeBrain`.
fn make_brain() -> Brain {
    Brain::new(&[], &SENSOR_TYPES, activities())
}

/// Vanilla `CopperGolemAi.getActivities`.
fn activities() -> Vec<ActivityData> {
    vec![core_activity(), idle_activity()]
}

/// Vanilla `CopperGolemAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(InteractWithDoor::create()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(&mem::GAZE_COOLDOWN_TICKS)),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(
                &mem::TRANSPORT_ITEMS_COOLDOWN_TICKS,
            )),
        ],
    )
}

/// Vanilla `CopperGolemAi.initIdleActivity`.
///
/// `TransportItemsBetweenContainers` runs as Steel's movement skeleton; see the module GAPs.
fn idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        0,
        vec![
            BehaviorBuilder::boxed(TransportItemsBetweenContainers::new(
                SPEED_MULTIPLIER_WHEN_IDLING,
                TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS,
                TRANSPORT_ITEM_VERTICAL_SEARCH_RADIUS,
            )),
            BehaviorBuilder::boxed(SetEntityLookTargetSometimes::for_type(
                &vanilla_entities::PLAYER,
                MAX_LOOK_DISTANCE,
                LOOK_INTERVAL,
            )),
            BehaviorBuilder::boxed(RunOne::with_entry_condition(
                vec![
                    (mem::WALK_TARGET, MemoryStatus::ValueAbsent),
                    (
                        mem::TRANSPORT_ITEMS_COOLDOWN_TICKS,
                        MemoryStatus::ValuePresent,
                    ),
                ],
                vec![
                    (
                        BehaviorBuilder::boxed(RandomStroll::stroll_with_distances(
                            SPEED_MULTIPLIER_WHEN_IDLING,
                            STROLL_HORIZONTAL_DISTANCE,
                            STROLL_VERTICAL_DISTANCE,
                        )),
                        1,
                    ),
                    (
                        BehaviorBuilder::boxed(DoNothing::new(
                            IDLE_MIN_DURATION,
                            IDLE_MAX_DURATION,
                        )),
                        1,
                    ),
                ],
            )),
        ],
    )
}

/// Vanilla `CopperGolemAi.updateActivity`: outside `CORE` a golem only ever idles.
fn update_activity(brain: &mut Brain) {
    brain.set_active_activity_to_first_valid(&[Activity::Idle]);
}

impl Entity for CopperGolemEntity {
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(self.weather_state().sounds().step, 1.0, 1.0);
    }

    fn thunder_hit(&self, _world: &Arc<World>, bolt: &SharedEntity) {
        let bolt_uuid = bolt.uuid();
        {
            let mut last = self.last_lightning_bolt_uuid.lock();
            if *last == Some(bolt_uuid) {
                return;
            }
            *last = Some(bolt_uuid);
        }
        let weather_state = self.weather_state();
        if weather_state != WeatheringState::Unaffected {
            self.set_next_weathering_tick(UNSET_WEATHERING_TICK);
            self.set_weather_state(weather_state.previous());
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("next_weather_age", self.next_weathering_tick());
        nbt.insert("weather_state", self.weather_state().name());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_next_weathering_tick(
            nbt.long("next_weather_age")
                .unwrap_or(UNSET_WEATHERING_TICK),
        );
        let state = nbt
            .string("weather_state")
            .and_then(|name| WeatheringState::from_name(name.to_str().as_ref()))
            .unwrap_or(WeatheringState::Unaffected);
        self.set_weather_state(state);
    }
}

impl LivingEntity for CopperGolemEntity {
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
        1.0
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(self.weather_state().sounds().hurt)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(self.weather_state().sounds().death)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        let game_time = self.level().map(|world| world.game_time());
        if let Some(game_time) = game_time {
            self.update_weathering(game_time);
        }

        result
    }
}

impl Mob for CopperGolemEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        let mut brain = self.brain.lock();
        brain.tick(&world, entity);
        update_activity(&mut brain);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held = {
            let inventory = player.inventory.lock();
            inventory.get_item_in_hand(hand).clone()
        };

        if held.is_empty() {
            let equipped = {
                let equipment = self.living_base.equipment().lock();
                equipment.get_ref(EquipmentSlot::MainHand).clone()
            };
            if !equipped.is_empty() {
                self.throw_held_item_at(player.position());
                return InteractionResult::Success;
            }
        }

        self.interact_with_tools(player, hand)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.play_spawn_sound();
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for CopperGolemEntity {}
