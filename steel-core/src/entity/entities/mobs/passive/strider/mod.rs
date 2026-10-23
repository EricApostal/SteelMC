//! Vanilla `Strider` (`net.minecraft.world.entity.monster.Strider`).
//!
//! Vanilla keeps `Strider` in the `monster` package even though the class extends
//! `Animal`; Steel mirrors the vanilla hierarchy, so this module implements
//! [`Animal`] (never [`crate::entity::Monster`]) and lives under `mobs::passive`.
//!
//! The strider walks on lava: it stands on the lava surface
//! (`can_stand_on_fluid`), floats via its half-height liquid collision shape,
//! takes no fire damage, is steered with a warped fungus on a stick and
//! suffocates (losing movement speed) outside warm blocks.
//!
//! ## Deviations / GAPs
//!
//! - GAP(strider-client-sync): vanilla `Strider.onSyncedDataUpdated` only feeds the
//!   client-side steering ramp (`steering.onSynced` on `DATA_BOOST_TIME`); the
//!   server half of that sync arrives from the item-steering trait, so no hook is
//!   needed here.
//! - GAP(strider-path-navigation): vanilla installs `StriderPathNavigation`, whose
//!   `isStableDestination` also accepts lava columns. Steel's navigation kind is
//!   fixed per entity, so the stable-destination override lives on
//!   [`PathfinderMob::is_stable_destination`] instead (same observable result for
//!   the goal/`RandomPos` callers that use it).
//! - GAP(strider-dismount-location): vanilla `Strider.getDismountLocationForPassenger`
//!   needs `Entity.getCollisionHorizontalEscapeVector` plus
//!   `DismountHelper.isBlockFloorValid`/`Level.getBlockFloorHeight`. Steel exposes
//!   neither, and `LivingEntity` has no dismount-location hook, so
//!   [`StriderEntity::get_dismount_location_for_passenger`] is implemented on the
//!   available [`crate::entity::dismount_helper`] primitives and awaits a trait hook.
//! - GAP(strider-jockey-ordering): Steel's spawner calls `finalize_spawn` *before*
//!   registering the mob, and `Entity::start_riding` resolves both ends through the
//!   world, so a jockey can only be mounted when the strider is already registered
//!   (chunk-generation/`/summon` paths). Otherwise no jockey is created at all,
//!   which keeps a stray unridden jockey mob out of the world.
//! - GAP(strider-zombified-piglin-group-data): vanilla passes
//!   `Zombie.ZombieGroupData(getSpawnAsBabyOdds(random), false)` into the jockey's
//!   `finalizeSpawn`. Steel's [`ZombifiedPiglinEntity`] has no `finalize_spawn`
//!   group-data support, so the baby roll is applied directly.
//! - GAP(strider-water-sensitivity): vanilla `Strider.isSensitiveToWater` has no
//!   `LivingEntity` capability; thrown water potions resolve it by entity type in
//!   [`crate::entity::entities::objects::projectiles::thrown_potion`], which already
//!   lists `minecraft:strider`. [`StriderEntity::is_sensitive_to_water`] mirrors the
//!   override for the same reason [`crate::entity::entities::mobs::golem::SnowGolemEntity`]
//!   does.
//! - GAP(strider-spawn-placement): [`StriderEntity::check_strider_spawn_rules`] is
//!   complete but `world::spawn::placement` has no `minecraft:strider` entry yet, so
//!   natural strider spawning is not wired.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::shapes::VoxelShape;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::fluid::FluidState;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::StriderEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes, vanilla_blocks,
    vanilla_entities, vanilla_game_events, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{
    BlockLocalAabb, BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey,
    Identifier,
};

use crate::behavior::{
    BLOCK_BEHAVIORS, BlockCollisionContext, BlockStateBehaviorExt as _, EntityFallOnContext,
    InteractionResult,
};
use crate::entity::ai::goal::{
    BreedGoal, FollowParentGoal, Goal, GoalControls, LookAtPlayerGoal, MoveToBlockGoal, PanicGoal,
    RandomLookAroundGoal, RandomStrollGoal, TemptGoal,
};
use crate::entity::ai::path::{PathComputationType, PathType};
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::hostile::piglin::ZombifiedPiglinEntity;
use crate::entity::entities::mobs::zombie::ZombieEntity;
use crate::entity::registry::ENTITIES;
use crate::entity::{
    AgeableMob, AgeableMobBase, AgeableMobGroupData, Animal, AnimalBase, Entity, EntityBase,
    EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData, ItemBasedSteering,
    ItemSteerable, LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity,
    SpawnGroupData, next_entity_id,
};
use crate::fluid::FluidStateExt as _;
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{LevelReader, World};

