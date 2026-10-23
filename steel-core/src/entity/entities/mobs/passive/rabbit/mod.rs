//! Vanilla `Rabbit` (`net.minecraft.world.entity.animal.rabbit.Rabbit`): a hopping
//! animal with biome-driven variants, a carrot-raiding goal, and the hostile
//! "killer bunny" variant.
//!
//! ## Deviations
//!
//! - Vanilla `Rabbit.RabbitGroupData` (one variant shared by a spawn group) has no Steel
//!   counterpart: [`SpawnGroupData`] only carries the ageable-mob payload, so every rabbit
//!   rolls its own variant in [`Mob::finalize_spawn`].
//! - Vanilla `Rabbit.getRandomRabbitVariant` picks white/gold rabbits from the
//!   `spawns_white_rabbits`/`spawns_gold_rabbits` biome tags (no moon phase in this
//!   version); this port mirrors that and seeds a [`LegacyRandom`] in place of the level's
//!   shared RNG. An unloaded chunk has no biome, so it falls through to the
//!   brown/salt/black distribution.
//! - The `RabbitMoveControl.tick` speed-modifier pre-step runs at the end of
//!   [`Mob::custom_server_ai_step`], which is the same point in the vanilla tick order:
//!   `Mob::tick_move_control` exposes no pre-step hook.
//! - `Rabbit.aiStep`'s `jumpTicks`/`jumpDuration` counters are kept because
//!   `startJumping` gates the living jump flag on them; only the client animation states
//!   are missing (see the GAP below).
//! - Vanilla `Rabbit.registerGoals` in this version has **no** `RandomLookAroundGoal` (its
//!   goal 11 is `LookAtPlayerGoal` only), so none is registered here.
//!
//! ## GAPs
//!
//! - GAP(rabbit-client-animation): `Entity` exposes no animation-state or render surface,
//!   so `Rabbit.hopAnimationState`/`idleHeadTiltAnimationState`, `setupAnimationStates`,
//!   `shouldPlayIdleAnimation`, `getJumpCompletion(float)` and `handleEntityEvent(1)`'s
//!   client branch are not ported. The server still broadcasts `EntityStatus::Jump` from
//!   `jumpFromGround` so clients can play the hop.
//! - GAP(rabbit-sprint-particle): `Entity` exposes no `canSpawnSprintParticle` hook;
//!   vanilla `Rabbit.canSpawnSprintParticle()` returns `false`.
//! - GAP(rabbit-leash-offset): `Entity` exposes no `getLeashOffset` hook; vanilla returns
//!   `(0.0, 0.6 * eyeHeight, width * 0.4)`.

use std::iter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::RabbitVariant;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::data_components::vanilla_components::RABBIT_VARIANT;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_biome_tags::BiomeTag;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::RabbitEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, level_events, sound_events, vanilla_attributes, vanilla_blocks,
    vanilla_entities, vanilla_game_events, vanilla_game_rules,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, Downcast as _, DowncastType, DowncastTypeKey, Identifier};
use text_components::TextComponent;
use text_components::translation::TranslatedMessage;

use crate::behavior::InteractionResult;
use crate::entity::ai::control::MoveControlOperation;
use crate::entity::ai::goal::{
    AvoidEntityGoal, BreedGoal, ClimbOnTopOfPowderSnowGoal, FloatGoal, Goal, GoalControls,
    HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal, MoveToBlockGoal,
    NearestAttackableTargetGoal, PanicGoal, TemptGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, AgeableMobGroupData, Animal, AnimalBase, Entity, EntityBase,
    EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, World};

