//! Vanilla Turtle entity with home-return, water travel and egg-laying parity.
//!
//! Mirrors `net.minecraft.world.entity.animal.turtle.Turtle`. The vanilla inner goal
//! classes (`Turtle.TurtleGoHomeGoal`, `Turtle.TurtleGoToWaterGoal`,
//! `Turtle.TurtleLayEggGoal`, `Turtle.TurtleTravelGoal`,
//! `Turtle.TurtleRandomStrollGoal`) are private structs in this module: they drive
//! turtle-only state, exactly like their vanilla counterparts.
//!
//! GAP: the vanilla `Turtle.TurtleMoveControl` speed curve and the
//! `Turtle.TurtlePathNavigation.isStableDestination` water preference (keyed on
//! `travelPos`) have no Steel hook; the shared move control and navigation are used
//! instead, so long-distance swim travel keeps the generic surface preference.
//!
//! Deviation: `Turtle.thunderHit` deals `f32::MAX` lightning damage rather than
//! calling vanilla's `hurtServer` directly, so the strike still passes through
//! Steel's shared damage pipeline.

use std::f64::consts::PI;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::entity_data::EntityPose;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::{LootContext, LootTableRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::TurtleEntityData;
use steel_registry::vanilla_game_events;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::vanilla_loot_tables;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, level_events, sound_events, vanilla_attributes, vanilla_blocks,
    vanilla_damage_types, vanilla_game_rules,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    BreedGoal, Goal, GoalControls, LookAtPlayerGoal, PanicGoal, RandomStrollGoal, TemptGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData, entity_loot_ref, living_loot_equipment,
};
use crate::fluid::FluidStateExt as _;
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::world::{LevelReader, World};

/// Vanilla `Turtle.nextStep` adds `0.15` to the accumulated movement distance.
const NEXT_STEP_OFFSET: f32 = 0.15;

/// Vanilla `Turtle.BABY_DIMENSIONS`: the adult box scaled by `BABY_SCALE`.
const TURTLE_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.12, -0.075)];
const TURTLE_BABY_WIDTH: f32 = 0.36;
const TURTLE_BABY_HEIGHT: f32 = 0.12;
const TURTLE_BABY_EYE_HEIGHT: f32 = 0.102;

const TURTLE_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    TURTLE_BABY_WIDTH,
    TURTLE_BABY_HEIGHT,
    TURTLE_BABY_EYE_HEIGHT,
    EntityAttachments::new(&TURTLE_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `Turtle.getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 200;
/// Vanilla `Turtle.getAgeScale` for babies.
const BABY_AGE_SCALE: f32 = 0.3;
/// Vanilla `Turtle.TurtleGoHomeGoal`/`TurtleTravelGoal` give-up distance.
const HOME_GIVE_UP_DISTANCE: f64 = 7.0;
/// Vanilla `TurtleLayEggGoal` requires the turtle to be this close to its home.
const LAY_EGG_HOME_DISTANCE: f64 = 9.0;
/// Vanilla `TurtleLayEggGoal` lays the egg after this many adjusted ticks.
const LAY_EGG_TICKS: i32 = 200;
/// Vanilla `TurtleLayEggGoal` re-enters love mode for this many ticks after laying.
const LAY_EGG_IN_LOVE_TICKS: i32 = 600;
/// Vanilla `TurtleLayEggGoal` rolls `random.nextInt(4) + 1` eggs.
const MAX_EGGS_PER_LAY: i32 = 4;
/// Vanilla `TurtleGoToWaterGoal`/`TurtleGoHomeGoal` give-up ticks.
const GO_TO_WATER_GIVE_UP_TICKS: i32 = 1200;
const GO_HOME_GIVE_UP_TICKS: i32 = 600;
/// Vanilla `TurtleGoHomeGoal` re-paths within this distance of home.
const GO_HOME_CLOSE_DISTANCE: f64 = 16.0;
/// Vanilla `Turtle.TurtleTravelGoal` travel target is picked within this box.
const TRAVEL_HORIZONTAL_RANGE: i32 = 512;
const TRAVEL_VERTICAL_RANGE: i32 = 4;
/// Vanilla `Turtle.TurtleRandomStrollGoal` interval.
const RANDOM_STROLL_INTERVAL: i32 = 100;

/// Vanilla `TurtleEggBlock.onSand`: the block below `pos` is sand.
fn egg_on_sand(world: &dyn LevelReader, pos: BlockPos) -> bool {
    world
        .get_block_state(pos.below())
        .get_block()
        .has_tag(&BlockTag::SAND)
}

/// Vanilla `TurtleEggBlock.isSand`: the block at `pos` is sand.
fn egg_is_sand(world: &dyn LevelReader, pos: BlockPos) -> bool {
    world
        .get_block_state(pos)
        .get_block()
        .has_tag(&BlockTag::SAND)
}

/// Vanilla `Turtle.BABY_ON_LAND_SELECTOR`: babies that are not in water.
///
/// Cat/ocelot target goals use this to avoid attacking baby turtles in water.
#[must_use]
pub fn is_baby_on_land_selector(entity: &SharedEntity) -> bool {
    entity.as_living_entity().is_some_and(LivingEntity::is_baby) && !entity.is_in_water()
}

/// Runtime state vanilla keeps in `Turtle` fields rather than synched data.
#[derive(Debug, Clone, Copy)]
struct TurtleState {
    home_pos: BlockPos,
    travel_pos: Option<BlockPos>,
    going_home: bool,
    lay_egg_counter: i32,
}

impl Default for TurtleState {
    fn default() -> Self {
        Self {
            home_pos: BlockPos::ZERO,
            travel_pos: None,
            going_home: false,
            lay_egg_counter: 0,
        }
    }
}

#[entity_behavior(class = "Turtle")]
/// Vanilla turtle entity with home-return, water travel and egg-laying state.
pub struct TurtleEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<TurtleEntityData>,
    state: SyncMutex<TurtleState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TurtleEntity`.
unsafe impl DowncastType for TurtleEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/turtle");
}