/// Vanilla `Strider.SUFFOCATING_MODIFIER_ID`.
const SUFFOCATING_MODIFIER_ID: Identifier = Identifier::vanilla_static("suffocating");
/// Vanilla `Strider.SUFFOCATING_MODIFIER`: `-0.34` multiplied into the base value.
const SUFFOCATING_MODIFIER: AttributeModifier = AttributeModifier {
    id: SUFFOCATING_MODIFIER_ID,
    amount: -0.34,
    operation: AttributeModifierOperation::AddMultipliedBase,
};
/// Vanilla `Strider.SUFFOCATE_STEERING_MODIFIER`.
const SUFFOCATE_STEERING_MODIFIER: f32 = 0.35;
/// Vanilla `Strider.STEERING_MODIFIER`.
const STEERING_MODIFIER: f32 = 0.55;
/// Vanilla `Strider.nextStep`: the strider's longer stride offset.
const NEXT_STEP_OFFSET: f32 = 0.6;

/// Vanilla `Strider.getLiquidCollisionShape` (`Block.column(16.0, 0.0, 8.0)`).
const STRIDER_LIQUID_COLLISION_SHAPE: VoxelShape =
    VoxelShape::from_boxes(&[BlockLocalAabb::new(0.0, 0.0, 0.0, 1.0, 0.5, 1.0)]);

/// Vanilla `Strider.BABY_DIMENSIONS` passenger attachment (`0.0, 0.65625, 0.0`).
const STRIDER_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.656_25, 0.0)];
/// Vanilla `Strider.BABY_DIMENSIONS`.
const STRIDER_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    0.45,
    0.85,
    0.4375,
    EntityAttachments::new(&STRIDER_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);

/// Vanilla `new StriderGoToLavaGoal(this, 1.0)` → `MoveToBlockGoal(mob, speed, 8, 2)`.
const LAVA_GOAL_SEARCH_RANGE: i32 = 8;
/// Vanilla `StriderGoToLavaGoal`'s `MoveToBlockGoal` vertical search range.
const LAVA_GOAL_VERTICAL_SEARCH_RANGE: i32 = 2;
/// Vanilla `Strider.StriderGoToLavaGoal.shouldRecalculatePath`: `tryTicks % 20 == 0`.
const LAVA_GOAL_RECALCULATE_PATH_INTERVAL: i32 = 20;

/// Vanilla `new RandomStrollGoal(this, 1.0, 60)`.
const STROLL_INTERVAL: i32 = 60;
/// Vanilla `Strider.tick`: `random.nextInt(140) == 0` while tempted.
const HAPPY_SOUND_INTERVAL: i32 = 140;
/// Vanilla `Strider.tick`: `random.nextInt(60) == 0` while panicking.
const RETREAT_SOUND_INTERVAL: i32 = 60;

/// Vanilla `Strider.finalizeSpawn`: `random.nextInt(30) == 0` zombified-piglin jockey.
const ZOMBIFIED_PIGLIN_JOCKEY_CHANCE: i32 = 30;
/// Vanilla `Strider.finalizeSpawn`: `random.nextInt(10) == 0` baby-strider jockey.
const STRIDER_JOCKEY_CHANCE: i32 = 10;
/// Vanilla `Strider.finalizeSpawn` fallback `new AgeableMobGroupData(0.5F)`.
const JOCKEY_GROUP_BABY_SPAWN_CHANCE: f32 = 0.5;
/// Vanilla `Strider.spawnJockey` returns `new AgeableMobGroupData(0.0F)`.
const SPAWNED_JOCKEY_GROUP_BABY_SPAWN_CHANCE: f32 = 0.0;
/// Vanilla `Strider.finalizeSpawn`: `jockey.setAge(-24000)`.
const BABY_STRIDER_JOCKEY_AGE: i32 = -24_000;
/// Vanilla `Strider.registerGoals`: `new LookAtPlayerGoal(this, Strider.class, 8.0F)`.
const LOOK_AT_STRIDER_PROBABILITY: f32 = 0.02;
/// Vanilla `Strider.registerGoals` look distances.
const LOOK_AT_DISTANCE: f64 = 8.0;

