//! Vanilla Sniffer entity: scenting/sniffing/searching/digging state cycle,
//! dig-position selection, seed drops, and egg-laying on breeding.
//!
//! Vanilla drives the sniffer from `SnifferAi` (a `Brain` with the
//! `NEAREST_LIVING_ENTITIES`/`HURT_BY`/`NEAREST_PLAYERS`/`FOOD_TEMPTATIONS`
//! sensors and the `Scenting`/`Sniffing`/`Searching`/`Digging`/`FinishedDigging`/
//! `FeelingHappy` behaviors). Those generic behaviors are not in Steel's
//! `ai/behavior` library yet, so the cycle ships as one goal
//! ([`SnifferSniffGoal`]) driving the same synced-state transitions, and the
//! brain provider stays a documented gap on [`SnifferEntity::brain`].

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_data::SnifferState;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::SnifferEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::vanilla_loot_tables;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_items,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Downcast, DowncastType, DowncastTypeKey, GlobalPos};

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::goal::{
    BreedGoal, FloatGoal, FollowParentGoal, Goal, GoalControls, LookAtPlayerGoal, PanicGoal,
    RandomLookAroundGoal, TemptGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::leash::Leashable;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData, entity_loot_ref, living_loot_equipment,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `Sniffer.SNIFFER_BABY_START_AGE`.
const SNIFFER_BABY_START_AGE: i32 = -48000;
/// Vanilla `Sniffer.DIGGING_BB_HEIGHT_OFFSET`.
const DIGGING_HEIGHT_OFFSET: f32 = 0.4;
/// Vanilla `Sniffer.DIGGING_DIMENSIONS` eye height.
const DIGGING_EYE_HEIGHT: f32 = 0.81;
/// Vanilla `Sniffer.DIGGING_DROP_SEED_OFFSET_TICKS`.
const DIGGING_DROP_SEED_OFFSET_TICKS: i32 = 120;
/// Vanilla `SnifferAi.SNIFFING_COOLDOWN_TICKS`.
const SNIFFING_COOLDOWN_TICKS: i64 = 9600;
/// Vanilla `Sniffer.getHeadPosition`: `position + forward * 2.25`.
const HEAD_FORWARD_DISTANCE: f64 = 2.25;
/// Vanilla `Sniffer.storeExploredPosition` keeps the newest 20 positions.
const EXPLORED_POSITIONS_LIMIT: usize = 20;
/// Vanilla `Sniffer.calculateDigPosition` tries five expanding radii.
const DIG_POSITION_ATTEMPTS: i32 = 5;
/// Vanilla `Sniffer.calculateDigPosition` vertical search distance.
const DIG_POSITION_VERTICAL_RANGE: i32 = 3;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `SnifferAi.SPEED_MULTIPLIER_WHEN_SNIFFING`.
const SNIFFING_SPEED_MULTIPLIER: f64 = 1.25;

/// Vanilla `SnifferAi.Scenting`/`Sniffing`/`FeelingHappy` durations.
const SCENTING_MIN_TICKS: i32 = 40;
const SCENTING_MAX_TICKS: i32 = 80;
const SNIFFING_MIN_TICKS: i32 = 40;
const SNIFFING_MAX_TICKS: i32 = 80;
const HAPPY_MIN_TICKS: i32 = 40;
const HAPPY_MAX_TICKS: i32 = 100;
/// Vanilla `SnifferAi.Searching` timeout.
const SEARCHING_TIMEOUT_TICKS: i32 = 600;
/// Vanilla `SnifferAi.Digging` duration.
const DIGGING_MIN_TICKS: i32 = 160;
const DIGGING_MAX_TICKS: i32 = 180;
/// Vanilla `SnifferAi.FinishedDigging` duration.
const RISING_TICKS: i32 = 40;

/// Memory types registered by vanilla `Sniffer.BRAIN_PROVIDER`'s sensors and
/// behavior entry conditions.
fn sniffer_memory_types() -> [MemoryModuleTypeRef; 7] {
    [
        &mem::SNIFFER_EXPLORED_POSITIONS,
        &mem::SNIFFER_SNIFFING_TARGET,
        &mem::SNIFFER_DIGGING,
        &mem::SNIFFER_HAPPY,
        &mem::SNIFF_COOLDOWN,
        &mem::TEMPTING_PLAYER,
        &mem::IS_PANICKING,
    ]
}

#[entity_behavior(class = "Sniffer")]
/// Vanilla sniffer with the synced sniff/dig state cycle and seed drops.
pub struct SnifferEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<SnifferEntityData>,
    /// Vanilla `Sniffer.getBrain()`. Only the sniff memories are registered
    /// until the generic behavior library lands; see [`Self::brain`].
    brain: SyncMutex<Brain>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SnifferEntity`.
unsafe impl DowncastType for SnifferEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/sniffer");
}

impl SnifferEntity {
    /// Creates a new sniffer at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a sniffer from persisted base entity state.
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
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        {
            // Vanilla `Sniffer` constructor pathfinding malus overrides.
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::Water, -1.0);
            malus.set(PathType::OnTopOfPowderSnow, -1.0);
            malus.set(PathType::DamageCautious, -1.0);
        }
        mob_base.navigation().lock().set_can_float(true);

        let mut entity_data = SnifferEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `SnifferAi` idle package priorities and speeds.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, SnifferPanicGoal::new(2.0));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                TemptGoal::with_stop_distance(
                    SNIFFING_SPEED_MULTIPLIER,
                    |item_stack| {
                        REGISTRY
                            .items
                            .is_in_tag(item_stack.item(), &ItemTag::SNIFFER_FOOD)
                    },
                    false,
                    3.5,
                ),
            );
            goal_selector.add_goal(4, FollowParentGoal::new(1.25));
            goal_selector.add_goal(5, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(6, RandomLookAroundGoal::new());
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(8, SnifferSniffGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(Brain::new(&sniffer_memory_types(), &[], Vec::new())),
        }
    }

    /// Returns the synced sniff state.
    #[must_use]
    pub fn sniffer_state(&self) -> SnifferState {
        *self.entity_data.lock().sniffer().state.get()
    }

    /// Sets the synced sniff state and refreshes dimensions like vanilla
    /// `Sniffer.onSyncedDataUpdated`.
    pub fn set_sniffer_state(&self, state: SnifferState) {
        self.entity_data.lock().sniffer_mut().state.set(state);
        self.refresh_dimensions();
    }

    /// Vanilla `Sniffer.transitionTo`: sets the state and plays the state sound.
    pub fn transition_to(&self, state: SnifferState) {
        match state {
            SnifferState::Idling | SnifferState::Searching => {}
            SnifferState::FeelingHappy => {
                self.play_sound(&sound_events::ENTITY_SNIFFER_HAPPY, 1.0, 1.0);
            }
            SnifferState::Scenting => {
                let pitch = if AgeableMob::is_baby(self) { 1.3 } else { 1.0 };
                self.play_sound(&sound_events::ENTITY_SNIFFER_SCENTING, 1.0, pitch);
            }
            SnifferState::Sniffing => {
                self.play_sound(&sound_events::ENTITY_SNIFFER_SNIFFING, 1.0, 1.0);
            }
            SnifferState::Digging => {
                self.begin_digging();
                return;
            }
            SnifferState::Rising => {
                self.play_sound(&sound_events::ENTITY_SNIFFER_DIGGING_STOP, 1.0, 1.0);
            }
        }

        self.set_sniffer_state(state);
    }

    /// Vanilla `Sniffer.onDiggingStart`.
    fn begin_digging(&self) {
        self.entity_data
            .lock()
            .sniffer_mut()
            .drop_seed_at_tick
            .set(self.tick_count() + DIGGING_DROP_SEED_OFFSET_TICKS);
        self.broadcast_entity_event(EntityStatus::SnifferDiggingSound);
        self.set_sniffer_state(SnifferState::Digging);
    }

    /// Returns the tick at which a digging sniffer drops its seed.
    #[must_use]
    pub fn drop_seed_at_tick(&self) -> i32 {
        *self.entity_data.lock().sniffer().drop_seed_at_tick.get()
    }

    /// Vanilla `Sniffer.isSearching`.
    #[must_use]
    pub fn is_searching(&self) -> bool {
        self.sniffer_state() == SnifferState::Searching
    }

    /// Vanilla `Sniffer.isTempted`: the `TEMPTING_PLAYER` memory is present.
    #[must_use]
    pub fn is_tempted(&self) -> bool {
        self.brain.lock().has_memory_value(&mem::TEMPTING_PLAYER)
    }

    /// Vanilla `Sniffer.canSniff`.
    #[must_use]
    pub fn can_sniff(&self) -> bool {
        !self.is_tempted()
            && !PathfinderMob::is_panicking(self)
            && !self.is_in_water()
            && !Animal::is_in_love(self)
            && self.on_ground()
            && !self.is_passenger()
            && !self.is_leashed()
    }

    /// Vanilla `Sniffer.canPlayDiggingSound`.
    #[must_use]
    pub fn can_play_digging_sound(&self) -> bool {
        matches!(
            self.sniffer_state(),
            SnifferState::Digging | SnifferState::Searching
        )
    }

    /// Vanilla `Sniffer.getHeadBlock`: two and a quarter blocks ahead, at
    /// `y + 0.2`.
    #[must_use]
    pub fn head_block(&self) -> BlockPos {
        let position = self.head_position();
        BlockPos::containing(position.x, self.position().y + 0.2, position.z)
    }

    fn head_position(&self) -> DVec3 {
        let (yaw, _) = self.rotation();
        let forward = self.calculate_view_vector(0.0, yaw);
        self.position() + forward * HEAD_FORWARD_DISTANCE
    }

    /// Vanilla `Sniffer.canDig`: no panic/temptation, adult, dry, grounded,
    /// unridden, and the block under the head is diggable.
    #[must_use]
    pub fn can_dig(&self) -> bool {
        !PathfinderMob::is_panicking(self)
            && !self.is_tempted()
            && !AgeableMob::is_baby(self)
            && !self.is_in_water()
            && self.on_ground()
            && !self.is_passenger()
            && self.can_dig_at(self.head_block().below())
    }

    /// Vanilla `Sniffer.canDig(BlockPos)`: diggable tag, not already explored,
    /// and reachable.
    fn can_dig_at(&self, position: BlockPos) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        if !world
            .get_block_state(position)
            .get_block()
            .has_tag(&BlockTag::SNIFFER_DIGGABLE_BLOCK)
        {
            return false;
        }
        if self
            .explored_positions()
            .iter()
            .any(|explored| explored.dimension == world.key && explored.pos == position)
        {
            return false;
        }
        // Vanilla checks `createPath(position, 1).canReach()`; a path returned by
        // Steel's pathfinder with reach range 1 already ends within reach.
        self.create_path_to(position, 1).is_some()
    }

    /// Vanilla `Sniffer.calculateDigPosition`: five expanding land searches.
    #[must_use]
    pub fn calculate_dig_position(&self) -> Option<BlockPos> {
        let world = self.level()?;
        for index in 0..DIG_POSITION_ATTEMPTS {
            let horizontal = 10 + 2 * index;
            let Some(candidate) =
                self.random_land_position(horizontal, DIG_POSITION_VERTICAL_RANGE)
            else {
                continue;
            };
            let position = BlockPos::containing(candidate.x, candidate.y, candidate.z).below();
            if !world.is_block_within_world_border(position) {
                continue;
            }
            if self.can_dig_at(position) {
                return Some(position);
            }
        }
        None
    }

    /// Vanilla `LandRandomPos.getPos(mob, xz, y)`: a land position with a solid
    /// floor, at most `xz` blocks away and `y` blocks vertically.
    fn random_land_position(&self, horizontal: i32, vertical: i32) -> Option<DVec3> {
        let world = self.level()?;
        let origin = self.position();
        for _ in 0..10 {
            let dx = rand::random_range(-horizontal..=horizontal);
            let dz = rand::random_range(-horizontal..=horizontal);
            let dy = rand::random_range(-vertical..=vertical);
            let candidate = origin + DVec3::new(f64::from(dx), f64::from(dy), f64::from(dz));
            let pos = BlockPos::containing(candidate.x, candidate.y, candidate.z);
            if world.get_block_state(pos).is_solid_render()
                || !world.get_block_state(pos.above()).is_air()
            {
                continue;
            }
            return Some(candidate);
        }
        None
    }

    /// Vanilla `Sniffer.getExploredPositions`.
    #[must_use]
    pub fn explored_positions(&self) -> Vec<GlobalPos> {
        self.brain
            .lock()
            .get_memory(&mem::SNIFFER_EXPLORED_POSITIONS)
            .unwrap_or_default()
    }

    /// Vanilla `Sniffer.storeExploredPosition`: prepend, keeping the newest 20.
    pub fn store_explored_position(&self, position: BlockPos) {
        let Some(world) = self.level() else {
            return;
        };
        let mut updated: Vec<GlobalPos> = self
            .explored_positions()
            .into_iter()
            .take(EXPLORED_POSITIONS_LIMIT)
            .collect();
        updated.insert(0, GlobalPos::new(world.key.clone(), position));
        self.brain
            .lock()
            .set_memory(&mem::SNIFFER_EXPLORED_POSITIONS, Some(updated));
    }

    /// Vanilla `Sniffer.onDiggingComplete`.
    pub fn on_digging_complete(&self, success: bool) {
        if success {
            self.store_explored_position(self.block_position());
        }
    }

    /// Vanilla `Sniffer.dropSeed`: rolls the dig loot table at the head block.
    fn drop_seed(&self) {
        if self.sniffer_state() != SnifferState::Digging
            || self.drop_seed_at_tick() != self.tick_count()
        {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let head = self.head_block();
        let (x, y, z) = head.get_center();
        let mut rng = rand::rng();
        let equipment = living_loot_equipment(self);
        let mut context = LootContext::new(&mut rng)
            .with_origin(x, y, z)
            .with_game_time(world.game_time())
            .with_this_entity(entity_loot_ref(self, Some(&equipment)));
        let drops = vanilla_loot_tables::GAMEPLAY_SNIFFER_DIGGING.get_random_items(&mut context);
        for drop in drops {
            if let Some(item_entity) = world.spawn_item(DVec3::new(x, y, z), drop) {
                item_entity.set_default_pickup_delay();
            }
        }
        self.play_sound(&sound_events::ENTITY_SNIFFER_DROP_SEED, 1.0, 1.0);
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

        // Sync base entity flags from resolved effect display state in one place.
        self.entity_data.set_base_invisible_flag(display.invisible);
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }

    /// Returns whether an item stack matches the vanilla sniffer food tag.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::SNIFFER_FOOD)
    }

    /// Sets or clears the `SNIFF_COOLDOWN` memory (vanilla `Digging.stop`).
    fn set_sniff_cooldown(&self) {
        self.brain
            .lock()
            .set_memory_with_expiry(&mem::SNIFF_COOLDOWN, (), SNIFFING_COOLDOWN_TICKS);
    }

    /// Returns whether the sniff cooldown has expired.
    fn sniff_cooldown_expired(&self) -> bool {
        !self.brain.lock().has_memory_value(&mem::SNIFF_COOLDOWN)
    }

    /// Vanilla `SnifferAi.resetSniffing`: drop sniff memories and go idle.
    pub fn reset_sniffing(&self) {
        {
            let mut brain = self.brain.lock();
            brain.erase_memory(&mem::SNIFFER_DIGGING);
            brain.erase_memory(&mem::SNIFFER_SNIFFING_TARGET);
        }
        self.transition_to(SnifferState::Idling);
    }

    /// Vanilla `Sniffer.onPathfindingStart`: water becomes traversable when
    /// burning or swimming.
    fn update_pathfinding_for_water(&self) {
        let malus = if self.is_on_fire() || self.is_in_water() {
            0.0
        } else {
            -1.0
        };
        self.mob_base()
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, malus);
    }

    /// Digging dimensions (vanilla `Sniffer.DIGGING_DIMENSIONS`).
    fn digging_dimensions(&self) -> EntityDimensions {
        let base = self.entity_type.dimensions;
        EntityDimensions {
            width: base.width,
            height: base.height - DIGGING_HEIGHT_OFFSET,
            eye_height: DIGGING_EYE_HEIGHT,
            attachments: base.attachments,
        }
        .scale(LivingEntity::get_age_scale(self))
    }
}

impl Entity for SnifferEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        self.update_pathfinding_for_water();
        self.drop_seed();
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        if self.sniffer_state() == SnifferState::Digging {
            self.digging_dimensions()
        } else if AgeableMob::is_baby(self) {
            self.entity_type
                .dimensions
                .scale(LivingEntity::get_age_scale(self))
        } else {
            self.entity_type
                .dimensions
                .scale(LivingEntity::get_scale(self))
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
        self.play_sound(&sound_events::ENTITY_SNIFFER_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        // Vanilla `Sniffer` overrides no `addAdditionalSaveData`: the sniff state,
        // the pending seed tick and the explored positions are runtime-only
        // (`explored positions` live in the brain's `SNIFFER_EXPLORED_POSITIONS`).
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
    }
}

impl LivingEntity for SnifferEntity {
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
        Some(&sound_events::ENTITY_SNIFFER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SNIFFER_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }

    fn on_death(&self, _source: &DamageSource) {
        // Vanilla `Sniffer.die` resets the sniff state before `super.die`;
        // `on_death` runs at the top of the `LivingEntity::die` default body,
        // so this is the same point without recursing into `die`.
        self.transition_to(SnifferState::Idling);
    }

    fn jump_from_ground(&self) {
        // `default_jump_from_ground` is the trait's default body; calling
        // `LivingEntity::jump_from_ground(self, …)` here would recurse.
        self.default_jump_from_ground();
        // Vanilla `Sniffer.jumpFromGround`: nudge forward when nearly still
        // while the move control is driving.
        let speed_modifier = self
            .mob_base()
            .controls()
            .lock()
            .move_control
            .speed_modifier();
        if speed_modifier <= 0.0 {
            return;
        }
        let horizontal_sqr = {
            let velocity = self.velocity();
            velocity.x * velocity.x + velocity.z * velocity.z
        };
        if horizontal_sqr < 0.01 {
            self.move_relative(0.1, DVec3::new(0.0, 0.0, 1.0));
        }
    }
}

impl AgeableMob for SnifferEntity {
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

    fn get_baby_start_age(&self) -> i32 {
        SNIFFER_BABY_START_AGE
    }
}

impl Animal for SnifferEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        SnifferEntity::is_food(item_stack)
    }

    fn can_mate(&self, partner: &dyn Animal) -> bool {
        let Some(partner) = partner
            .as_entity_event_source()
            .downcast_ref::<SnifferEntity>()
        else {
            return false;
        };
        let breedable = |state: SnifferState| {
            matches!(
                state,
                SnifferState::Idling | SnifferState::Scenting | SnifferState::FeelingHappy
            )
        };
        breedable(self.sniffer_state())
            && breedable(partner.sniffer_state())
            // `Animal::can_mate` default, inlined: calling it from this override
            // would resolve back to this method and recurse.
            && self.uuid() != partner.uuid()
            && self.entity_type() == partner.entity_type()
            && self.is_in_love()
            && partner.is_in_love()
    }

    fn spawn_child_from_breeding(&self, world: &Arc<World>, partner: &dyn Animal) {
        // Vanilla `Sniffer.spawnChildFromBreeding` lays a sniffer egg instead of
        // spawning a baby.
        if let Some(egg) =
            world.spawn_item(self.position(), ItemStack::new(&vanilla_items::SNIFFER_EGG))
        {
            egg.set_default_pickup_delay();
        }
        self.finalize_spawn_child_from_breeding(world, partner, None);
        self.play_sound(
            &sound_events::BLOCK_SNIFFER_EGG_PLOP,
            1.0,
            0.5 + (rand::random::<f32>() - rand::random::<f32>()) * 0.2,
        );
    }

    fn play_eating_sound(&self) {
        let pitch = 0.8 + rand::random::<f32>() * 0.4;
        // Vanilla `Sniffer.playEatingSound`: `level.playSound(null, this, SNIFFER_EAT, …)`,
        // the entity overload that sends `ClientboundSoundEntityPacket`.
        if let Some(world) = self.level() {
            world.play_sound_from_entity(
                &sound_events::ENTITY_SNIFFER_EAT,
                SoundSource::Neutral,
                self.id(),
                self.position(),
                1.0,
                pitch,
                None,
            );
        }
    }
}

impl Mob for SnifferEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn custom_server_ai_step(&self) {
        Animal::custom_server_ai_step_animal(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        // Vanilla returns null while digging or searching.
        if matches!(
            self.sniffer_state(),
            SnifferState::Digging | SnifferState::Searching
        ) {
            None
        } else {
            Some(&sound_events::ENTITY_SNIFFER_IDLE)
        }
    }

    fn max_head_y_rot(&self) -> f32 {
        50.0
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let is_food = {
            let inventory = player.inventory.lock();
            SnifferEntity::is_food(inventory.get_item_in_hand(hand))
        };

        let result = Animal::mob_interact_animal(self, player, hand);
        if result.consumes_action() && is_food {
            self.play_eating_sound();
        }

        result
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    fn brain_tick(&self, entity: &SharedEntity) {
        // Vanilla `Sniffer.customServerAiStep` ticks the brain first.
        // GAP(sniffer-brain-provider): the brain has no activities until the
        // generic behaviors (`AnimalPanic`, `AnimalMakeLove`, `FollowTemptation`,
        // `LookAtTargetSink`, `MoveToTargetSink`, `RandomStroll`,
        // `SetEntityLookTarget`, `SetWalkTargetFromLookTarget`, `DoNothing`,
        // `RunOne`, `CountDownCooldownTicks`) and sensors (`FOOD_TEMPTATIONS`,
        // `NEAREST_PLAYERS`) exist; the scent/sniff/search/dig cycle runs as
        // `SnifferSniffGoal` instead.
        let Some(world) = self.level() else {
            return;
        };
        self.brain.lock().tick(world.as_ref(), entity);
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for SnifferEntity {
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        Animal::animal_walk_target_value(self, pos)
    }
}

/// Vanilla `SnifferAi`'s `AnimalPanic` wrapper: panicking resets sniffing.
struct SnifferPanicGoal {
    inner: PanicGoal,
}

impl SnifferPanicGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            inner: PanicGoal::new(speed_modifier),
        }
    }
}

impl Goal for SnifferPanicGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn is_panic_goal(&self) -> bool {
        true
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(sniffer) = mob.as_entity_event_source().downcast_ref::<SnifferEntity>() {
            sniffer.reset_sniffing();
        }
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }
}

/// The `SnifferAi` sniff cycle as a single goal: `Scenting` → `Sniffing` →
/// `Searching` → `Digging` → `Rising` → `FeelingHappy` → idle.
///
/// Vanilla splits these across `Scenting`/`Sniffing`/`Searching`/`Digging`/
/// `FinishedDigging`/`FeelingHappy` behaviors in the `SNIFF`/`DIG` activities;
/// Steel keeps one goal that drives the same synced states, timings, sounds and
/// memories because the generic `Brain` behaviors are not available yet.
struct SnifferSniffGoal {
    phase: SniffPhase,
    timer: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SniffPhase {
    Idle,
    Scenting,
    Sniffing,
    Searching,
    Digging,
    Rising,
    FeelingHappy,
}

impl SnifferSniffGoal {
    const fn new() -> Self {
        Self {
            phase: SniffPhase::Idle,
            timer: 0,
        }
    }

    fn sniffer(mob: &dyn PathfinderMob) -> Option<&SnifferEntity> {
        mob.as_entity_event_source().downcast_ref::<SnifferEntity>()
    }
}

impl Goal for SnifferSniffGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(sniffer) = Self::sniffer(mob) else {
            return false;
        };
        self.phase == SniffPhase::Idle
            && sniffer.sniffer_state() == SnifferState::Idling
            && sniffer.can_sniff()
            && sniffer.sniff_cooldown_expired()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(sniffer) = Self::sniffer(mob) else {
            return false;
        };
        if self.phase == SniffPhase::Idle {
            return false;
        }
        // Vanilla `Searching.canStillUse` gives up when the sniffer can no longer
        // sniff (hurt, wet, leashed, …).
        if self.phase == SniffPhase::Searching && !sniffer.can_sniff() {
            sniffer.transition_to(SnifferState::Idling);
            return false;
        }
        true
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(sniffer) = Self::sniffer(mob) else {
            return;
        };
        self.phase = SniffPhase::Scenting;
        self.timer = rand::random_range(SCENTING_MIN_TICKS..SCENTING_MAX_TICKS);
        sniffer.transition_to(SnifferState::Scenting);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let Some(sniffer) = Self::sniffer(mob) else {
            self.phase = SniffPhase::Idle;
            return;
        };
        // Interrupted mid-cycle: vanilla's behaviors reset to idling.
        if self.phase != SniffPhase::Idle && self.phase != SniffPhase::FeelingHappy {
            sniffer.transition_to(SnifferState::Idling);
        }
        self.phase = SniffPhase::Idle;
        self.timer = 0;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(sniffer) = Self::sniffer(mob) else {
            return;
        };

        self.timer -= 1;
        if self.timer > 0 {
            return;
        }

        match self.phase {
            SniffPhase::Idle => {}
            SniffPhase::Scenting => {
                self.phase = SniffPhase::Sniffing;
                self.timer = rand::random_range(SNIFFING_MIN_TICKS..SNIFFING_MAX_TICKS);
                sniffer.transition_to(SnifferState::Sniffing);
            }
            SniffPhase::Sniffing => {
                let Some(target) = sniffer.calculate_dig_position() else {
                    sniffer.transition_to(SnifferState::Idling);
                    self.phase = SniffPhase::Idle;
                    return;
                };
                let (x, y, z) = target.get_center();
                sniffer.transition_to(SnifferState::Searching);
                sniffer.move_to_pos(DVec3::new(x, y, z), SNIFFING_SPEED_MULTIPLIER);
                self.phase = SniffPhase::Searching;
                self.timer = SEARCHING_TIMEOUT_TICKS;
            }
            SniffPhase::Searching => {
                let arrived = sniffer.mob_base().navigation().lock().is_done();
                if arrived {
                    if sniffer.can_dig() {
                        sniffer.transition_to(SnifferState::Digging);
                        self.phase = SniffPhase::Digging;
                        self.timer = rand::random_range(DIGGING_MIN_TICKS..DIGGING_MAX_TICKS);
                    } else {
                        sniffer.transition_to(SnifferState::Idling);
                        self.phase = SniffPhase::Idle;
                    }
                } else {
                    // Timeout: vanilla `Searching` stops after 600 ticks.
                    sniffer.transition_to(SnifferState::Idling);
                    sniffer.mob_base().navigation().lock().stop();
                    self.phase = SniffPhase::Idle;
                }
            }
            SniffPhase::Digging => {
                sniffer.on_digging_complete(true);
                sniffer.set_sniff_cooldown();
                sniffer.transition_to(SnifferState::Rising);
                self.phase = SniffPhase::Rising;
                self.timer = RISING_TICKS;
            }
            SniffPhase::Rising => {
                sniffer.transition_to(SnifferState::FeelingHappy);
                self.phase = SniffPhase::FeelingHappy;
                self.timer = rand::random_range(HAPPY_MIN_TICKS..HAPPY_MAX_TICKS);
            }
            SniffPhase::FeelingHappy => {
                sniffer.transition_to(SnifferState::Idling);
                self.phase = SniffPhase::Idle;
            }
        }
    }
}