impl TurtleEntity {
    /// Creates a new turtle at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a turtle from persisted base entity state.
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
        let mut entity_data = TurtleEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Keep vanilla `Turtle.registerGoals` priorities and speeds in order.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, PanicGoal::new(1.2));
            goal_selector.add_goal(1, TurtleBreedGoal::new(1.0));
            goal_selector.add_goal(1, TurtleLayEggGoal::new(1.0));
            goal_selector.add_goal(
                2,
                TemptGoal::new(
                    1.1,
                    |item_stack| {
                        REGISTRY
                            .items
                            .is_in_tag(item_stack.item(), &ItemTag::TURTLE_FOOD)
                    },
                    false,
                ),
            );
            goal_selector.add_goal(3, TurtleGoToWaterGoal::new(1.0));
            goal_selector.add_goal(4, TurtleGoHomeGoal::new(1.0));
            goal_selector.add_goal(7, TurtleTravelGoal::new(1.0));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(9, TurtleRandomStrollGoal::new(1.0, RANDOM_STROLL_INTERVAL));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            state: SyncMutex::new(TurtleState::default()),
        }
    }

    /// Returns the block the turtle calls home.
    #[must_use]
    pub fn home_pos(&self) -> BlockPos {
        self.state.lock().home_pos
    }

    /// Sets the block the turtle calls home.
    pub fn set_home_pos(&self, pos: BlockPos) {
        self.state.lock().home_pos = pos;
    }

    /// Vanilla `Turtle.hasEgg`.
    #[must_use]
    pub fn has_egg(&self) -> bool {
        *self.entity_data.lock().has_egg.get()
    }

    /// Vanilla `Turtle.setHasEgg`.
    pub fn set_has_egg(&self, has_egg: bool) {
        self.entity_data.lock().has_egg.set(has_egg);
    }

    /// Vanilla `Turtle.isLayingEgg`.
    #[must_use]
    pub fn is_laying_egg(&self) -> bool {
        *self.entity_data.lock().laying_egg.get()
    }

    /// Vanilla `Turtle.setLayingEgg`, which also resets the lay counter.
    pub fn set_laying_egg(&self, laying: bool) {
        self.state.lock().lay_egg_counter = i32::from(laying);
        self.entity_data.lock().laying_egg.set(laying);
    }

    fn is_going_home(&self) -> bool {
        self.state.lock().going_home
    }

    fn set_going_home(&self, going_home: bool) {
        self.state.lock().going_home = going_home;
    }

    fn travel_pos(&self) -> Option<BlockPos> {
        self.state.lock().travel_pos
    }

    fn set_travel_pos(&self, travel_pos: Option<BlockPos>) {
        self.state.lock().travel_pos = travel_pos;
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

    /// Returns whether an item stack matches the vanilla turtle food tag.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::TURTLE_FOOD)
    }

    /// Vanilla `Turtle.aiStep` egg-laying particles and sound.
    fn tick_laying_egg(&self) {
        if !Entity::is_alive(self) || !self.is_laying_egg() {
            return;
        }

        {
            // Vanilla only plays the digging effect; the goal advances the counter.
            let state = self.state.lock();
            if state.lay_egg_counter < 1 || state.lay_egg_counter % 5 != 0 {
                return;
            }
        }

        let Some(world) = self.level() else {
            return;
        };
        let pos = self.block_position();
        if egg_on_sand(world.as_ref(), pos) {
            let below = world.get_block_state(pos.below());
            world.level_event(
                level_events::PARTICLES_AND_SOUND_DESTROY_BLOCK,
                pos,
                level_events::encode_block_state_data(u32::from(below.0)),
                None,
            );
            self.game_event(&vanilla_game_events::ENTITY_ACTION);
        }
    }
}