/// Vanilla `LivingEntity.getAttributeValue(Attributes.STEP_HEIGHT)` fallback.
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `Strider` synchronized data plus item-steering state.
#[entity_behavior(class = "Strider")]
pub struct StriderEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    steering: SyncMutex<ItemBasedSteering>,
    entity_data: SyncMutex<StriderEntityData>,
    /// Mirrors vanilla `Strider.temptGoal.isRunning()` for the happy sound.
    tempt_running: Arc<AtomicBool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `StriderEntity`.
unsafe impl DowncastType for StriderEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/strider");
}

impl StriderEntity {
    /// Creates a new strider at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a strider from persisted base entity state.
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
        // Vanilla `Animal` defaults, then `Strider`'s own malus table.
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        {
            let mut malus = mob_base.pathfinding_malus().lock();
            // Vanilla `Strider` constructor.
            malus.set(PathType::Water, -1.0);
            malus.set(PathType::Lava, 0.0);
            malus.set(PathType::FireInNeighbor, 0.0);
            malus.set(PathType::Fire, 0.0);
        }

        let tempt_running = Arc::new(AtomicBool::new(false));
        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, PanicGoal::new(1.65));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, StriderTemptGoal::new(1.4, Arc::clone(&tempt_running)));
            goal_selector.add_goal(4, StriderGoToLavaGoal::new(1.0));
            goal_selector.add_goal(5, FollowParentGoal::new(1.0));
            goal_selector.add_goal(7, RandomStrollGoal::with_interval(1.0, STROLL_INTERVAL));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(LOOK_AT_DISTANCE));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());
            goal_selector.add_goal(
                9,
                LookAtPlayerGoal::new_for_living_entities(
                    LOOK_AT_DISTANCE,
                    LOOK_AT_STRIDER_PROBABILITY,
                    |entity, _world| entity.entity_type() == &vanilla_entities::STRIDER,
                ),
            );
        }

        let steering = SyncMutex::new(ItemBasedSteering::new());
        let mut entity_data = StriderEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            steering,
            entity_data: SyncMutex::new(entity_data),
            tempt_running,
        }
    }

    /// Vanilla `Strider.setSuffocating`: syncs the flag and adds/removes the
    /// transient movement-speed penalty.
    pub fn set_suffocating(&self, flag: bool) {
        self.entity_data.lock().suffocating.set(flag);
        if flag {
            self.attributes().lock().set_modifier(
                vanilla_attributes::MOVEMENT_SPEED,
                SUFFOCATING_MODIFIER,
                false,
            );
        } else {
            self.attributes()
                .lock()
                .remove_modifier(vanilla_attributes::MOVEMENT_SPEED, &SUFFOCATING_MODIFIER_ID);
        }
    }

    /// Vanilla `Strider.isSuffocating`.
    #[must_use]
    pub fn is_suffocating(&self) -> bool {
        *self.entity_data.lock().suffocating.get()
    }

    /// Returns whether the stack is vanilla strider food (`minecraft:strider_food`).
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::STRIDER_FOOD)
    }

    /// Vanilla `Strider.isSensitiveToWater`; see the module GAP note for why this
    /// is an inherent method rather than a `LivingEntity` hook.
    #[must_use]
    pub const fn is_sensitive_to_water(&self) -> bool {
        true
    }

    /// Vanilla `Strider.isBeingTempted`.
    #[must_use]
    pub fn is_being_tempted(&self) -> bool {
        self.tempt_running.load(Ordering::Relaxed)
    }

    /// Vanilla `Strider.tick`: happy/retreat sounds, then the suffocation state.
    fn tick_strider_state(&self) {
        if self.is_being_tempted() && rand::random_range(0..HAPPY_SOUND_INTERVAL) == 0 {
            self.make_sound(Some(&sound_events::ENTITY_STRIDER_HAPPY));
        } else if PathfinderMob::is_panicking(self)
            && rand::random_range(0..RETREAT_SOUND_INTERVAL) == 0
        {
            self.make_sound(Some(&sound_events::ENTITY_STRIDER_RETREAT));
        }

        if Mob::is_no_ai(self) {
            return;
        }

        let Some(world) = self.level() else {
            return;
        };

        let state_inside = world.get_block_state(self.block_position());
        let state_on = self
            .on_pos_legacy()
            .map(|pos| world.get_block_state(pos))
            .unwrap_or(state_inside);
        let in_warm_blocks = state_inside
            .get_block()
            .has_tag(&BlockTag::STRIDER_WARM_BLOCKS)
            || state_on.get_block().has_tag(&BlockTag::STRIDER_WARM_BLOCKS)
            || self.fluid_contact().lava_height() > 0.0;
        // Vanilla `this.getVehicle() instanceof Strider strider && !strider.isSuffocating()`.
        let on_warm_strider = self.vehicle().is_some_and(|vehicle| {
            vehicle
                .downcast_ref::<StriderEntity>()
                .is_some_and(|strider| !strider.is_suffocating())
        });

        self.set_suffocating(!in_warm_blocks && !on_warm_strider);
    }

    /// Vanilla `Strider.floatStrider`: buoyancy while inside lava.
    fn float_strider(&self) {
        if !self.is_in_lava() {
            return;
        }

        let collision_context =
            BlockCollisionContext::entity(self.position().y, self.is_descending());
        let Some(world) = self.level() else {
            return;
        };

        let block_pos = self.block_position();
        if collision_context.is_above(STRIDER_LIQUID_COLLISION_SHAPE, block_pos, true)
            && !world
                .get_block_state(block_pos.above())
                .get_fluid_state()
                .is_lava()
        {
            self.set_on_ground(true);
        } else {
            self.set_velocity(self.velocity() * 0.5 + DVec3::new(0.0, 0.05, 0.0));
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
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }

    /// Vanilla `Strider.spawnJockey`: positions, finalizes and mounts `jockey`,
    /// returning the vanilla jockey group data.
    ///
    /// See the module GAP note: mounting needs both entities registered, so the
    /// jockey is dropped before it is added when the strider is not in the world.
    fn spawn_jockey(
        &self,
        world: &Arc<World>,
        jockey: &SharedEntity,
        jockey_group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let Some(vehicle) = world.get_entity_by_id(self.id()) else {
            return None;
        };
        let Some(jockey_mob) = jockey.as_mob() else {
            return None;
        };

        jockey_mob.snap_to(self.position(), self.rotation().0, 0.0);
        jockey_mob.finalize_spawn(world, EntitySpawnReason::Jockey, jockey_group_data);
        if let Err(error) = world.try_add_entity(Arc::clone(jockey)) {
            log::warn!("failed to add strider jockey: {error}");
            return None;
        }

        jockey_mob.start_riding(&vehicle);
        Some(SpawnGroupData::AgeableMob(
            AgeableMobGroupData::with_baby_spawn_chance(SPAWNED_JOCKEY_GROUP_BABY_SPAWN_CHANCE),
        ))
    }

    /// Vanilla `Strider.getDismountLocationForPassenger`.
    ///
    /// Uses the shared [`crate::entity::dismount_helper`] primitives for the floor
    /// validity and collision checks; see the module GAP note.
    #[must_use]
    pub fn get_dismount_location_for_passenger(&self, passenger: &dyn Entity) -> DVec3 {
        let fallback = DVec3::new(
            self.position().x,
            self.bounding_box().max_y(),
            self.position().z,
        );
        let Some(world) = self.level() else {
            return fallback;
        };

        let bounding_box = self.bounding_box();
        let collider_width = bounding_box.max_x() - bounding_box.min_x();
        let passenger_box = passenger.bounding_box();
        let passenger_width = passenger_box.max_x() - passenger_box.min_x();
        let yaw = self.rotation().0;

        // Vanilla `GetCollisionHorizontalEscapeVector` fan of five headings.
        let directions = [0.0_f32, -22.5, 22.5, -45.0, 45.0].map(|offset| {
            collision_horizontal_escape_vector(collider_width, passenger_width, yaw + offset)
        });

        let collider_top = bounding_box.max_y();
        let collider_bottom = bounding_box.min_y() - 0.5;
        let mut target_positions: Vec<BlockPos> = Vec::new();
        for direction in directions {
            let x = (self.position().x + direction.x).floor() as i32;
            let z = (self.position().z + direction.z).floor() as i32;

            let mut y = collider_top;
            while y > collider_bottom {
                target_positions.push(BlockPos::new(x, y.floor() as i32, z));
                y -= 1.0;
            }
        }

        for target_pos in target_positions {
            if world
                .get_block_state(target_pos)
                .get_fluid_state()
                .is_lava()
            {
                continue;
            }

            if let Some(location) = crate::entity::dismount_helper::find_safe_dismount_location(
                &world, passenger, target_pos, false,
            ) {
                return location;
            }
        }

        fallback
    }

    fn set_ridden_rotation(&self, controller_yaw: f32, controller_pitch: f32) {
        self.set_rotation((controller_yaw, controller_pitch * 0.5));
        self.base.set_old_yaw_to_current();
        let yaw = self.rotation().0;
        self.set_y_body_rot(yaw);
        self.set_y_head_rot(yaw);
    }

    /// Vanilla `Strider.checkStriderSpawnRules`.
    ///
    /// See the module GAP note: the rule is complete but not registered in the
    /// spawn-placement table yet.
    #[must_use]
    pub fn check_strider_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut impl steel_utils::random::Random,
    ) -> bool {
        let mut check_pos = pos;
        loop {
            check_pos = check_pos.above();
            if !level.get_block_state(check_pos).get_fluid_state().is_lava() {
                break;
            }
        }

        level.get_block_state(check_pos).is_air()
    }
}

