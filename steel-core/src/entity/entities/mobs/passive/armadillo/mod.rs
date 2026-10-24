//! Vanilla Armadillo entity: roll-up/scare states, scute shedding, and brushing.
//!
//! Vanilla drives the armadillo from `ArmadilloAi` (a `Brain` with the
//! `NEAREST_LIVING_ENTITIES`/`HURT_BY`/`FOOD_TEMPTATIONS`/`NEAREST_ADULT`/
//! `ARMADILLO_SCARE_DETECTED` sensors). The generic behavior library Steel needs
//! for that provider (`AnimalMakeLove`, `AnimalPanic`, `FollowTemptation`,
//! `BabyFollowAdult`, `RandomLookAround`, `RandomStroll`,
//! `SetEntityLookTargetSometimes`, `SetWalkTargetFromLookTarget`, `RunOne`,
//! `CountDownCooldownTicks`) and the `FOOD_TEMPTATIONS`/`NEAREST_ADULT`/
//! `ARMADILLO_SCARE_DETECTED` sensors do not exist yet, so this entity ships the
//! same state machine goal-based plus an inline port of the scare sensor and of
//! `ArmadilloAi.ArmadilloBallUp`; see [`ArmadilloEntity::brain`].

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::wrap_degrees;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_data::ArmadilloState;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::ArmadilloEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::vanilla_loot_tables;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_damage_type_tags,
    vanilla_entity_type_tags, vanilla_game_events, vanilla_items,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Downcast, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::goal::{
    BreedGoal, FloatGoal, FollowParentGoal, Goal, GoalControls, LookAtPlayerGoal, PanicGoal,
    RandomLookAroundGoal, TemptGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::leash::Leashable;
use crate::entity::living_entity::shearing_loot_items_with_rng;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, LivingTravelInput, Mob,
    MobBase, PathfinderMob, SharedEntity, SpawnGroupData, entity_loot_ref, living_loot_equipment,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::{LevelReader, World};

/// Vanilla `Armadillo.BABY_SCALE`.
const ARMADILLO_BABY_SCALE: f32 = 0.6;
/// Vanilla `BABY_DIMENSIONS.withEyeHeight(0.21875F)`.
const ARMADILLO_BABY_EYE_HEIGHT: f32 = 0.21875;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `Armadillo.SCARE_DISTANCE_HORIZONTAL`.
const SCARE_DISTANCE_HORIZONTAL: f64 = 7.0;
/// Vanilla `Armadillo.SCARE_DISTANCE_VERTICAL`.
const SCARE_DISTANCE_VERTICAL: f64 = 2.0;
/// Vanilla `Armadillo.SCARE_CHECK_INTERVAL` (`MobSensor` scan rate).
const SCARE_CHECK_INTERVAL: i32 = 80;
/// Vanilla `MobSensor` memory time-to-live for `DANGER_DETECTED_RECENTLY`.
const DANGER_DETECTED_EXPIRY_TICKS: i64 = 80;
/// Vanilla `ArmadilloState.UNROLLING.animationDuration()`.
const UNROLLING_ANIMATION_TICKS: i64 = 30;
/// Vanilla `ArmadilloState.ROLLING.animationDuration()`.
const ROLLING_ANIMATION_TICKS: i64 = 10;
/// Vanilla `ArmadilloState.SCARED.animationDuration()`.
const SCARED_ANIMATION_TICKS: i32 = 50;
/// Vanilla `ArmadilloBallUp.BALL_UP_STAY_IN_STATE`: 5 minutes.
const BALL_UP_STAY_IN_STATE: i64 = 5 * 60 * 20;
/// Vanilla `ArmadilloBallUp.DANGER_DETECTED_RECENTLY_DANGER_THRESHOLD`.
const DANGER_THRESHOLD_TICKS: i64 = 75;
/// Vanilla `Armadillo.pickNextScuteDropTime` lower bound: 5 minutes.
const SCUTE_DROP_MIN_TICKS: i32 = 20 * 60 * 5;
/// Vanilla `Armadillo.pickNextScuteDropTime` random range: 5 more minutes.
const SCUTE_DROP_RANDOM_RANGE_TICKS: i32 = 20 * 60 * 5;
/// Vanilla brush durability cost (`Armadillo.mobInteract`).
const BRUSH_DURABILITY_COST: i32 = 16;
/// Vanilla `Armadillo.brushOffScute` spawn height offset.
const SCUTE_SPAWN_Y_OFFSET: f64 = 1.0;

/// Memory types registered by vanilla `Armadillo.BRAIN_PROVIDER`'s sensors and
/// behavior entry conditions.
fn armadillo_memory_types() -> [MemoryModuleTypeRef; 1] {
    [&mem::DANGER_DETECTED_RECENTLY]
}

#[entity_behavior(class = "Armadillo")]
/// Vanilla armadillo with synced shell state, scute shed timer, and brushing.
pub struct ArmadilloEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<ArmadilloEntityData>,
    /// Vanilla `Armadillo.getBrain()`. Only the danger memory is registered
    /// until the generic behavior library lands; see [`Self::brain`].
    brain: SyncMutex<Brain>,
    /// Ticks spent in the current shell state (vanilla `Armadillo.inStateTicks`).
    in_state_ticks: SyncMutex<i64>,
    /// Ticks until the next scute shed (vanilla `Armadillo.scuteTime`).
    scute_time: SyncMutex<i32>,
    /// Ticks until the next `ArmadilloBallUp` peek (`nextPeekTimer`).
    next_peek_timer: SyncMutex<i32>,
    /// Whether danger was detected on the previous ball-up tick.
    danger_was_around: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ArmadilloEntity`.
unsafe impl DowncastType for ArmadilloEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/armadillo");
}

impl ArmadilloEntity {
    /// Creates a new armadillo at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs an armadillo from persisted base entity state.
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
        let mut entity_data = ArmadilloEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `ArmadilloAi` idle package priorities and speeds.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, ArmadilloPanicGoal::new(2.0));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(
                3,
                TemptGoal::new(
                    1.25,
                    |item_stack| {
                        REGISTRY
                            .items
                            .is_in_tag(item_stack.item(), &ItemTag::ARMADILLO_FOOD)
                    },
                    false,
                ),
            );
            goal_selector.add_goal(4, FollowParentGoal::new(1.25));
            goal_selector.add_goal(5, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(6, RandomLookAroundGoal::new());
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(Brain::new(&armadillo_memory_types(), &[], Vec::new())),
            in_state_ticks: SyncMutex::new(0),
            scute_time: SyncMutex::new(Self::pick_next_scute_drop_time()),
            next_peek_timer: SyncMutex::new(0),
            danger_was_around: SyncMutex::new(false),
        }
    }

    /// Vanilla `Armadillo.pickNextScuteDropTime`: 5 to 10 minutes of ticks.
    fn pick_next_scute_drop_time() -> i32 {
        rand::random_range(0..SCUTE_DROP_RANDOM_RANGE_TICKS) + SCUTE_DROP_MIN_TICKS
    }

    /// Returns the synced shell state.
    #[must_use]
    pub fn armadillo_state(&self) -> ArmadilloState {
        *self.entity_data.lock().armadillo().armadillo_state.get()
    }

    /// Sets the shell state, resetting the in-state timer like
    /// `Armadillo.onSyncedDataUpdated`.
    pub fn switch_to_state(&self, state: ArmadilloState) {
        self.entity_data
            .lock()
            .armadillo_mut()
            .armadillo_state
            .set(state);
        *self.in_state_ticks.lock() = 0;
    }

    /// Vanilla `Armadillo.isScared`.
    #[must_use]
    pub fn is_scared(&self) -> bool {
        self.armadillo_state() != ArmadilloState::Idle
    }

    /// Vanilla `Armadillo.shouldHideInShell`.
    #[must_use]
    pub fn should_hide_in_shell(&self) -> bool {
        let ticks_in_state = *self.in_state_ticks.lock();
        match self.armadillo_state() {
            ArmadilloState::Idle => false,
            ArmadilloState::RollingUp => ticks_in_state > 5,
            ArmadilloState::Rolled | ArmadilloState::Scared => true,
            ArmadilloState::Unrolling => ticks_in_state < 26,
        }
    }

    /// Vanilla `Armadillo.shouldSwitchToScaredState`.
    #[must_use]
    pub fn should_switch_to_scared_state(&self) -> bool {
        self.armadillo_state() == ArmadilloState::RollingUp
            && *self.in_state_ticks.lock() > ROLLING_ANIMATION_TICKS
    }

    /// Vanilla `Armadillo.rollUp`.
    pub fn roll_up(&self) {
        if self.is_scared() {
            return;
        }

        self.mob_base().navigation().lock().stop();
        self.living_base().set_speed(0.0);
        self.living_base().set_travel_input(LivingTravelInput::ZERO);
        self.reset_love();
        self.game_event(&vanilla_game_events::ENTITY_ACTION);
        self.make_sound(Some(&sound_events::ENTITY_ARMADILLO_ROLL));
        self.switch_to_state(ArmadilloState::RollingUp);
    }

    /// Vanilla `Armadillo.rollOut`.
    pub fn roll_out(&self) {
        if !self.is_scared() {
            return;
        }

        self.game_event(&vanilla_game_events::ENTITY_ACTION);
        self.make_sound(Some(&sound_events::ENTITY_ARMADILLO_UNROLL_FINISH));
        self.switch_to_state(ArmadilloState::Idle);
    }

    /// Vanilla `Armadillo.canStayRolledUp`.
    #[must_use]
    pub fn can_stay_rolled_up(&self) -> bool {
        !PathfinderMob::is_panicking(self)
            && !self.is_in_liquid()
            && !self.is_leashed()
            && !self.is_passenger()
            && !Entity::is_vehicle(self)
    }

    /// Vanilla `Armadillo.isScaredBy`: nearby undead, the last hurter, or a
    /// sprinting/riding non-spectator player inside the 7x2x7 scare box.
    #[must_use]
    pub fn is_scared_by(&self, living: &dyn LivingEntity) -> bool {
        let bounds = self.bounding_box().inflate_xyz(
            SCARE_DISTANCE_HORIZONTAL,
            SCARE_DISTANCE_VERTICAL,
            SCARE_DISTANCE_HORIZONTAL,
        );
        if !bounds.intersects(living.bounding_box()) {
            return false;
        }
        if REGISTRY.entity_types.is_in_tag(
            living.entity_type(),
            &vanilla_entity_type_tags::EntityTypeTag::UNDEAD,
        ) {
            return true;
        }
        if self
            .last_hurt_by_mob()
            .is_some_and(|hurter| hurter.uuid() == living.uuid())
        {
            return true;
        }
        living.as_player().is_some_and(|player| {
            !Entity::is_spectator(player)
                && (LivingEntity::is_sprinting(player) || player.is_passenger())
        })
    }

    /// Returns whether this armadillo is in liquid (vanilla `isInLiquid`).
    fn is_in_liquid(&self) -> bool {
        self.is_in_water() || self.is_in_lava()
    }

    /// Runs the `ARMADILLO_SCARE_DETECTED` sensor: every 80 ticks, when the
    /// armadillo can stay rolled up, mark danger for a nearby scaring entity.
    fn tick_scare_detection(&self) {
        if self.tick_count() % SCARE_CHECK_INTERVAL != 0 {
            return;
        }

        if !self.can_stay_rolled_up() {
            self.brain
                .lock()
                .erase_memory(&mem::DANGER_DETECTED_RECENTLY);
            return;
        }

        let Some(world) = self.level() else {
            return;
        };
        let bounds = self.bounding_box().inflate_xyz(
            SCARE_DISTANCE_HORIZONTAL,
            SCARE_DISTANCE_VERTICAL,
            SCARE_DISTANCE_HORIZONTAL,
        );
        let danger = world.get_entities_in_aabb(&bounds).iter().any(|entity| {
            entity
                .as_living_entity()
                .is_some_and(|living| self.is_scared_by(living))
        });
        if danger {
            self.brain.lock().set_memory_with_expiry(
                &mem::DANGER_DETECTED_RECENTLY,
                true,
                DANGER_DETECTED_EXPIRY_TICKS,
            );
        }
    }

    /// Vanilla `ArmadilloAi.ARMADILLO_ROLLING_OUT` + `ArmadilloBallUp`.
    ///
    /// The rolling-out one-shot fires when `DANGER_DETECTED_RECENTLY` is absent;
    /// `ArmadilloBallUp` starts on the PANIC activity (danger present, not
    /// panicking) and then owns the rolling → scared → unrolling → idle cycle.
    fn tick_shell_behavior(&self) {
        let danger_present = self
            .brain
            .lock()
            .has_memory_value(&mem::DANGER_DETECTED_RECENTLY);

        if self.is_scared() {
            if !danger_present {
                self.roll_out();
                return;
            }
            if !self.can_stay_rolled_up() {
                self.roll_out();
                return;
            }
        } else if danger_present
            && !PathfinderMob::is_panicking(self)
            && self.on_ground()
            && !self.is_in_liquid()
        {
            self.roll_up();
            return;
        }

        if !self.is_scared() {
            return;
        }

        // `ArmadilloBallUp.tick`.
        if *self.in_state_ticks.lock() > BALL_UP_STAY_IN_STATE {
            self.roll_out();
            return;
        }

        {
            let mut next_peek_timer = self.next_peek_timer.lock();
            if *next_peek_timer > 0 {
                *next_peek_timer -= 1;
            }
        }

        if self.should_switch_to_scared_state() {
            self.switch_to_state(ArmadilloState::Scared);
            if self.on_ground() {
                self.make_sound(Some(&sound_events::ENTITY_ARMADILLO_LAND));
            }
            return;
        }

        let danger_ticks = self
            .brain
            .lock()
            .time_until_expiry(&mem::DANGER_DETECTED_RECENTLY);
        let danger_is_around = danger_ticks > DANGER_THRESHOLD_TICKS;
        {
            let mut danger_was_around = self.danger_was_around.lock();
            if danger_is_around != *danger_was_around {
                *self.next_peek_timer.lock() = Self::pick_next_peek_timer();
            }
            *danger_was_around = danger_is_around;
        }

        match self.armadillo_state() {
            ArmadilloState::Scared => {
                if *self.next_peek_timer.lock() == 0 && self.on_ground() && danger_is_around {
                    self.broadcast_entity_event(EntityStatus::ArmadilloPeek);
                    *self.next_peek_timer.lock() = Self::pick_next_peek_timer();
                }

                if danger_ticks < UNROLLING_ANIMATION_TICKS {
                    self.make_sound(Some(&sound_events::ENTITY_ARMADILLO_UNROLL_START));
                    self.switch_to_state(ArmadilloState::Unrolling);
                }
            }
            ArmadilloState::Unrolling if danger_ticks > UNROLLING_ANIMATION_TICKS => {
                self.switch_to_state(ArmadilloState::Scared);
            }
            _ => {}
        }
    }

    /// Vanilla `ArmadilloBallUp.pickNextPeekTimer`.
    fn pick_next_peek_timer() -> i32 {
        SCARED_ANIMATION_TICKS + rand::random_range(100..=400)
    }

    /// Vanilla `Armadillo.customServerAiStep` scute-shed timer.
    fn tick_scute_shed(&self) {
        if !Entity::is_alive(self) {
            return;
        }

        {
            let mut scute_time = self.scute_time.lock();
            *scute_time -= 1;
            if *scute_time > 0 {
                return;
            }
        }

        *self.scute_time.lock() = Self::pick_next_scute_drop_time();
        let Some(world) = self.level() else {
            return;
        };
        if !self.should_drop_loot(world.as_ref()) {
            return;
        }

        let mut rng = rand::rng();
        let position = self.position();
        let equipment = living_loot_equipment(self);
        let mut context = LootContext::new(&mut rng)
            .with_origin(position.x, position.y, position.z)
            .with_game_time(world.game_time())
            .with_this_entity(entity_loot_ref(self, Some(&equipment)));
        let drops = vanilla_loot_tables::GAMEPLAY_ARMADILLO_SHED.get_random_items(&mut context);
        if drops.is_empty() {
            return;
        }

        for drop in drops {
            self.spawn_at_location(drop, SCUTE_SPAWN_Y_OFFSET);
        }
        self.play_sound(
            &sound_events::ENTITY_ARMADILLO_SCUTE_DROP,
            1.0,
            1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.2,
        );
        self.game_event(&vanilla_game_events::ENTITY_PLACE);
    }

    /// Returns the remaining ticks until the next scute shed.
    #[must_use]
    pub fn scute_time(&self) -> i32 {
        *self.scute_time.lock()
    }

    /// Sets the remaining ticks until the next scute shed.
    pub fn set_scute_time(&self, scute_time: i32) {
        *self.scute_time.lock() = scute_time;
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

    /// Returns whether an item stack matches the vanilla armadillo food tag.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::ARMADILLO_FOOD)
    }

    /// Vanilla `Armadillo.brushOffScute`: drops the brush loot table and plays
    /// the brush sound. Returns whether an adult armadillo was brushed.
    pub fn brush_off_scute(&self, tool: &ItemStack) -> bool {
        if AgeableMob::is_baby(self) {
            return false;
        }

        let mut rng = rand::rng();
        for drop in shearing_loot_items_with_rng(
            self,
            &vanilla_loot_tables::BRUSH_ARMADILLO,
            tool,
            &mut rng,
        ) {
            self.spawn_at_location(drop, SCUTE_SPAWN_Y_OFFSET);
        }

        self.play_sound(&sound_events::ENTITY_ARMADILLO_BRUSH, 1.0, 1.0);
        self.game_event(&vanilla_game_events::ENTITY_INTERACT);
        true
    }

    /// Vanilla `Armadillo.actuallyHurt` post-damage reaction.
    fn react_to_damage(&self, source: &DamageSource) {
        if self.is_no_ai() || self.is_dead_or_dying() {
            return;
        }

        if source.causing_entity().is_some() {
            self.brain.lock().set_memory_with_expiry(
                &mem::DANGER_DETECTED_RECENTLY,
                true,
                DANGER_DETECTED_EXPIRY_TICKS,
            );
            if self.can_stay_rolled_up() {
                self.roll_up();
            }
        } else if source.is(&vanilla_damage_type_tags::DamageTypeTag::PANIC_ENVIRONMENTAL_CAUSES) {
            self.roll_out();
        }
    }

    /// Vanilla `Armadillo.tick` head clamp while scared; `getMaxHeadYRot` is 0
    /// then, which freezes the head (`head + delta - clamp(delta, 0, 0)`).
    fn clamp_head_rotation_to_body(&self) {
        if !self.is_scared() {
            return;
        }
        let limit = Mob::max_head_y_rot(self);
        let head_y_rot = self.y_head_rot();
        let delta = wrap_degrees(self.y_body_rot() - head_y_rot);
        self.set_y_head_rot(head_y_rot + delta - delta.clamp(-limit, limit));
    }
}

impl Entity for ArmadilloEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        if self.is_scared() {
            ArmadilloEntity::clamp_head_rotation_to_body(self);
        }
        *self.in_state_ticks.lock() += 1;
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        if AgeableMob::is_baby(self) {
            let scaled = self.entity_type.dimensions.scale(ARMADILLO_BABY_SCALE);
            EntityDimensions {
                eye_height: ARMADILLO_BABY_EYE_HEIGHT,
                ..scaled
            }
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

    /// Vanilla `Armadillo.hurtServer` + `actuallyHurt`: rolled-up damage is
    /// halved as `(damage - 1) / 2`, and a surviving hit updates the danger
    /// memory / rolls the armadillo up.
    ///
    /// Steel hooks this on `Entity::hurt` rather than on
    /// `LivingEntity::hurt_server`/`actually_hurt`: those have no `default_*`
    /// companion, and `Trait::method(self, …)` inside `impl Trait for Self`
    /// resolves back to the override itself (infinite recursion). Same shape as
    /// `BatEntity`'s pre-damage hook.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let amount = if self.is_scared() {
            (amount - 1.0) / 2.0
        } else {
            amount
        };
        let hurt = LivingEntity::hurt_server(self, world, source, amount);
        if hurt {
            // Vanilla reacts after `super.actuallyHurt`, so a killing blow is
            // already dying there; `react_to_damage` keeps the same guard.
            self.react_to_damage(source);
        }
        hurt
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_ARMADILLO_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        // Vanilla `Armadillo.addAdditionalSaveData`: `state` + `scute_time`.
        nbt.insert(
            "state",
            match self.armadillo_state() {
                ArmadilloState::Idle => "idle",
                ArmadilloState::RollingUp => "rolling",
                ArmadilloState::Rolled | ArmadilloState::Scared => "scared",
                ArmadilloState::Unrolling => "unrolling",
            },
        );
        nbt.insert("scute_time", self.scute_time());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);

        let state = match nbt.string("state").map(ToString::to_string) {
            Some(state) if state == "rolling" => ArmadilloState::RollingUp,
            Some(state) if state == "scared" => ArmadilloState::Scared,
            Some(state) if state == "unrolling" => ArmadilloState::Unrolling,
            _ => ArmadilloState::Idle,
        };
        self.switch_to_state(state);
        if let Some(scute_time) = nbt.int("scute_time") {
            self.set_scute_time(scute_time);
        }
    }
}

impl LivingEntity for ArmadilloEntity {
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
        Some(if self.is_scared() {
            &sound_events::ENTITY_ARMADILLO_HURT_REDUCED
        } else {
            &sound_events::ENTITY_ARMADILLO_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ARMADILLO_DEATH)
    }

    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            ARMADILLO_BABY_SCALE
        } else {
            1.0
        }
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
}

impl AgeableMob for ArmadilloEntity {
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
}

impl Animal for ArmadilloEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        ArmadilloEntity::is_food(item_stack)
    }

    fn can_fall_in_love(&self) -> bool {
        // Vanilla `Armadillo.canFallInLove`: inlined `Animal` default
        // (`inLoveTime <= 0`); calling `Animal::can_fall_in_love(self)` here
        // would resolve back to this override and recurse.
        self.in_love_time() <= 0 && !self.is_scared()
    }

    fn play_eating_sound(&self) {
        self.make_sound(Some(&sound_events::ENTITY_ARMADILLO_EAT));
    }

    fn check_animal_spawn_rules(
        level: &dyn LevelReader,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        // Vanilla `Armadillo.checkArmadilloSpawnRules`.
        level
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&BlockTag::ARMADILLO_SPAWNABLE_ON)
            && Self::is_bright_enough_to_spawn(level, pos)
    }
}

impl Mob for ArmadilloEntity {
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
        self.tick_scare_detection();
        self.tick_shell_behavior();
        self.tick_scute_shed();
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        if self.is_scared() {
            None
        } else {
            Some(&sound_events::ENTITY_ARMADILLO_AMBIENT)
        }
    }

    fn max_head_y_rot(&self) -> f32 {
        if self.is_scared() { 0.0 } else { 32.0 }
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
        let item_stack = {
            let inventory = player.inventory.lock();
            inventory.get_item_in_hand(hand).clone()
        };

        if item_stack.is(&vanilla_items::BRUSH) && self.brush_off_scute(&item_stack) {
            player.inventory.lock().hurt_item_in_hand(
                hand,
                BRUSH_DURABILITY_COST,
                player.has_infinite_materials(),
            );
            return InteractionResult::Success;
        }

        if self.is_scared() {
            return InteractionResult::Fail;
        }

        Animal::mob_interact_animal(self, player, hand)
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    fn brain_tick(&self, entity: &SharedEntity) {
        // Vanilla `Armadillo.customServerAiStep` ticks the brain first.
        // GAP(armadillo-brain-provider): the brain has no activities until the
        // generic behaviors (`AnimalPanic`, `AnimalMakeLove`, `FollowTemptation`,
        // `BabyFollowAdult`, `RandomStroll`, `RandomLookAround`,
        // `SetEntityLookTargetSometimes`, `SetWalkTargetFromLookTarget`,
        // `RunOne`, `CountDownCooldownTicks`) and sensors (`FOOD_TEMPTATIONS`,
        // `NEAREST_ADULT`, `ARMADILLO_SCARE_DETECTED`) exist; the scare sensor and
        // `ArmadilloBallUp` run inline in `custom_server_ai_step` instead.
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

impl PathfinderMob for ArmadilloEntity {
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        Animal::animal_walk_target_value(self, pos)
    }
}

/// Vanilla `ArmadilloAi.ArmadilloPanic`: an `AnimalPanic` that rolls out first.
struct ArmadilloPanicGoal {
    inner: PanicGoal,
}

impl ArmadilloPanicGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            inner: PanicGoal::new(speed_modifier),
        }
    }
}

impl Goal for ArmadilloPanicGoal {
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
        if let Some(armadillo) = mob
            .as_entity_event_source()
            .downcast_ref::<ArmadilloEntity>()
        {
            armadillo.roll_out();
        }
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }
}