/// Vanilla `LivingEntity.dropFromGiftLootTable` for the turtle grow table.
impl TurtleEntity {
    fn drop_gift_loot_table(&self, loot_table: LootTableRef) -> bool {
        let position = self.position();
        let mut rng = rand::rng();
        let equipment = living_loot_equipment(self);
        // Vanilla `LootContextParamSets.GIFT` carries only ORIGIN and THIS_ENTITY.
        let mut context = LootContext::new(&mut rng)
            .with_origin(position.x, position.y, position.z)
            .with_this_entity(entity_loot_ref(self, Some(&equipment)));

        let items = loot_table.get_random_items(&mut context);
        let dropped_any = !items.is_empty();
        for item in items {
            self.spawn_at_location(item, 0.0);
        }
        dropped_any
    }
}

impl Entity for TurtleEntity {
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
        if AgeableMob::is_baby(self) {
            TURTLE_BABY_DIMENSIONS.scale(scale)
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
        let sound = if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_TURTLE_SHAMBLE_BABY
        } else {
            &sound_events::ENTITY_TURTLE_SHAMBLE
        };
        self.play_sound(sound, 0.15, 1.0);
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_TURTLE_SWIM
    }

    fn play_swim_sound(&self, volume: f32) {
        // Vanilla `Turtle.playSwimSound` amplifies the shared swim sound by 1.5.
        // The `Entity::play_swim_sound` default body is inlined: a UFCS call here
        // would resolve back to this override and recurse.
        let volume = volume * 1.5;
        let pitch = 1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.4;
        self.play_sound(self.swim_sound(), volume, pitch);
    }

    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn thunder_hit(&self, world: &Arc<World>, _bolt: &SharedEntity) {
        self.hurt(
            world,
            &DamageSource::environment(&vanilla_damage_types::LIGHTNING_BOLT),
            f32::MAX,
        );
    }

    fn next_step(&self) -> f32 {
        self.base().movement_progress().move_dist() + NEXT_STEP_OFFSET
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);

        let home_pos = self.home_pos();
        nbt.insert(
            "home_pos",
            NbtTag::IntArray(vec![home_pos.x(), home_pos.y(), home_pos.z()]),
        );
        nbt.insert("has_egg", i8::from(self.has_egg()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.set_home_pos(
            nbt.int_array("home_pos")
                .filter(|position| position.len() == 3)
                .map_or_else(
                    || self.block_position(),
                    |position| BlockPos::new(position[0], position[1], position[2]),
                ),
        );

        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_has_egg(nbt.byte("has_egg").is_some_and(|value| value != 0));
    }
}

impl LivingEntity for TurtleEntity {
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
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_TURTLE_HURT_BABY
        } else {
            &sound_events::ENTITY_TURTLE_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_TURTLE_DEATH_BABY
        } else {
            &sound_events::ENTITY_TURTLE_DEATH
        })
    }

    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            BABY_AGE_SCALE
        } else {
            1.0
        }
    }

    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        _base_gravity: f64,
        _is_falling: bool,
        _old_y: f64,
    ) -> Option<MoveResult> {
        // Vanilla `Turtle.travelInWater`.
        self.move_relative(0.1, input);
        let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
        self.set_velocity(self.velocity() * 0.9);
        let home_pos = self.home_pos();
        if self.target().is_none()
            && (!self.is_going_home() || !home_pos.closer_to_center_than(self.position(), 20.0))
        {
            self.set_velocity(self.velocity() + DVec3::new(0.0, -0.005, 0.0));
        }
        Some(result)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        self.tick_laying_egg();
        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }
}