/// Vanilla `Entity.getCollisionHorizontalEscapeVector`.
fn collision_horizontal_escape_vector(
    collider_width: f64,
    colliding_width: f64,
    direction_degrees: f32,
) -> DVec3 {
    let distance = (collider_width + colliding_width + 1.0e-5) / 2.0;
    let radians = direction_degrees * (std::f32::consts::PI / 180.0);
    let direction_x = -f64::from(radians.sin());
    let direction_z = f64::from(radians.cos());
    let scale = direction_x.abs().max(direction_z.abs());
    DVec3::new(
        direction_x * distance / scale,
        0.0,
        direction_z * distance / scale,
    )
}

impl Entity for StriderEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    /// Vanilla `Strider.tick`: strider state, `super.tick()`, then the lava float.
    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_strider_state();
        self.tick_living_entity(&entity);
        self.float_strider();
    }

    /// Vanilla `Strider` sets `blocksBuilding = true`.
    fn blocks_building(&self) -> bool {
        true
    }

    /// Vanilla `Strider.isOnFire` is always `false`.
    fn is_on_fire(&self) -> bool {
        false
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            STRIDER_BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        if self.is_saddled()
            && let Some(passenger) = self.first_passenger()
            && passenger.as_player().is_some_and(|player| {
                player.is_holding(&mut |item_stack: &ItemStack| {
                    item_stack.is(&vanilla_items::WARPED_FUNGUS_ON_A_STICK)
                })
            })
        {
            return Some(passenger);
        }

        self.controlling_passenger_mob()
    }

    fn can_add_passenger(&self, _passenger: &dyn Entity) -> bool {
        // Vanilla `Strider.canAddPassenger`.
        !self.is_vehicle() && !self.is_eye_in_lava()
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

    /// Vanilla `Strider.nextStep`.
    fn next_step(&self) -> f32 {
        self.base().movement_progress().move_dist() + NEXT_STEP_OFFSET
    }

    /// Vanilla `Strider.playStepSound`: the lava step sound while in lava.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        let sound = if self.is_in_lava() {
            &sound_events::ENTITY_STRIDER_STEP_LAVA
        } else {
            &sound_events::ENTITY_STRIDER_STEP
        };
        self.play_sound(sound, 1.0, 1.0);
    }

    /// Vanilla `Strider.checkFallDamage`: lava resets the fall distance.
    ///
    /// The default `Entity.checkFallDamage` body is inlined because a
    /// `Entity::check_fall_damage(self, ..)` UFCS call would resolve back to this
    /// override and recurse.
    fn check_fall_damage(
        self: Arc<Self>,
        vertical_movement: f64,
        on_ground: bool,
        on_state: BlockStateId,
        pos: BlockPos,
        world: &Arc<World>,
    ) {
        if self.is_in_lava() {
            self.reset_fall_distance();
            return;
        }

        if !self.is_in_water() && vertical_movement < 0.0 {
            self.base().accumulate_fall_distance(vertical_movement);
        }

        if !on_ground {
            return;
        }

        let fall_distance = self.fall_distance();
        if fall_distance > 0.0 {
            let behavior = BLOCK_BEHAVIORS.get_behavior(on_state.get_block());
            let fall_context =
                EntityFallOnContext::from_entity(fall_distance, self.as_entity_event_source());
            if let Some(fall_damage) = behavior.fall_on(on_state, world, pos, fall_context) {
                let damage_applied = Arc::clone(&self).cause_fall_damage(
                    fall_damage.fall_distance,
                    fall_damage.damage_modifier,
                    &fall_damage.source,
                );
                behavior.after_fall_on_damage(
                    on_state,
                    world,
                    pos,
                    self.as_entity_event_source(),
                    &fall_damage,
                    damage_applied,
                );
            }

            let supporting_state = self
                .base()
                .supporting_block()
                .map_or(on_state, |supporting_pos| {
                    world.get_block_state(supporting_pos)
                });
            world.game_event(
                &vanilla_game_events::HIT_GROUND,
                BlockPos::new(
                    self.position().x.floor() as i32,
                    self.position().y.floor() as i32,
                    self.position().z.floor() as i32,
                ),
                &GameEventContext::new(Some(self.as_entity_event_source()), Some(supporting_state)),
            );
        }

        self.reset_fall_distance();
    }

    /// Vanilla `Strider.checkSpawnObstruction` is `level.isUnobstructed(this)`,
    /// which skips the default `Entity` liquid check.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        let aabb = self.bounding_box();
        !world.has_entity_in_aabb_matching(&aabb, |entity| {
            entity.id() != self.id()
                && !entity.is_removed()
                && !entity.is_spectator()
                && entity.blocks_building()
                && !entity.is_passenger_of_same_vehicle(self.as_entity_event_source())
        })
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
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