/// Vanilla `Rabbit.STROLL_SPEED_MOD`.
const STROLL_SPEED_MOD: f64 = 0.6;
/// Vanilla `Rabbit.BREED_SPEED_MOD`.
const BREED_SPEED_MOD: f64 = 0.8;
/// Vanilla `Rabbit.FOLLOW_SPEED_MOD`.
const FOLLOW_SPEED_MOD: f64 = 1.0;
/// Vanilla `Rabbit.FLEE_SPEED_MOD`, also the panic goal's speed.
const FLEE_SPEED_MOD: f64 = 2.2;
/// Vanilla `Rabbit.ATTACK_SPEED_MOD`.
const ATTACK_SPEED_MOD: f64 = 1.4;
/// Vanilla `Rabbit.BABY_JUMP_HEIGHT`.
const BABY_JUMP_HEIGHT: f64 = 0.5;
/// Vanilla `Rabbit.ADULT_JUMP_HEIGHT`.
const ADULT_JUMP_HEIGHT: f64 = 1.5;
/// Vanilla `Rabbit.JUMP_DELAY_TICKS`.
const JUMP_DELAY_TICKS: i32 = 10;
/// Vanilla `Rabbit.PANIC_JUMP_DELAY_TICKS`.
const PANIC_JUMP_DELAY_TICKS: i32 = 3;
/// Vanilla `Rabbit.JUMP_DURATION_IN_TICKS`.
const JUMP_DURATION_IN_TICKS: i32 = 15;
/// Vanilla `Rabbit.EVIL_ARMOR_VALUE`.
const EVIL_ARMOR_VALUE: f64 = 8.0;
/// Vanilla `Rabbit.EVIL_ATTACK_POWER_INCREMENT`.
const EVIL_ATTACK_POWER_INCREMENT: f64 = 5.0;
/// Vanilla `Rabbit.MORE_CARROTS_DELAY`.
const MORE_CARROTS_DELAY: i32 = 40;
/// Vanilla `Rabbit.getJumpPower` divides the base power by this before `super.getJumpPower`.
const JUMP_POWER_DIVISOR: f32 = 0.42;
/// Vanilla `Rabbit.getJumpPower`'s default base jump power.
const DEFAULT_JUMP_POWER: f32 = 0.3;
/// Vanilla `Rabbit.getJumpPower`'s base jump power while walking slowly.
const SLOW_JUMP_POWER: f32 = 0.2;
/// Vanilla `Rabbit.getJumpPower`'s raised jump power.
const RAISED_JUMP_POWER: f32 = 0.5;
/// Vanilla `Rabbit.getJumpPower`'s speed-modifier threshold for the slow jump.
const SLOW_JUMP_SPEED_THRESHOLD: f64 = 0.6;
/// Vanilla `Rabbit.jumpFromGround`'s low horizontal speed threshold.
const JUMP_BOOST_MIN_HORIZONTAL_SPEED_SQR: f64 = 0.01;
/// Vanilla `Rabbit.jumpFromGround`'s `moveRelative` speed.
const JUMP_BOOST_SPEED: f32 = 0.1;
/// Vanilla `Rabbit.RabbitMoveControl.setWantedPosition`'s in-water speed override.
const WATER_SPEED_MODIFIER: f64 = 1.5;
/// Vanilla `Rabbit.customServerAiStep`'s squared range for the evil jump attack.
const EVIL_JUMP_ATTACK_RANGE_SQR: f64 = 16.0;
/// Vanilla `Rabbit.RaidGardenGoal`'s movement speed.
const RAID_SPEED_MOD: f64 = 0.7;
/// Vanilla `Rabbit.RaidGardenGoal`'s `super(rabbit, 0.7F, 16)` search range.
const RAID_SEARCH_RANGE: i32 = 16;
/// Vanilla `Rabbit.RaidGardenGoal.tick`'s wait after a raid attempt.
const RAID_RETRY_DELAY_TICKS: i32 = 10;
/// Vanilla `Rabbit.RaidGardenGoal.tick`'s look-control rotation speed.
const RAID_LOOK_SPEED: f32 = 10.0;
/// Vanilla `CarrotBlock`'s maximum age (`CropBlock.getMaxAge`).
const CARROT_MAX_AGE: u8 = 7;
/// Vanilla `Rabbit`'s `entity.minecraft.killer_bunny` description id.
const KILLER_BUNNY_TRANSLATION_KEY: &str = "entity.minecraft.killer_bunny";

/// Vanilla `Rabbit.EVIL_ATTACK_POWER_MODIFIER`.
const EVIL_ATTACK_POWER_MODIFIER: Identifier = Identifier::vanilla_static("evil");

const RABBIT_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.4, 0.0)];
const RABBIT_BABY_WIDTH: f32 = 0.24;
const RABBIT_BABY_HEIGHT: f32 = 0.4;
const RABBIT_BABY_EYE_HEIGHT: f32 = 0.39;

/// Vanilla `Rabbit.BABY_DIMENSIONS`.
const RABBIT_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    RABBIT_BABY_WIDTH,
    RABBIT_BABY_HEIGHT,
    RABBIT_BABY_EYE_HEIGHT,
    EntityAttachments::new(&RABBIT_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);