impl AgeableMob for TurtleEntity {
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

    fn age_boundary_changed(&self, baby: bool) {
        self.refresh_dimensions();
        if baby {
            return;
        }

        // Vanilla `Turtle.ageBoundaryReached` drops the `gameplay/turtle_grow` gift
        // loot when a baby grows up and mob drops are enabled.
        let Some(world) = self.level() else {
            return;
        };
        if !world.get_game_rule(&vanilla_game_rules::MOB_DROPS) {
            return;
        }
        self.drop_gift_loot_table(&vanilla_loot_tables::GAMEPLAY_TURTLE_GROW);
    }
}

impl Animal for TurtleEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        TurtleEntity::is_food(item_stack)
    }

    fn can_fall_in_love(&self) -> bool {
        // Vanilla `Turtle.canFallInLove`: love mode only without a stored egg.
        // `Animal.can_fall_in_love`'s `inLove <= 0` check already ran in the caller.
        !self.has_egg()
    }

    fn spawn_child_from_breeding(&self, world: &Arc<World>, partner: &dyn Animal) {
        // Vanilla `Turtle.TurtleBreedGoal.breed`: the female stores an egg instead of
        // creating offspring, and both parents enter the post-breeding cooldown.
        self.set_has_egg(true);
        self.finalize_spawn_child_from_breeding(world, partner, None);
    }

    /// Vanilla `Turtle.checkTurtleSpawnRules`.
    fn check_animal_spawn_rules(
        level: &dyn LevelReader,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        pos.y() < level.sea_level() + 4
            && (egg_is_sand(level, pos) || egg_on_sand(level, pos))
            && Self::is_bright_enough_to_spawn(level, pos)
    }
}

impl Mob for TurtleEntity {
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

    fn ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        // Vanilla `Turtle.getAmbientSound`: land-only, adult-only ambient sound.
        if !self.is_in_water() && self.on_ground() && !AgeableMob::is_baby(self) {
            Some(&sound_events::ENTITY_TURTLE_AMBIENT_LAND)
        } else {
            None
        }
    }

    fn mob_can_be_leashed(&self) -> bool {
        false
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `Turtle.finalizeSpawn` anchors the turtle to its spawn block.
        self.set_home_pos(self.block_position());
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        Animal::mob_interact_animal(self, player, hand)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for TurtleEntity {
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        // Vanilla `Turtle.getWalkTargetValue`: water and sand are worth 10.
        let Some(world) = self.level() else {
            return 0.0;
        };

        if (!self.is_going_home() && world.get_block_state(pos).get_fluid_state().is_water())
            || egg_on_sand(world.as_ref(), pos)
        {
            10.0
        } else {
            world.pathfinding_cost_from_light_levels(pos)
        }
    }
}

/// Vanilla `Turtle.TurtleBreedGoal`: the shared [`BreedGoal`] plus the vanilla
/// `!hasEgg()` start condition. The egg itself is stored by
/// `TurtleEntity::spawn_child_from_breeding`.
struct TurtleBreedGoal {
    breed: BreedGoal,
}

impl TurtleBreedGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            breed: BreedGoal::new(speed_modifier),
        }
    }

    fn turtle(mob: &dyn PathfinderMob) -> Option<&TurtleEntity> {
        mob.as_entity_event_source().downcast_ref::<TurtleEntity>()
    }
}

impl Goal for TurtleBreedGoal {
    fn controls(&self) -> GoalControls {
        self.breed.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        // Vanilla `Turtle.TurtleBreedGoal.canUse` adds `!this.turtle.hasEgg()`.
        if Self::turtle(mob).is_none_or(TurtleEntity::has_egg) {
            return false;
        }
        self.breed.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.breed.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.breed.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.breed.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.breed.tick(mob, entity);
    }
}

/// Vanilla `Turtle.TurtleRandomStrollGoal`: never strolls in water, home, or with an egg.
///
/// The walk logic is the shared [`RandomStrollGoal`]; only `canUse` is turtle-specific.
struct TurtleRandomStrollGoal {
    stroll: RandomStrollGoal,
}