impl LivingEntity for StriderEntity {
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
        Some(&sound_events::ENTITY_STRIDER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_STRIDER_DEATH)
    }

    /// Vanilla `Strider.canUseSlot`: the saddle slot only while alive and adult.
    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        slot != EquipmentSlot::Saddle || (Entity::is_alive(self) && !AgeableMob::is_baby(self))
    }

    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        slot == EquipmentSlot::Saddle || Mob::can_pick_up_loot(self)
    }

    fn equip_sound(&self, slot: EquipmentSlot, _stack: &ItemStack) -> Option<SoundEventRef> {
        (slot == EquipmentSlot::Saddle).then_some(&sound_events::ENTITY_STRIDER_SADDLE)
    }

    /// Vanilla `Strider.canStandOnFluid`: lava is walkable.
    fn can_stand_on_fluid(&self, fluid: FluidState) -> bool {
        fluid.is_lava()
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn tick_ridden(&self, controller: &Player, _ridden_input: DVec3) {
        let (yaw, pitch) = controller.rotation();
        self.set_ridden_rotation(yaw, pitch);
        ItemSteerable::tick_boost(self);
    }

    fn ridden_input(&self, _controller: &Player, _self_input: DVec3) -> DVec3 {
        // Vanilla `Strider.getRiddenInput`: always forward.
        DVec3::new(0.0, 0.0, 1.0)
    }

    fn ridden_speed(&self, _controller: &Player) -> f32 {
        let movement_speed = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::MOVEMENT_SPEED) as f32;
        let steering_modifier = if self.is_suffocating() {
            SUFFOCATE_STEERING_MODIFIER
        } else {
            STEERING_MODIFIER
        };
        movement_speed * steering_modifier * ItemSteerable::boost_factor(self)
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }
}