/// Vanilla `Rabbit` fields that are neither synchronized entity data nor vanilla AI state.
#[derive(Debug, Clone, Copy, Default)]
struct RabbitState {
    /// Vanilla `Rabbit.jumpTicks`: progress through the current hop animation.
    jump_ticks: i32,
    /// Vanilla `Rabbit.jumpDuration`: length of the current hop animation.
    jump_duration: i32,
    /// Vanilla `Rabbit.wasOnGround`: ground state of the previous AI step.
    was_on_ground: bool,
    /// Vanilla `Rabbit.jumpDelayTicks`.
    jump_delay_ticks: i32,
    /// Vanilla `Rabbit.moreCarrotTicks`: carrot cooldown left after raiding a garden.
    more_carrot_ticks: i32,
    /// Vanilla `Rabbit.RabbitMoveControl.nextJumpSpeed`.
    next_jump_speed: f64,
    /// Vanilla `Rabbit.RabbitJumpControl.jump`, mirrored so `customServerAiStep` can read it.
    want_jump: bool,
    /// Vanilla `Rabbit.RabbitJumpControl.canJump`.
    can_jump: bool,
}

#[entity_behavior(class = "Rabbit")]
/// Vanilla rabbit with synced variant, hopping movement and carrot-raiding behaviour.
pub struct RabbitEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<RabbitEntityData>,
    state: SyncMutex<RabbitState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `RabbitEntity`.
unsafe impl DowncastType for RabbitEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/rabbit");
}

impl RabbitEntity {
    /// Creates a new rabbit at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a rabbit from persisted base entity state.
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
        let mut entity_data = RabbitEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.variant_type.set(RabbitVariant::default().id());