impl TurtleRandomStrollGoal {
    const fn new(speed_modifier: f64, interval: i32) -> Self {
        Self {
            stroll: RandomStrollGoal::with_interval(speed_modifier, interval),
        }
    }

    fn turtle(mob: &dyn PathfinderMob) -> Option<&TurtleEntity> {
        mob.as_entity_event_source().downcast_ref::<TurtleEntity>()
    }
}

impl Goal for TurtleRandomStrollGoal {
    fn controls(&self) -> GoalControls {
        self.stroll.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        if mob.is_in_water() || turtle.is_going_home() || turtle.has_egg() {
            return false;
        }
        self.stroll.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.stroll.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.stroll.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.stroll.tick(mob, entity);
    }
}

/// Vanilla `Turtle.TurtleGoHomeGoal`: swim home when carrying an egg, or rarely
/// when far away, and give up when the path cannot be found.
struct TurtleGoHomeGoal {
    speed_modifier: f64,
    stuck: bool,
    close_to_home_try_ticks: i32,
}

impl TurtleGoHomeGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            stuck: false,
            close_to_home_try_ticks: 0,
        }
    }

    fn turtle(mob: &dyn PathfinderMob) -> Option<&TurtleEntity> {
        mob.as_entity_event_source().downcast_ref::<TurtleEntity>()
    }
}

impl Goal for TurtleGoHomeGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        if AgeableMob::is_baby(turtle) {
            return false;
        }
        if turtle.has_egg() {
            return true;
        }

        // Vanilla `reducedTickDelay(700)`.
        let interval = reduced_tick_delay(700);
        rand::random_range(0..interval) == 0
            && !turtle
                .home_pos()
                .closer_to_center_than(mob.position(), 64.0)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        !turtle
            .home_pos()
            .closer_to_center_than(mob.position(), HOME_GIVE_UP_DISTANCE)
            && !self.stuck
            && self.close_to_home_try_ticks <= GO_HOME_GIVE_UP_TICKS
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(turtle) = Self::turtle(mob) {
            turtle.set_going_home(true);
        }
        self.stuck = false;
        self.close_to_home_try_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(turtle) = Self::turtle(mob) {
            turtle.set_going_home(false);
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(turtle) = Self::turtle(mob) else {
            return;
        };
        let home_pos = turtle.home_pos();
        let close_to_home = home_pos.closer_to_center_than(mob.position(), GO_HOME_CLOSE_DISTANCE);
        if close_to_home {
            self.close_to_home_try_ticks += 1;
        }

        if !mob.mob_base().navigation().lock().is_done() {
            return;
        }

        let home_center = DVec3::new(
            f64::from(home_pos.x()) + 0.5,
            f64::from(home_pos.y()),
            f64::from(home_pos.z()) + 0.5,
        );
        let mut next_pos = random_pos_towards(mob, 16, 3, home_center, PI / 10.0);
        if next_pos.is_none() {
            next_pos = random_pos_towards(mob, 8, 7, home_center, PI / 2.0);
        }

        if let Some(pos) = next_pos
            && !close_to_home
            && let Some(world) = mob.level()
            && !world
                .get_block_state(BlockPos::containing(pos.x, pos.y, pos.z))
                .get_fluid_state()
                .is_water()
        {
            next_pos = random_pos_towards(mob, 16, 5, home_center, PI / 2.0);
        }

        let Some(next_pos) = next_pos else {
            self.stuck = true;
            return;
        };

        mob.move_to_pos(next_pos, self.speed_modifier);
    }
}

/// Vanilla `Turtle.TurtleGoToWaterGoal`: walk into water, babies included.
struct TurtleGoToWaterGoal {
    speed_modifier: f64,
    block_pos: BlockPos,
    try_ticks: i32,
    next_start_tick: i32,
    vertical_search_start: i32,
}

impl TurtleGoToWaterGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            block_pos: BlockPos::ZERO,
            try_ticks: 0,
            next_start_tick: 0,
            // Vanilla sets `verticalSearchStart = -1`.
            vertical_search_start: -1,
        }
    }

    fn turtle(mob: &dyn PathfinderMob) -> Option<&TurtleEntity> {
        mob.as_entity_event_source().downcast_ref::<TurtleEntity>()
    }

    fn is_valid_target(world: &dyn LevelReader, pos: BlockPos) -> bool {
        world.get_block_state(pos).get_fluid_state().is_water()
    }

    fn find_nearest_water(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        let Some(pos) = find_nearest_block(
            mob.block_position(),
            24,
            self.vertical_search_start,
            1,
            |pos| mob.is_within_home_pos(pos) && Self::is_valid_target(world.as_ref(), pos),
        ) else {
            return false;
        };
        self.block_pos = pos;
        true
    }
}