impl AgeableMob for StriderEntity {
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

    fn get_breed_offspring(
        &self,
        world: &Arc<World>,
        _partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        // Vanilla `Strider.getBreedOffspring`: `EntityTypes.STRIDER.create(level, BREEDING)`.
        ENTITIES.create(
            &vanilla_entities::STRIDER,
            next_entity_id(),
            self.position(),
            Arc::downgrade(world),
        )
    }
}

impl Animal for StriderEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        StriderEntity::is_food(item_stack)
    }
}

impl ItemSteerable for StriderEntity {
    fn item_based_steering(&self) -> &SyncMutex<ItemBasedSteering> {
        &self.steering
    }

    fn boost_time_total(&self) -> i32 {
        *self.entity_data.lock().boost_time.get()
    }

    fn set_boost_time_total(&self, boost_time_total: i32) {
        self.entity_data.lock().boost_time.set(boost_time_total);
    }
}

impl Mob for StriderEntity {
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

    /// Vanilla `Strider.getAmbientSound`: silent while panicking or tempted.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        (!PathfinderMob::is_panicking(self) && !self.is_being_tempted())
            .then_some(&sound_events::ENTITY_STRIDER_AMBIENT)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        if AgeableMob::is_baby(self) {
            return self.finalize_spawn_ageable_mob(world, spawn_reason, group_data);
        }