        {
            // Vanilla `Rabbit.registerGoals`, same priorities and speeds.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, ClimbOnTopOfPowderSnowGoal::new());
            goal_selector.add_goal(1, RabbitPanicGoal::new(FLEE_SPEED_MOD));
            goal_selector.add_goal(2, BreedGoal::new(BREED_SPEED_MOD));
            goal_selector.add_goal(
                3,
                TemptGoal::new(
                    FOLLOW_SPEED_MOD,
                    |item_stack| {
                        REGISTRY
                            .items
                            .is_in_tag(item_stack.item(), &ItemTag::RABBIT_FOOD)
                    },
                    false,
                ),
            );
            goal_selector.add_goal(
                4,
                RabbitAvoidEntityGoal::new(
                    8.0,
                    FLEE_SPEED_MOD,
                    FLEE_SPEED_MOD,
                    |target, _world| target.as_player().is_some(),
                ),
            );
            goal_selector.add_goal(
                4,
                RabbitAvoidEntityGoal::new(
                    10.0,
                    FLEE_SPEED_MOD,
                    FLEE_SPEED_MOD,
                    |target, _world| target.entity_type() == &vanilla_entities::WOLF,
                ),
            );
            goal_selector.add_goal(
                4,
                RabbitAvoidEntityGoal::new(
                    4.0,
                    FLEE_SPEED_MOD,
                    FLEE_SPEED_MOD,
                    |target, _world| target.as_monster().is_some(),
                ),
            );
            goal_selector.add_goal(5, RaidGardenGoal::new());
            goal_selector.add_goal(6, WaterAvoidingRandomStrollGoal::new(STROLL_SPEED_MOD));
            goal_selector.add_goal(11, LookAtPlayerGoal::new(10.0));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(RabbitState::default()),
        }
    }

    /// Returns the active rabbit variant (vanilla `Rabbit.getVariant`).
    #[must_use]
    pub fn variant(&self) -> RabbitVariant {
        RabbitVariant::by_id(*self.entity_data.lock().variant_type.get())
    }

    /// Sets the active rabbit variant (vanilla `Rabbit.setVariant`).
    ///
    /// The `EVIL` variant also turns the rabbit into a killer bunny: armor, the melee
    /// attack goals, the transient attack-damage modifier and the custom name.
    pub fn set_variant(&self, variant: RabbitVariant) {
        if variant == RabbitVariant::Evil {
            self.attributes()
                .lock()
                .set_base_value(vanilla_attributes::ARMOR, EVIL_ARMOR_VALUE);
            {
                let mut goal_selector = self.mob_base().goal_selector().lock();
                goal_selector.add_goal(4, MeleeAttackGoal::new(ATTACK_SPEED_MOD, true));
            }
            {
                let mut target_selector = self.mob_base().target_selector().lock();
                target_selector
                    .add_goal(1, HurtByTargetGoal::new().set_alert_others(iter::empty()));
                target_selector.add_goal(
                    2,
                    NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
                );
                target_selector.add_goal(
                    2,
                    NearestAttackableTargetGoal::new(true, |target, _world| {
                        target.entity_type() == &vanilla_entities::WOLF
                    }),
                );
            }
            self.attributes().lock().set_modifier(
                vanilla_attributes::ATTACK_DAMAGE,
                AttributeModifier {
                    id: EVIL_ATTACK_POWER_MODIFIER,
                    amount: EVIL_ATTACK_POWER_INCREMENT,
                    operation: AttributeModifierOperation::AddValue,
                },
                false,
            );
            if self.custom_name().is_none() {
                self.set_custom_name(Some(TextComponent::translated(TranslatedMessage::new(
                    KILLER_BUNNY_TRANSLATION_KEY,
                    None,
                ))));
            }
            // A freshly evil rabbit attacks immediately instead of waiting out the hop delay.
            self.state.lock().jump_delay_ticks = 0;
        } else {
            self.attributes().lock().remove_modifier(
                vanilla_attributes::ATTACK_DAMAGE,
                &EVIL_ATTACK_POWER_MODIFIER,
            );
        }

        self.entity_data.lock().variant_type.set(variant.id());
    }

    /// Returns whether an item stack matches the vanilla rabbit food tag.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::RABBIT_FOOD)
    }

    /// Vanilla `Rabbit.checkRabbitSpawnRules`.
    #[must_use]
    pub fn check_rabbit_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut steel_utils::random::RandomSource,
    ) -> bool {
        level
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&BlockTag::RABBITS_SPAWNABLE_ON)
            && <Self as Animal>::is_bright_enough_to_spawn(level, pos)
    }

    /// Vanilla `Rabbit.getRandomRabbitVariant`: snowy biomes spawn white rabbits, deserts
    /// spawn gold rabbits, everything else rolls brown/salt/black.
    fn random_rabbit_variant(world: &World, pos: BlockPos) -> RabbitVariant {
        let biome = world.biome_at(pos);
        let mut random = LegacyRandom::from_seed(rand::random());
        let random_value = random.next_i32_bounded(100);

        if biome.is_some_and(|biome| biome.has_tag(&BiomeTag::SPAWNS_WHITE_RABBITS)) {
            if random_value < 80 {
                RabbitVariant::White
            } else {
                RabbitVariant::WhiteSplotched
            }
        } else if biome.is_some_and(|biome| biome.has_tag(&BiomeTag::SPAWNS_GOLD_RABBITS)) {
            RabbitVariant::Gold
        } else if random_value < 50 {
            RabbitVariant::Brown
        } else if random_value < 90 {
            RabbitVariant::Salt
        } else {
            RabbitVariant::Black
        }
    }

    /// Vanilla `Rabbit.getJumpSound`.
    fn jump_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_RABBIT_JUMP
    }

    /// Vanilla `Rabbit.wantsMoreFood`.
    fn wants_more_food(&self) -> bool {
        self.state.lock().more_carrot_ticks <= 0
    }

    /// Vanilla `Rabbit.RaidGardenGoal`'s `this.rabbit.moreCarrotTicks = 40`.
    fn set_more_carrot_ticks(&self, more_carrot_ticks: i32) {
        self.state.lock().more_carrot_ticks = more_carrot_ticks;
    }

    /// Vanilla `Rabbit.startJumping`.
    fn start_jumping(&self) {
        self.set_jumping(true);
        let mut state = self.state.lock();
        state.jump_duration = JUMP_DURATION_IN_TICKS;
        state.jump_ticks = 0;
    }

    /// Vanilla `Rabbit.setSpeedModifier`: keeps the navigation and move-control speeds in
    /// step, routing through the rabbit's move-control override.
    fn set_speed_modifier(&self, speed: f64) {
        self.mob_base()
            .navigation()
            .lock()
            .set_speed_modifier(speed);
        let wanted_position = self
            .mob_base()
            .controls()
            .lock()
            .move_control
            .wanted_position();
        self.set_wanted_position(wanted_position, speed);
    }

    /// Vanilla `Rabbit.facePoint`: turns the body (yaw only) toward a point.
    fn face_point(&self, face_x: f64, face_z: f64) {
        let position = self.position();
        let yaw = (face_z - position.z)
            .atan2(face_x - position.x)
            .to_degrees() as f32
            - 90.0;
        let (_, pitch) = self.rotation();
        self.set_rotation((yaw, pitch));
    }

    /// Vanilla `Rabbit.RabbitMoveControl.setLandingDelay`.
    fn set_landing_delay(&self) {
        let speed_modifier = self
            .mob_base()
            .controls()
            .lock()
            .move_control
            .speed_modifier();
        self.state.lock().jump_delay_ticks = if speed_modifier < FLEE_SPEED_MOD {
            JUMP_DELAY_TICKS
        } else {
            PANIC_JUMP_DELAY_TICKS
        };
    }

    /// Vanilla `Rabbit.checkLandingDelay`.
    fn check_landing_delay(&self) {
        self.set_landing_delay();
        self.state.lock().can_jump = false;
    }

    /// Vanilla `MoveControl.hasWanted`.
    fn move_control_has_wanted(&self) -> bool {
        self.mob_base().controls().lock().move_control.operation() == MoveControlOperation::MoveTo
    }

    /// Vanilla `Path.getNextEntityPos`: the center of the path's next node.
    fn next_path_node_entity_pos(&self) -> Option<DVec3> {
        let navigation = self.mob_base().navigation().lock();
        let path = navigation.path()?;
        if path.is_done() {
            return None;
        }

        let node = path.next_node_pos()?;
        let half = f64::from((self.bounding_box().width() + 1.0) as i32) * 0.5;
        Some(DVec3::new(
            f64::from(node.x()) + half,
            f64::from(node.y()),
            f64::from(node.z()) + half,
        ))
    }

    /// Vanilla `Rabbit.aiStep`'s hop-animation counters.
    fn tick_jump_animation(&self) {
        let mut state = self.state.lock();
        if state.jump_ticks != state.jump_duration {
            state.jump_ticks += 1;
        } else if state.jump_duration != 0 {
            state.jump_ticks = 0;
            state.jump_duration = 0;
            drop(state);
            self.set_jumping(false);
        }
    }

    /// Vanilla `Rabbit.RabbitMoveControl.tick`'s speed-modifier pre-step.
    fn tick_rabbit_move_control(&self) {
        let operation = self.mob_base().controls().lock().move_control.operation();
        let want_jump = self.state.lock().want_jump;

        if self.on_ground() && !self.is_jumping() && !want_jump {
            self.set_speed_modifier(0.0);
        } else if operation == MoveControlOperation::MoveTo
            || operation == MoveControlOperation::Jumping
        {
            let next_jump_speed = self.state.lock().next_jump_speed;
            self.set_speed_modifier(next_jump_speed);
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

        // Sync base entity flags from resolved effect display state in one place.
        self.entity_data.set_base_invisible_flag(display.invisible);
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }
}