impl Goal for TurtleGoToWaterGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::JUMP
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        let is_baby = AgeableMob::is_baby(turtle);
        if is_baby && !mob.is_in_water() {
            return self.find_nearest_water(mob);
        }
        if turtle.is_going_home() || mob.is_in_water() || turtle.has_egg() {
            return false;
        }

        if self.next_start_tick > 0 {
            self.next_start_tick -= 1;
            return false;
        }
        self.next_start_tick = reduced_tick_delay(200 + rand::random_range(0..200));

        self.find_nearest_water(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if mob.is_in_water() || self.try_ticks > GO_TO_WATER_GIVE_UP_TICKS {
            return false;
        }
        let Some(world) = mob.level() else {
            return false;
        };
        Self::is_valid_target(world.as_ref(), self.block_pos)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.try_ticks = 0;
        self.move_towards_target(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let target = self.block_pos;
        let target_center = DVec3::new(
            f64::from(target.x()) + 0.5,
            f64::from(target.y()),
            f64::from(target.z()) + 0.5,
        );
        if target_center.distance_squared(mob.position()) < 1.0 {
            self.try_ticks -= 1;
        } else {
            self.try_ticks += 1;
            // Vanilla `shouldRecalculatePath` is `tryTicks % 160 == 0`.
            if self.try_ticks % 160 == 0 {
                self.move_towards_target(mob);
            }
        }
    }
}

impl TurtleGoToWaterGoal {
    fn move_towards_target(&self, mob: &dyn PathfinderMob) {
        let target = self.block_pos;
        let speed = if Self::turtle(mob).is_some_and(AgeableMob::is_baby) {
            2.0
        } else {
            self.speed_modifier
        };
        mob.move_to_pos(
            DVec3::new(
                f64::from(target.x()) + 0.5,
                f64::from(target.y()),
                f64::from(target.z()) + 0.5,
            ),
            speed,
        );
    }
}

/// Vanilla `Turtle.TurtleLayEggGoal`: return home, then dig in the sand and place a
/// `TURTLE_EGG` block with 1-4 eggs.
struct TurtleLayEggGoal {
    speed_modifier: f64,
    block_pos: BlockPos,
    try_ticks: i32,
    next_start_tick: i32,
}

impl TurtleLayEggGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            block_pos: BlockPos::ZERO,
            try_ticks: 0,
            next_start_tick: 0,
        }
    }

    fn turtle(mob: &dyn PathfinderMob) -> Option<&TurtleEntity> {
        mob.as_entity_event_source().downcast_ref::<TurtleEntity>()
    }

    /// Vanilla `TurtleLayEggGoal.isValidTarget`: sand with empty space above.
    fn is_valid_target(world: &dyn LevelReader, pos: BlockPos) -> bool {
        world.get_block_state(pos.above()).is_air() && egg_is_sand(world, pos)
    }

    fn find_nest(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };

        let Some(pos) = find_nearest_block(mob.block_position(), 16, 0, 1, |pos| {
            mob.is_within_home_pos(pos) && Self::is_valid_target(world.as_ref(), pos)
        }) else {
            return false;
        };
        self.block_pos = pos;
        true
    }
}