        let mut random = LegacyRandom::from_seed(rand::random());
        let mut group_data = group_data;

        if random.next_i32_bounded(ZOMBIFIED_PIGLIN_JOCKEY_CHANCE) == 0
            && world.is_spawning_monsters()
        {
            let jockey = ENTITIES.create(
                &vanilla_entities::ZOMBIFIED_PIGLIN,
                next_entity_id(),
                self.position(),
                Arc::downgrade(world),
            );
            if let Some(jockey) = jockey {
                // Vanilla passes `Zombie.ZombieGroupData(getSpawnAsBabyOdds(random), false)`
                // to the jockey's `finalizeSpawn`; Steel's zombified piglin has no
                // group-data support, so the baby roll is applied directly.
                if let Some(zombified_piglin) =
                    jockey.as_ref().downcast_ref::<ZombifiedPiglinEntity>()
                {
                    zombified_piglin.set_baby(ZombieEntity::get_spawn_as_baby_odds(&mut random));
                }

                if let Some(jockey_group_data) = self.spawn_jockey(world, &jockey, None) {
                    if let Some(jockey_living) = jockey.as_ref().as_living_entity() {
                        jockey_living.living_base().equipment().lock().set(
                            EquipmentSlot::MainHand,
                            ItemStack::new(&vanilla_items::WARPED_FUNGUS_ON_A_STICK),
                        );
                    }
                    self.living_base.equipment().lock().set(
                        EquipmentSlot::Saddle,
                        ItemStack::new(&vanilla_items::SADDLE),
                    );
                    self.set_guaranteed_drop(EquipmentSlot::Saddle);
                    group_data = Some(jockey_group_data);
                }
            }
        } else if random.next_i32_bounded(STRIDER_JOCKEY_CHANCE) == 0 {
            let jockey = ENTITIES.create(
                &vanilla_entities::STRIDER,
                next_entity_id(),
                self.position(),
                Arc::downgrade(world),
            );
            if let Some(jockey) = jockey {
                if let Some(jockey_mob) = jockey.as_ageable_mob() {
                    jockey_mob.set_age(BABY_STRIDER_JOCKEY_AGE);
                }

                if let Some(jockey_group_data) = self.spawn_jockey(world, &jockey, None) {
                    group_data = Some(jockey_group_data);
                }
            }
        } else {
            group_data = Some(SpawnGroupData::AgeableMob(
                AgeableMobGroupData::with_baby_spawn_chance(JOCKEY_GROUP_BABY_SPAWN_CHANCE),
            ));
        }

        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    /// Vanilla `Strider.mobInteract`: mount when saddled, then food/saddle use.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };
        let has_food = StriderEntity::is_food(&item_stack);