/// Vanilla `Rabbit.RabbitPanicGoal`: a panic goal that re-applies the rabbit's speed
/// modifier every tick so the hop movement keeps up.
struct RabbitPanicGoal {
    panic_goal: PanicGoal,
    speed_modifier: f64,
}

impl RabbitPanicGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            panic_goal: PanicGoal::new(speed_modifier),
            speed_modifier,
        }
    }
}

impl Goal for RabbitPanicGoal {
    fn controls(&self) -> GoalControls {
        self.panic_goal.controls()
    }

    fn is_panic_goal(&self) -> bool {
        self.panic_goal.is_panic_goal()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.panic_goal.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.panic_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.panic_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.panic_goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.panic_goal.tick(mob, entity);
        if let Some(rabbit) = mob.downcast_ref::<RabbitEntity>() {
            rabbit.set_speed_modifier(self.speed_modifier);
        }
    }
}

/// Vanilla `Rabbit.RabbitAvoidEntityGoal`: avoids its target unless the rabbit is evil.
struct RabbitAvoidEntityGoal {
    avoid_goal: AvoidEntityGoal,
}

impl RabbitAvoidEntityGoal {
    fn new(
        max_dist: f32,
        walk_speed_modifier: f64,
        sprint_speed_modifier: f64,
        selector: impl Fn(&dyn LivingEntity, &World) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            avoid_goal: AvoidEntityGoal::with_selector(
                max_dist,
                walk_speed_modifier,
                sprint_speed_modifier,
                selector,
            ),
        }
    }
}

impl Goal for RabbitAvoidEntityGoal {
    fn controls(&self) -> GoalControls {
        self.avoid_goal.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let is_evil = mob
            .downcast_ref::<RabbitEntity>()
            .is_some_and(|rabbit| rabbit.variant() == RabbitVariant::Evil);
        !is_evil && self.avoid_goal.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.avoid_goal.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.avoid_goal.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.avoid_goal.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.avoid_goal.tick(mob, entity);
    }
}

/// Vanilla `Rabbit.RaidGardenGoal`: walks to a carrot crop and eats one age stage.
struct RaidGardenGoal {
    move_to_block: MoveToBlockGoal,
    wants_to_raid: Arc<AtomicBool>,
    can_raid: Arc<AtomicBool>,
}