impl Goal for TurtleLayEggGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::JUMP
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        if !turtle.has_egg()
            || !turtle
                .home_pos()
                .closer_to_center_than(mob.position(), LAY_EGG_HOME_DISTANCE)
        {
            return false;
        }

        if self.next_start_tick > 0 {
            self.next_start_tick -= 1;
            return false;
        }
        self.next_start_tick = reduced_tick_delay(200 + rand::random_range(0..200));

        self.find_nest(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        if !turtle.has_egg()
            || !turtle
                .home_pos()
                .closer_to_center_than(mob.position(), LAY_EGG_HOME_DISTANCE)
        {
            return false;
        }

        let Some(world) = mob.level() else {
            return false;
        };
        self.try_ticks <= GO_TO_WATER_GIVE_UP_TICKS
            && Self::is_valid_target(world.as_ref(), self.block_pos)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.try_ticks = 0;
        self.move_towards_target(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(turtle) = Self::turtle(mob) else {
            return;
        };
        let target = self.block_pos;
        let target_center = DVec3::new(
            f64::from(target.x()) + 0.5,
            f64::from(target.y()),
            f64::from(target.z()) + 0.5,
        );
        let reached_target = target_center.distance_squared(mob.position()) < 1.0;
        if reached_target {
            self.try_ticks -= 1;
        } else {
            self.try_ticks += 1;
            if self.try_ticks % 40 == 0 {
                self.move_towards_target(mob);
            }
        }

        if mob.is_in_water() || !reached_target {
            return;
        }

        let lay_egg_counter = turtle.state.lock().lay_egg_counter;
        if lay_egg_counter < 1 {
            turtle.set_laying_egg(true);
        } else if lay_egg_counter > reduced_tick_delay(LAY_EGG_TICKS) {
            Self::lay_egg(turtle);
        }

        if turtle.is_laying_egg() {
            turtle.state.lock().lay_egg_counter += 1;
        }
    }
}

impl TurtleLayEggGoal {
    fn move_towards_target(&self, mob: &dyn PathfinderMob) {
        let target = self.block_pos;
        mob.move_to_pos(
            DVec3::new(
                f64::from(target.x()) + 0.5,
                f64::from(target.y() + 1),
                f64::from(target.z()) + 0.5,
            ),
            self.speed_modifier,
        );
    }

    /// Vanilla `TurtleLayEggGoal` egg placement block of `tick`.
    fn lay_egg(turtle: &TurtleEntity) {
        let Some(world) = turtle.level() else {
            return;
        };

        let turtle_pos = turtle.block_position();
        let pitch = 0.9 + rand::random::<f32>() * 0.2;
        world.play_block_sound(
            &sound_events::ENTITY_TURTLE_LAY_EGG,
            turtle_pos,
            0.3,
            pitch,
            None,
        );

        let egg_pos = turtle_pos.above();
        let eggs = rand::random_range(1..=MAX_EGGS_PER_LAY);
        let egg_state = vanilla_blocks::TURTLE_EGG
            .default_state()
            .set_value(&BlockStateProperties::EGGS, eggs as u8);
        world.set_block(egg_pos, egg_state, UpdateFlags::UPDATE_ALL);
        turtle.game_event(&vanilla_game_events::BLOCK_PLACE);

        turtle.set_has_egg(false);
        turtle.set_laying_egg(false);
        Animal::set_in_love_time(turtle, LAY_EGG_IN_LOVE_TICKS);
    }
}

/// Vanilla `Turtle.TurtleTravelGoal`: swim towards a distant water target.
struct TurtleTravelGoal {
    speed_modifier: f64,
    stuck: bool,
}

impl TurtleTravelGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            stuck: false,
        }
    }

    fn turtle(mob: &dyn PathfinderMob) -> Option<&TurtleEntity> {
        mob.as_entity_event_source().downcast_ref::<TurtleEntity>()
    }
}