        if !has_food && self.is_saddled() && !self.is_vehicle() && !player.is_secondary_use_active()
        {
            if let Some(world) = self.level()
                && let Some(vehicle) = world.get_entity_by_id(self.id())
            {
                player.start_riding(&vehicle);
            }
            return InteractionResult::Success;
        }

        let interaction_result = Animal::mob_interact_animal(self, player, hand);
        if !interaction_result.consumes_action() {
            return if LivingEntity::is_equippable_in_slot(self, &item_stack, EquipmentSlot::Saddle)
            {
                LivingEntity::interact_living_entity_with_equippable(self, player, hand)
            } else {
                InteractionResult::Pass
            };
        }

        // Vanilla plays the strider eating sound for a consumed food interaction.
        if has_food && !self.is_silent() {
            self.play_sound(
                &sound_events::ENTITY_STRIDER_EAT,
                1.0,
                1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.2,
            );
        }

        interaction_result
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for StriderEntity {
    /// Vanilla `Strider.getWalkTargetValue`: lava is attractive, lava immersion is
    /// repulsive.
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        let Some(world) = self.level() else {
            return 0.0;
        };

        if world.get_block_state(pos).get_fluid_state().is_lava() {
            10.0
        } else if self.is_in_lava() {
            f32::NEG_INFINITY
        } else {
            0.0
        }
    }

    /// Vanilla `Strider.StriderPathNavigation.isStableDestination`.
    ///
    /// The vanilla default body is inlined because a
    /// `PathfinderMob::is_stable_destination(self, ..)` UFCS call would resolve back
    /// to this override and recurse.
    fn is_stable_destination(&self, pos: BlockPos) -> bool {
        let Some(world) = self.level() else {
            return false;
        };

        world.get_block_state(pos).get_block() == &vanilla_blocks::LAVA
            || self
                .mob_base()
                .navigation()
                .lock()
                .is_stable_destination(world.as_ref(), pos)
    }
}

/// Vanilla `Strider.StriderGoToLavaGoal`: walk to a lava pool with a pathfindable
/// block above it.
struct StriderGoToLavaGoal {
    move_to_block: MoveToBlockGoal,
}

impl StriderGoToLavaGoal {
    fn new(speed_modifier: f64) -> Self {
        Self {
            move_to_block: MoveToBlockGoal::with_vertical_search_range(
                speed_modifier,
                LAVA_GOAL_SEARCH_RANGE,
                LAVA_GOAL_VERTICAL_SEARCH_RANGE,
                |level, pos| {
                    level.get_block_state(pos).get_block() == &vanilla_blocks::LAVA
                        && level
                            .get_block_state(pos.above())
                            .is_pathfindable(PathComputationType::Land)
                },
            )
            // Vanilla `getMoveToTarget` returns `blockPos` rather than the block above.
            .with_move_to_target(|pos| pos)
            .with_recalculate_path_interval(LAVA_GOAL_RECALCULATE_PATH_INTERVAL),
        }
    }
}

impl Goal for StriderGoToLavaGoal {
    fn controls(&self) -> GoalControls {
        self.move_to_block.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.is_in_lava() && self.move_to_block.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.is_in_lava() && self.move_to_block.can_continue_to_use(mob)
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to_block.requires_update_every_tick()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.move_to_block.start(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to_block.tick(mob, entity);
    }
}

/// Vanilla `Strider` tempt goal handle: mirrors `Strider.temptGoal.isRunning()`.
struct StriderTemptGoal {
    inner: TemptGoal,
    /// Mirrors `Strider.temptGoal.isRunning()` for the happy sound in `Strider.tick`.
    running: Arc<AtomicBool>,
}

impl StriderTemptGoal {
    fn new(speed_modifier: f64, running: Arc<AtomicBool>) -> Self {
        Self {
            inner: TemptGoal::new(
                speed_modifier,
                |item_stack| {
                    REGISTRY
                        .items
                        .is_in_tag(item_stack.item(), &ItemTag::STRIDER_TEMPT_ITEMS)
                },
                false,
            ),
            running,
        }
    }
}

impl Goal for StriderTemptGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
        self.running.store(true, Ordering::Relaxed);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
        self.running.store(false, Ordering::Relaxed);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}