impl RaidGardenGoal {
    fn new() -> Self {
        let wants_to_raid = Arc::new(AtomicBool::new(false));
        let can_raid = Arc::new(AtomicBool::new(false));
        let predicate_wants_to_raid = Arc::clone(&wants_to_raid);
        let predicate_can_raid = Arc::clone(&can_raid);

        Self {
            move_to_block: MoveToBlockGoal::new(
                RAID_SPEED_MOD,
                RAID_SEARCH_RANGE,
                move |level, pos| {
                    is_valid_raid_target(level, pos, &predicate_wants_to_raid, &predicate_can_raid)
                },
            ),
            wants_to_raid,
            can_raid,
        }
    }

    /// Vanilla `RaidGardenGoal.tick`'s reached-target branch.
    fn raid_crop(&mut self, rabbit: &RabbitEntity, world: &Arc<World>) {
        let crops_pos = self.move_to_block.block_pos().above();
        let block_state = world.get_block_state(crops_pos);

        if self.can_raid.load(Ordering::Relaxed)
            && block_state.get_block() == &vanilla_blocks::CARROTS
        {
            let carrot_age = block_state.get_value(&BlockStateProperties::AGE_7);
            if carrot_age == 0 {
                world.set_block_state(
                    crops_pos,
                    vanilla_blocks::AIR.default_state(),
                    UpdateFlags::UPDATE_CLIENTS,
                );
                world.destroy_block(crops_pos, true);
            } else {
                world.set_block_state(
                    crops_pos,
                    block_state.set_value(&BlockStateProperties::AGE_7, carrot_age - 1),
                    UpdateFlags::UPDATE_CLIENTS,
                );
                world.game_event(
                    &vanilla_game_events::BLOCK_CHANGE,
                    crops_pos,
                    &GameEventContext::new(Some(rabbit), None),
                );
                world.level_event(
                    level_events::PARTICLES_AND_SOUND_DESTROY_BLOCK,
                    crops_pos,
                    level_events::encode_block_state_data(u32::from(block_state.0)),
                    None,
                );
            }

            rabbit.set_more_carrot_ticks(MORE_CARROTS_DELAY);
        }

        self.can_raid.store(false, Ordering::Relaxed);
        self.move_to_block
            .set_next_start_tick(RAID_RETRY_DELAY_TICKS);
    }
}

/// Vanilla `RaidGardenGoal.isValidTarget`.
fn is_valid_raid_target(
    level: &dyn LevelReader,
    pos: BlockPos,
    wants_to_raid: &AtomicBool,
    can_raid: &AtomicBool,
) -> bool {
    let state = level.get_block_state(pos);
    if !state.get_block().has_tag(&BlockTag::SUPPORTS_CROPS)
        || !wants_to_raid.load(Ordering::Relaxed)
        || can_raid.load(Ordering::Relaxed)
    {
        return false;
    }

    let crops = level.get_block_state(pos.above());
    if crops.get_block() == &vanilla_blocks::CARROTS
        && crops.get_value(&BlockStateProperties::AGE_7) >= CARROT_MAX_AGE
    {
        can_raid.store(true, Ordering::Relaxed);
        return true;
    }

    false
}

impl Goal for RaidGardenGoal {
    fn controls(&self) -> GoalControls {
        self.move_to_block.controls()
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to_block.requires_update_every_tick()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.move_to_block.next_start_tick() <= 0 {
            let Some(world) = mob.level() else {
                return false;
            };
            if !world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING) {
                return false;
            }

            self.can_raid.store(false, Ordering::Relaxed);
            self.wants_to_raid.store(
                mob.downcast_ref::<RabbitEntity>()
                    .is_some_and(RabbitEntity::wants_more_food),
                Ordering::Relaxed,
            );
        }

        self.move_to_block.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_raid.load(Ordering::Relaxed) && self.move_to_block.can_continue_to_use(mob)
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to_block.tick(mob, entity);

        let block_pos = self.move_to_block.block_pos();
        let look_target = DVec3::new(
            f64::from(block_pos.x()) + 0.5,
            f64::from(block_pos.y()) + 1.0,
            f64::from(block_pos.z()) + 0.5,
        );
        let max_head_x_rot = mob.max_head_x_rot();
        mob.mob_base().controls().lock().look_control.set_look_at(
            look_target,
            RAID_LOOK_SPEED,
            max_head_x_rot,
        );

        if !self.move_to_block.is_reached_target() {
            return;
        }

        let Some(rabbit) = mob.downcast_ref::<RabbitEntity>() else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };
        self.raid_crop(rabbit, &world);
    }
}