impl Goal for TurtleTravelGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        !turtle.is_going_home() && !turtle.has_egg() && mob.is_in_water()
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(turtle) = Self::turtle(mob) else {
            return false;
        };
        !mob.mob_base().navigation().lock().is_done()
            && !self.stuck
            && !turtle.is_going_home()
            && !Animal::is_in_love(turtle)
            && !turtle.has_egg()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(turtle) = Self::turtle(mob) else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };

        let position = mob.position();
        let mut y_offset = rand::random_range(-TRAVEL_VERTICAL_RANGE..=TRAVEL_VERTICAL_RANGE);
        if f64::from(y_offset) + position.y > f64::from(world.sea_level()) - 1.0 {
            y_offset = 0;
        }

        let target = BlockPos::containing(
            position.x
                + f64::from(rand::random_range(
                    -TRAVEL_HORIZONTAL_RANGE..=TRAVEL_HORIZONTAL_RANGE,
                )),
            position.y + f64::from(y_offset),
            position.z
                + f64::from(rand::random_range(
                    -TRAVEL_HORIZONTAL_RANGE..=TRAVEL_HORIZONTAL_RANGE,
                )),
        );
        turtle.set_travel_pos(Some(target));
        self.stuck = false;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(turtle) = Self::turtle(mob) else {
            return;
        };
        let Some(travel_pos) = turtle.travel_pos() else {
            self.stuck = true;
            return;
        };

        if !mob.mob_base().navigation().lock().is_done() {
            return;
        }

        let target_center = DVec3::new(
            f64::from(travel_pos.x()) + 0.5,
            f64::from(travel_pos.y()),
            f64::from(travel_pos.z()) + 0.5,
        );
        let mut next_pos = random_pos_towards(mob, 16, 3, target_center, PI / 10.0);
        if next_pos.is_none() {
            next_pos = random_pos_towards(mob, 8, 7, target_center, PI / 2.0);
        }

        // Vanilla rejects positions whose 34-block chunk neighborhood is unloaded.
        if let Some(pos) = next_pos {
            let x = pos.x.floor() as i32;
            let z = pos.z.floor() as i32;
            if !mob
                .level()
                .is_some_and(|world| world.has_chunks_at(x - 34, z - 34, x + 34, z + 34))
            {
                next_pos = None;
            }
        }

        let Some(next_pos) = next_pos else {
            self.stuck = true;
            return;
        };

        mob.move_to_pos(next_pos, self.speed_modifier);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(turtle) = Self::turtle(mob) {
            turtle.set_travel_pos(None);
        }
        mob.mob_base().navigation().lock().stop();
    }
}

/// Vanilla `Mob.reducedTickDelay`.
///
/// `crate::entity::ai::goal::reduced_tick_delay` is `pub(super)` to the goal module,
/// so the identical expression lives here for the turtle goals that need it.
const fn reduced_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
}

/// Vanilla `DefaultRandomPos.getPosTowards`.
///
/// GAP: Steel's `ai::goal::random_pos` keeps this function private to the goal
/// module, so the random-direction search is reproduced here with Vanilla's
/// 10-attempt walk-target weighting. The vanilla water/lava rejection of candidate
/// blocks is not applied; callers filter the water case themselves (the home goal
/// re-rolls its target at a wider angle, exactly like vanilla).
fn random_pos_towards(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
    towards_pos: DVec3,
    max_xz_radians_from_dir: f64,
) -> Option<DVec3> {
    const ATTEMPTS: usize = 10;

    let dir = towards_pos - mob.position();
    let base_angle = dir.z.atan2(dir.x);

    let mut best: Option<(f32, DVec3)> = None;
    for _ in 0..ATTEMPTS {
        let angle =
            base_angle + rand::random_range(-max_xz_radians_from_dir..=max_xz_radians_from_dir);
        let distance = rand::random::<f64>() * f64::from(horizontal_dist);
        let y_offset = (rand::random::<f64>() * 2.0 - 1.0) * f64::from(vertical_dist);
        let candidate =
            mob.position() + DVec3::new(angle.cos() * distance, y_offset, angle.sin() * distance);
        let weight =
            mob.get_walk_target_value(BlockPos::containing(candidate.x, candidate.y, candidate.z));

        if best.is_none_or(|(best_weight, _)| weight > best_weight) {
            best = Some((weight, candidate));
        }
    }

    best.map(|(_, pos)| pos)
}

/// Vanilla `MoveToBlockGoal` search helper shared by the turtle water/egg goals.
///
/// The shared [`crate::entity::ai::goal`] `MoveToBlockGoal` is not exported, so the
/// nearest-match search (horizontal range, vertical range, `home` restriction) is
/// reproduced here for the two turtle goals that need it.
fn find_nearest_block(
    origin: BlockPos,
    horizontal_range: i32,
    vertical_start: i32,
    vertical_range: i32,
    mut predicate: impl FnMut(BlockPos) -> bool,
) -> Option<BlockPos> {
    let vertical_end = vertical_start + vertical_range;
    let mut best: Option<(i32, BlockPos)> = None;

    for y in vertical_start..vertical_end {
        for dx in -horizontal_range..=horizontal_range {
            for dz in -horizontal_range..=horizontal_range {
                let pos = BlockPos::new(origin.x() + dx, origin.y() + y, origin.z() + dz);
                if !predicate(pos) {
                    continue;
                }

                let distance = dx * dx + y * y + dz * dz;
                if best.is_none_or(|(best_distance, _)| distance < best_distance) {
                    best = Some((distance, pos));
                }
            }
        }
    }

    best.map(|(_, pos)| pos)
}