impl Entity for RabbitEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(variant) = item_stack.get(RABBIT_VARIANT) {
            self.set_variant(*variant);
        }
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            RABBIT_BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
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
        if self.variant() == RabbitVariant::Evil {
            SoundSource::Hostile
        } else {
            SoundSource::Neutral
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("RabbitType", self.variant().id());
        nbt.insert("MoreCarrotTicks", self.state.lock().more_carrot_ticks);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        // Vanilla reads the legacy int codec, where unknown ids fall back to `DEFAULT`.
        self.set_variant(RabbitVariant::by_id(
            nbt.int("RabbitType")
                .unwrap_or_else(|| RabbitVariant::default().id()),
        ));
        self.state.lock().more_carrot_ticks = nbt.int("MoreCarrotTicks").unwrap_or(0);
    }
}

impl LivingEntity for RabbitEntity {
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

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_RABBIT_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_RABBIT_DEATH)
    }

    /// Vanilla `Rabbit.getJumpPower`: hop power grows with the move-control speed and
    /// whenever the rabbit has to climb.
    fn get_jump_power(&self) -> f32 {
        let speed_modifier = self
            .mob_base()
            .controls()
            .lock()
            .move_control
            .speed_modifier();
        let mut base_jump_power = if speed_modifier <= SLOW_JUMP_SPEED_THRESHOLD {
            SLOW_JUMP_POWER
        } else {
            DEFAULT_JUMP_POWER
        };

        if self
            .next_path_node_entity_pos()
            .is_some_and(|next| next.y > self.position().y + 0.5)
        {
            base_jump_power = RAISED_JUMP_POWER;
        }

        let wanted_y = self
            .mob_base()
            .controls()
            .lock()
            .move_control
            .wanted_position()
            .y;
        if self.horizontal_collision() || self.is_jumping() && wanted_y > self.position().y + 0.5 {
            base_jump_power = RAISED_JUMP_POWER;
        }

        self.get_jump_power_with_multiplier(base_jump_power / JUMP_POWER_DIVISOR)
    }

    /// Vanilla `Rabbit.jumpFromGround`: adds the hop's forward push and broadcasts the
    /// jump animation to nearby clients.
    fn jump_from_ground(&self) {
        self.default_jump_from_ground();

        let speed_modifier = self
            .mob_base()
            .controls()
            .lock()
            .move_control
            .speed_modifier();
        if speed_modifier > 0.0 {
            let velocity = self.velocity();
            if velocity.x.mul_add(velocity.x, velocity.z * velocity.z)
                < JUMP_BOOST_MIN_HORIZONTAL_SPEED_SQR
            {
                let jump_height = if AgeableMob::is_baby(self) {
                    BABY_JUMP_HEIGHT
                } else {
                    ADULT_JUMP_HEIGHT
                };
                self.move_relative(JUMP_BOOST_SPEED, DVec3::new(0.0, jump_height, 1.0));
            }
        }

        self.broadcast_entity_event(EntityStatus::Jump);
    }

    /// Vanilla `Rabbit.setJumping`: every hop start plays the rabbit jump sound.
    fn set_jumping(&self, jumping: bool) {
        self.living_base.set_jumping(jumping);
        if jumping {
            let pitch = (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0;
            self.play_sound(self.jump_sound(), self.sound_volume(), pitch * 0.8);
        }
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        self.tick_jump_animation();
        result
    }
}

impl AgeableMob for RabbitEntity {
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

    /// Vanilla `Rabbit.getBreedOffspring`: a fresh biome roll, unless the 1-in-20 mutation
    /// roll fails, in which case one parent's variant is inherited.
    fn initialize_breed_offspring(&self, partner: &dyn AgeableMob, offspring: &dyn AgeableMob) {
        let Some(offspring) = offspring.downcast_ref::<RabbitEntity>() else {
            return;
        };

        let mut variant = self.level().map_or_else(
            || self.variant(),
            |world| Self::random_rabbit_variant(&world, self.block_position()),
        );
        if rand::random_range(0..20) != 0 {
            variant = match partner.downcast_ref::<RabbitEntity>() {
                Some(partner_rabbit) if rand::random::<bool>() => partner_rabbit.variant(),
                _ => self.variant(),
            };
        }

        offspring.set_variant(variant);
    }
}

impl Animal for RabbitEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        RabbitEntity::is_food(item_stack)
    }
}

impl Mob for RabbitEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Rabbit.customServerAiStep`.
    ///
    /// Vanilla `Animal.customServerAiStep`'s love reset is skipped here exactly like
    /// vanilla (`Rabbit` does not call `super`); `Animal::tick_animal_love` still covers it
    /// from `aiStep`.
    fn custom_server_ai_step(&self) {
        {
            let mut state = self.state.lock();
            if state.jump_delay_ticks > 0 {
                state.jump_delay_ticks -= 1;
            }
            if state.more_carrot_ticks > 0 {
                state.more_carrot_ticks =
                    (state.more_carrot_ticks - rand::random_range(0..3)).max(0);
            }
        }

        if self.on_ground() {
            // Vanilla re-reads `jumpDelayTicks` after `checkLandingDelay`, so the freshly
            // landed tick cannot start another hop.
            if !self.state.lock().was_on_ground {
                self.set_jumping(false);
                self.check_landing_delay();
            }

            let jump_delay_ticks = self.state.lock().jump_delay_ticks;
            if self.variant() == RabbitVariant::Evil
                && jump_delay_ticks == 0
                && let Some(target) = self.target()
            {
                let target_position = target.position();
                if self.distance_to_sqr(target_position) < EVIL_JUMP_ATTACK_RANGE_SQR {
                    self.face_point(target_position.x, target_position.z);
                    let speed_modifier = self
                        .mob_base()
                        .controls()
                        .lock()
                        .move_control
                        .speed_modifier();
                    self.set_wanted_position(target_position, speed_modifier);
                    self.start_jumping();
                    self.state.lock().was_on_ground = true;
                }
            }

            let (want_jump, can_jump) = {
                let state = self.state.lock();
                (state.want_jump, state.can_jump)
            };
            if want_jump {
                if !can_jump {
                    self.state.lock().can_jump = true;
                }
            } else if self.move_control_has_wanted() && jump_delay_ticks == 0 {
                let wanted_position = self
                    .mob_base()
                    .controls()
                    .lock()
                    .move_control
                    .wanted_position();
                let face_position = self.next_path_node_entity_pos().unwrap_or(wanted_position);
                self.face_point(face_position.x, face_position.z);
                self.start_jumping();
            }
        }

        self.state.lock().was_on_ground = self.on_ground();

        // See the module docs: this is the `RabbitMoveControl.tick` pre-step, in vanilla's
        // position right before `Mob::tick_move_control`.
        self.tick_rabbit_move_control();
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_RABBIT_AMBIENT)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla shares the variant through `RabbitGroupData`; Steel's `SpawnGroupData`
        // has no such payload, so each rabbit rolls its own.
        let variant = Self::random_rabbit_variant(world, self.block_position());
        self.set_variant(variant);
        // Vanilla `new RabbitGroupData(variant)` passes `super(1.0F)`, so every rabbit
        // after the group's first spawns as a baby.
        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::AgeableMob(
                AgeableMobGroupData::with_baby_spawn_chance(1.0),
            ))
        });
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        Animal::mob_interact_animal(self, player, hand)
    }

    /// Vanilla `Rabbit.playAttackSound`: only killer bunnies vocalize their attack.
    fn play_attack_sound(&self) {
        if self.variant() == RabbitVariant::Evil {
            let pitch = (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0;
            self.play_sound(&sound_events::ENTITY_RABBIT_ATTACK, 1.0, pitch);
        }
    }

    /// Vanilla `Rabbit.RabbitMoveControl.setWantedPosition`: hops faster in water and
    /// remembers the last real speed for the hop.
    ///
    /// `Mob::set_wanted_position`'s controlled-vehicle forwarding is not repeated: a rabbit
    /// can never be a controlled mob vehicle.
    fn set_wanted_position(&self, position: DVec3, speed_modifier: f64) {
        let speed_modifier = if self.is_in_water() {
            WATER_SPEED_MODIFIER
        } else {
            speed_modifier
        };

        self.mob_base()
            .controls()
            .lock()
            .move_control
            .set_wanted_position(position, speed_modifier);
        if speed_modifier > 0.0 {
            self.state.lock().next_jump_speed = speed_modifier;
        }
    }

    /// Vanilla `JumpControl.jump`, mirrored so `customServerAiStep` can read the pending
    /// jump (vanilla `RabbitJumpControl.wantJump`).
    fn jump_control_jump(&self) {
        self.mob_base().controls().lock().jump_control.jump();
        self.state.lock().want_jump = true;
    }

    /// Vanilla `Rabbit.RabbitJumpControl.tick`: unlike `JumpControl`, a pending jump starts
    /// the rabbit hop and an idle jump control leaves the living jump flag untouched.
    fn tick_jump_control(&self) {
        let jump = self.mob_base().controls().lock().jump_control.tick();
        self.state.lock().want_jump = false;
        if jump {
            self.start_jumping();
        }
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for RabbitEntity {}

#[cfg(test)]
mod tests;
