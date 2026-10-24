//! Vanilla `Dolphin` (`net.minecraft.world.entity.animal.dolphin.Dolphin`).
//!
//! Vanilla is a water creature: `WaterBoundPathNavigation` with breaching enabled,
//! `SmoothSwimmingMoveControl(85, 10, 0.02F, 0.1F, true)` and
//! `SmoothSwimmingLookControl(10)`. Steel selects the water-bound navigation kind
//! on its shared navigation and ticks the two smooth-swimming controls.
//!
//! Deviation: vanilla `Dolphin.DolphinSwimToTreasureGoal` needs
//! `ServerLevel.findNearestMapStructure(StructureTags.DOLPHIN_LOCATED, …)`; Steel has no
//! structure-locator API yet, so that goal is not registered and a dolphin that is fed
//! fish keeps the `GotFish` flag without swimming to buried treasure.
//! Deviation: vanilla `getSwimSplashSound` has no Steel hook; only `getSwimSound`
//! (`ENTITY_DOLPHIN_SWIM`) is reachable, so water-entry splashes use the generic sound.

use std::f32::consts::TAU;
use std::ptr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::DolphinEntityData;
use steel_registry::vanilla_mob_effects;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_blocks, vanilla_damage_types, vanilla_entities,
    vanilla_item_tags,
};
use steel_utils::Downcast as _;
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::MobEffectInstance;
use crate::entity::ai::control::{
    SmoothSwimmingLookControl, SmoothSwimmingMoveControl, tick_smooth_swimming_look_control,
    tick_smooth_swimming_move_control,
};
use crate::entity::ai::goal::{
    AvoidEntityGoal, BreathAirGoal, DolphinJumpGoal, FollowPlayerRiddenEntityGoal, Goal,
    GoalControls, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal, RandomLookAroundGoal,
    RandomSwimmingGoal, TryFindWaterGoal,
};
use crate::entity::ai::navigation::NavigationKind;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::ItemEntity;
use crate::entity::entities::mobs::water::abstract_fish::check_water_animal_spawn_obstruction;
use crate::entity::{
    AgeableMob, AgeableMobBase, AgeableMobGroupData, Animal, AnimalBase, Entity, EntityBase,
    EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::world::{LevelReader, World};

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Dolphin.MAX_MOISTNESS_LEVEL`.
const MAX_MOISTNESS: i32 = 2400;
/// Vanilla `Dolphin.getMaxAirSupply`.
const MAX_AIR_SUPPLY: i32 = 4800;
/// Vanilla `Dolphin` dry-out damage.
const DRY_OUT_DAMAGE: f32 = 1.0;
/// Vanilla `Dolphin.BABY_SCALE`.
const BABY_AGE_SCALE: f32 = 0.65;
/// Vanilla `Dolphin.BABY_DIMENSIONS`: the adult box scaled by 0.65 with eye height 0.09375.
const DOLPHIN_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.3125, 0.0)];
const DOLPHIN_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    0.585,
    0.39,
    0.09375,
    EntityAttachments::new(&DOLPHIN_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);
/// Vanilla `Dolphin.finalizeSpawn` default `AgeableMobGroupData` baby chance.
const DOLPHIN_BABY_SPAWN_CHANCE: f32 = 0.1;
/// Vanilla `Dolphin.tick` ground bounce impulse while drying out.
const DRY_BOUNCE_IMPULSE: f64 = 0.2;
const DRY_BOUNCE_UP: f64 = 0.5;
/// Vanilla `Dolphin.tick` full yaw spin while drying out on land.
const DRY_BOUNCE_FULL_YAW: f32 = 360.0;
/// Vanilla `Dolphin.DolphinSwimWithPlayerGoal` trigger/keep range.
const SWIM_WITH_PLAYER_RANGE: f64 = 10.0;
const SWIM_WITH_PLAYER_KEEP_SQR: f64 = 256.0;
/// Vanilla `Dolphin.DolphinSwimWithPlayerGoal` stop distance squared.
const SWIM_WITH_PLAYER_STOP_SQR: f64 = 6.25;
/// Vanilla `Dolphin.DolphinSwimWithPlayerGoal` speed modifier.
const SWIM_WITH_PLAYER_SPEED: f64 = 1.2;
/// Vanilla `Dolphin.DolphinSwimWithPlayerGoal` grace refresh divisor.
const SWIM_WITH_PLAYER_GRACE_REFRESH: i32 = 6;
/// Vanilla `MobEffects.DOLPHINS_GRACE` duration granted by the dolphin.
const DOLPHINS_GRACE_DURATION: i32 = 100;
/// Vanilla `Dolphin.closeToNextPos` distance.
const CLOSE_TO_NEXT_POS_DISTANCE: f64 = 12.0;
/// Vanilla `Dolphin.ItemGoal` search box inflation.
const ITEM_SEARCH_RADIUS: f64 = 8.0;
/// Vanilla `Dolphin.MoveToItemGoal` speed modifier.
const ITEM_GOAL_SPEED: f64 = 1.2;
/// Vanilla `Dolphin.MoveToItemGoal` cooldown roll range.
const ITEM_GOAL_COOLDOWN_RANGE: i32 = 100;
/// Vanilla `Dolphin.ItemGoal.drop` throw impulse and pickup delay.
const ITEM_DROP_IMPULSE: f64 = 0.3;
const ITEM_DROP_Y_FACTOR: f64 = 1.5;
const ITEM_DROP_JITTER: f64 = 0.02;
const ITEM_DROP_PICKUP_DELAY: i32 = 40;
const ITEM_DROP_EYE_OFFSET: f64 = 0.3;
/// Vanilla `Dolphin`: `new SmoothSwimmingMoveControl<>(this, 85, 10, 0.02F, 0.1F, true)`.
const DOLPHIN_MOVE_CONTROL: SmoothSwimmingMoveControl =
    SmoothSwimmingMoveControl::new(85, 10, 0.02, 0.1, true);
/// Vanilla `Dolphin`: `new SmoothSwimmingLookControl(this, 10)`.
const DOLPHIN_LOOK_CONTROL: SmoothSwimmingLookControl = SmoothSwimmingLookControl::new(10);

/// Vanilla `Dolphin`.
#[entity_behavior(class = "Dolphin")]
pub struct DolphinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<DolphinEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `DolphinEntity`.
unsafe impl DowncastType for DolphinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/dolphin");
}

impl DolphinEntity {
    /// Creates a new dolphin entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved dolphin entity.
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
        // Vanilla `Dolphin` constructor: `setCanPickUpLoot(true)`.
        *mob_base.can_pick_up_loot().lock() = true;
        // Vanilla `Dolphin.createNavigation` -> `WaterBoundPathNavigation` with
        // `allowBreaching = mob.is(EntityTypes.DOLPHIN)`, plus the vanilla
        // smooth-swimming control pair (`Dolphin.java:92-93`).
        {
            let mut navigation = mob_base.navigation().lock();
            navigation.set_kind(NavigationKind::WaterBound);
            navigation.set_allow_breaching(true);
        }
        // Vanilla `AgeableWaterCreature`: `setPathfindingMalus(WATER, 0.0F)`.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, 0.0);
        let mut entity_data = DolphinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.got_fish.set(false);
        entity_data.moistness_level.set(MAX_MOISTNESS);

        // Vanilla `Dolphin.registerGoals`. The treasure goal (priority 1) needs the
        // structure locator, which Steel does not have yet; see the module docs.
        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, BreathAirGoal::new());
            goal_selector.add_goal(0, TryFindWaterGoal::new());
            goal_selector.add_goal(2, DolphinSwimWithPlayerGoal::new(SWIM_WITH_PLAYER_SPEED));
            goal_selector.add_goal(4, RandomSwimmingGoal::new(1.0, 10));
            goal_selector.add_goal(4, RandomLookAroundGoal::new());
            goal_selector.add_goal(5, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(5, DolphinJumpGoal::new(10));
            goal_selector.add_goal(6, MeleeAttackGoal::new(1.2, true));
            goal_selector.add_goal(7, MoveToItemGoal::new());
            goal_selector.add_goal(8, PlayWithItemsGoal::new());
            goal_selector.add_goal(8, FollowPlayerRiddenEntityGoal::new_matching(is_boat_type));
            goal_selector.add_goal(
                8,
                FollowPlayerRiddenEntityGoal::new_matching(is_nautilus_type),
            );
            goal_selector.add_goal(
                9,
                AvoidEntityGoal::with_selector(8.0, 1.0, 1.0, |target, _world| {
                    target.entity_type() == &vanilla_entities::GUARDIAN
                        || target.entity_type() == &vanilla_entities::ELDER_GUARDIAN
                }),
            );

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
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
    }

    /// Vanilla `Dolphin.gotFish`.
    #[must_use]
    pub fn got_fish(&self) -> bool {
        *self.entity_data.lock().got_fish.get()
    }

    /// Vanilla `Dolphin.setGotFish`.
    pub fn set_got_fish(&self, got_fish: bool) {
        self.entity_data.lock().got_fish.set(got_fish);
    }

    /// Vanilla `Dolphin.getMoistnessLevel`.
    #[must_use]
    pub fn moistness_level(&self) -> i32 {
        *self.entity_data.lock().moistness_level.get()
    }

    /// Vanilla `Dolphin.setMoisntessLevel`.
    pub fn set_moistness_level(&self, level: i32) {
        self.entity_data.lock().moistness_level.set(level);
    }

    /// Vanilla `Dolphin.tick` moistness and dry-out damage.
    fn tick_moistness(&self, entity: &SharedEntity) {
        if self.is_no_ai() {
            self.set_air_supply(Self::max_air_supply());
            return;
        }

        // Vanilla `isInWaterOrRain`.
        if self.is_in_water() || self.is_in_rain_now() {
            self.set_moistness_level(MAX_MOISTNESS);
            return;
        }

        self.set_moistness_level(self.moistness_level() - 1);
        if self.moistness_level() <= 0
            && let Some(world) = self.level()
        {
            let source = DamageSource::environment(&vanilla_damage_types::DRY_OUT);
            let _ = entity.hurt(&world, &source, DRY_OUT_DAMAGE);
        }

        // Vanilla `Dolphin.tick`: drying-out dolphins bounce on the ground.
        if self.on_ground() {
            let velocity = self.velocity();
            self.set_velocity(DVec3::new(
                velocity.x + (f64::from(rand::random::<f32>() * 2.0 - 1.0)) * DRY_BOUNCE_IMPULSE,
                DRY_BOUNCE_UP,
                velocity.z + (f64::from(rand::random::<f32>() * 2.0 - 1.0)) * DRY_BOUNCE_IMPULSE,
            ));
            let (_, pitch) = self.rotation();
            self.set_rotation((rand::random::<f32>() * DRY_BOUNCE_FULL_YAW, pitch));
            self.set_on_ground(false);
        }
    }

    /// Vanilla `Dolphin.closeToNextPos`: the navigation target is within 12 blocks.
    #[must_use]
    pub fn close_to_next_pos(&self) -> bool {
        let navigation = self.mob_base.navigation().lock();
        navigation.target_pos().is_some_and(|target| {
            target.closer_to_center_than(self.position(), CLOSE_TO_NEXT_POS_DISTANCE)
        })
    }

    /// Vanilla `Entity.isInRain`: raining at the entity's block position.
    fn is_in_rain_now(&self) -> bool {
        self.level()
            .is_some_and(|world| world.is_raining_at(self.block_position()))
    }

    /// Vanilla `Dolphin.getMaxAirSupply`.
    #[must_use]
    pub const fn max_air_supply() -> i32 {
        MAX_AIR_SUPPLY
    }

    /// Vanilla `AgeableWaterCreature.checkSurfaceAgeableWaterCreatureSpawnRules`, the
    /// `SpawnPlacements` predicate for `EntityType.DOLPHIN`.
    ///
    /// Vanilla ignores `entity_type` and `random`; the position must be within 13
    /// blocks below sea level, the block below must be water and the block above
    /// must be a full water block.
    #[must_use]
    pub fn check_surface_ageable_water_creature_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut RandomSource,
    ) -> bool {
        let min_spawn_level = level.sea_level() - 13;
        pos.y() >= min_spawn_level
            && pos.y() <= level.sea_level
            && level
                .get_block_state(pos.below())
                .get_fluid_state()
                .is_water()
            && level.get_block_state(pos.above()).get_block() == &vanilla_blocks::WATER
    }
}

impl Entity for DolphinEntity {
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
        self.tick_moistness(&entity);
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            DOLPHIN_BABY_DIMENSIONS.scale(scale)
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

    /// Vanilla `Dolphin.getMaxAirSupply`.
    fn max_air_supply(&self) -> i32 {
        MAX_AIR_SUPPLY
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_DOLPHIN_SWIM
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("GotFish", i8::from(self.got_fish()));
        nbt.insert("Moistness", self.moistness_level());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_got_fish(nbt.byte("GotFish").is_some_and(|value| value != 0));
        self.set_moistness_level(nbt.int("Moistness").unwrap_or(MAX_MOISTNESS));
    }
}

impl LivingEntity for DolphinEntity {
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

    /// Vanilla `Dolphin.canDispenserEquipIntoSlot` always returns `false`.
    fn can_dispenser_equip_into_slot(&self, _slot: EquipmentSlot) -> bool {
        false
    }

    /// Vanilla `Dolphin.getAgeScale`.
    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            BABY_AGE_SCALE
        } else {
            1.0
        }
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_DOLPHIN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_DOLPHIN_DEATH)
    }

    /// Vanilla `Dolphin.increaseAirSupply`: refill straight to the maximum.
    fn increase_air_supply(&self, _current_supply: i32) -> i32 {
        MAX_AIR_SUPPLY
    }

    /// Vanilla `Dolphin.handleAirSupply` is empty: the moistness timer in
    /// [`DolphinEntity::tick_moistness`] is the dolphin's only drying-out damage, so
    /// air supply is never decremented.
    fn tick_living_air_supply(&self) {
        if !LivingEntity::is_alive(self) {
            return;
        }

        if self
            .vehicle()
            .is_some_and(|vehicle| vehicle.dismounts_underwater())
        {
            self.stop_riding();
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
        // Vanilla `Dolphin.travelInWater`.
        self.move_relative(self.get_speed(), input);
        let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, self.velocity())?;
        self.set_velocity(self.velocity() * 0.9);
        if self.target().is_none() {
            self.set_velocity(self.velocity() + DVec3::new(0.0, -0.005, 0.0));
        }
        Some(result)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.mob_ai_step(entity)
    }
}

impl Mob for DolphinEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Dolphin`'s `moveControl = new SmoothSwimmingMoveControl<>(this, 85, 10, 0.02F, 0.1F, true)`.
    fn tick_move_control(&self) {
        tick_smooth_swimming_move_control(self, DOLPHIN_MOVE_CONTROL);
    }

    /// Vanilla `Dolphin`'s `lookControl = new SmoothSwimmingLookControl(this, 10)`.
    fn tick_look_control(&self) {
        tick_smooth_swimming_look_control(self, DOLPHIN_LOOK_CONTROL);
    }

    /// Vanilla `Dolphin.getAmbientSound` depends on being in water.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_in_water() {
            &sound_events::ENTITY_DOLPHIN_AMBIENT_WATER
        } else {
            &sound_events::ENTITY_DOLPHIN_AMBIENT
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Dolphin.canAttack`: babies never attack.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        // Vanilla `Mob.canAttack`: ghasts are never targeted directly.
        !AgeableMob::is_baby(self)
            && target.entity_type() != &vanilla_entities::GHAST
            && LivingEntity::can_attack(self, target)
    }

    /// Vanilla `Dolphin.playAttackSound`.
    fn play_attack_sound(&self) {
        self.play_sound(&sound_events::ENTITY_DOLPHIN_ATTACK, 1.0, 1.0);
    }

    /// Vanilla `Dolphin.getMaxHeadXRot`.
    fn max_head_x_rot(&self) -> f32 {
        1.0
    }

    /// Vanilla `Dolphin.getMaxHeadYRot`.
    fn max_head_y_rot(&self) -> f32 {
        1.0
    }

    /// Vanilla `Dolphin.canBeLeashed`.
    fn mob_can_be_leashed(&self) -> bool {
        true
    }

    /// Vanilla `AgeableWaterCreature.checkSpawnObstruction` is
    /// `level.isUnobstructed(this)`; the shared `Mob` default would reject the water
    /// the dolphin spawns in.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        check_water_animal_spawn_obstruction(world, self)
    }

    /// Vanilla `Dolphin.finalizeSpawn`: fill the air supply, level the pitch and
    /// default the group data to a 0.1 baby chance.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_air_supply(Self::max_air_supply());
        let (yaw, _) = self.rotation();
        self.set_rotation((yaw, 0.0));

        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::AgeableMob(AgeableMobGroupData::new(
                true,
                DOLPHIN_BABY_SPAWN_CHANCE,
            )))
        });
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    /// Vanilla `Dolphin.mobInteract`: feeding a fish either ages a baby up or
    /// sets the `GotFish` flag that starts the treasure hunt.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held_is_fish = {
            let inventory = player.inventory.lock();
            let stack = inventory.get_item_in_hand(hand);
            !stack.is_empty() && stack.item().has_tag(&vanilla_item_tags::ItemTag::FISHES)
        };

        if held_is_fish {
            self.play_sound(&sound_events::ENTITY_DOLPHIN_EAT, 1.0, 1.0);
            if AgeableMob::can_age_up(self) {
                Mob::use_player_item(self, player, hand);
                let ticks_until_adult = -self.get_age();
                let seconds = AgeableMobBase::get_speed_up_seconds_when_feeding(ticks_until_adult);
                AgeableMob::age_up(self, seconds, true);
            } else {
                self.set_got_fish(true);
                Mob::use_player_item(self, player, hand);
            }
            return InteractionResult::Success;
        }

        Animal::mob_interact_animal(self, player, hand)
    }
}

impl AgeableMob for DolphinEntity {
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
}

impl Animal for DolphinEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn spawn_child_from_breeding(&self, world: &Arc<World>, partner: &dyn Animal) {
        // Vanilla `Dolphin.getBreedOffspring`.
        self.finalize_spawn_child_from_breeding(world, partner, None);
    }

    /// Vanilla `Dolphin.isFood` accepts `#minecraft:fishes` (dolphins eat fish).
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is_empty()
            && item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::FISHES)
    }
}
impl PathfinderMob for DolphinEntity {}

/// Vanilla `FollowPlayerRiddenEntityGoal(AbstractBoat)`: matches every boat and raft.
fn is_boat_type(entity_type: EntityTypeRef) -> bool {
    let path = entity_type.key.path.as_ref();
    path.ends_with("_boat") || path.ends_with("_raft")
}

/// Vanilla `FollowPlayerRiddenEntityGoal(AbstractNautilus)`.
fn is_nautilus_type(entity_type: EntityTypeRef) -> bool {
    ptr::eq(entity_type, &raw const vanilla_entities::NAUTILUS)
        || ptr::eq(entity_type, &raw const vanilla_entities::ZOMBIE_NAUTILUS)
}

/// Vanilla `Dolphin.DolphinSwimWithPlayerGoal`: an unridden dolphin escorts a
/// swimming player and keeps `DOLPHINS_GRACE` applied.
pub(super) struct DolphinSwimWithPlayerGoal {
    speed_modifier: f64,
    player: Option<SharedEntity>,
}

impl DolphinSwimWithPlayerGoal {
    pub(super) const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            player: None,
        }
    }

    fn dolphin(mob: &dyn PathfinderMob) -> Option<&DolphinEntity> {
        mob.as_entity_event_source().downcast_ref::<DolphinEntity>()
    }

    /// Vanilla `MobEffectInstance(MobEffects.DOLPHINS_GRACE, 100)`.
    fn apply_grace(player: &SharedEntity) {
        let Some(living) = player.as_living_entity() else {
            return;
        };
        living.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::DOLPHINS_GRACE,
            DOLPHINS_GRACE_DURATION,
            0,
        ));
    }
}

impl Goal for DolphinSwimWithPlayerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(dolphin) = Self::dolphin(mob) else {
            return false;
        };
        let Some(world) = dolphin.level() else {
            return false;
        };

        // Vanilla `SWIM_WITH_PLAYER_TARGETING`: non-combat, range 10, no line of sight.
        let Some(player) =
            world.nearest_player(dolphin.position(), SWIM_WITH_PLAYER_RANGE, |player| {
                !player.is_spectator()
            })
        else {
            return false;
        };

        if !player.is_swimming() {
            return false;
        }
        if dolphin
            .target()
            .is_some_and(|target| target.id() == player.id())
        {
            return false;
        }

        let player_entity: SharedEntity = player;
        self.player = Some(player_entity);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(dolphin) = Self::dolphin(mob) else {
            return false;
        };
        let Some(player) = self.player.as_ref() else {
            return false;
        };

        player.is_swimming()
            && dolphin.position().distance_squared(player.position()) < SWIM_WITH_PLAYER_KEEP_SQR
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        if let Some(player) = self.player.as_ref() {
            Self::apply_grace(player);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.player = None;
        if let Some(dolphin) = Self::dolphin(mob) {
            dolphin.mob_base().navigation().lock().stop();
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(dolphin) = Self::dolphin(mob) else {
            return;
        };
        let Some(player) = self.player.clone() else {
            return;
        };

        dolphin
            .mob_base()
            .controls()
            .lock()
            .look_control
            .set_look_at(
                player.position(),
                dolphin.max_head_y_rot() + 20.0,
                dolphin.max_head_x_rot(),
            );

        if dolphin.position().distance_squared(player.position()) < SWIM_WITH_PLAYER_STOP_SQR {
            dolphin.mob_base().navigation().lock().stop();
        } else {
            dolphin.move_to_pos(player.position(), self.speed_modifier);
        }

        if player.is_swimming() && rand::random_range(0..SWIM_WITH_PLAYER_GRACE_REFRESH) == 0 {
            Self::apply_grace(&player);
        }
    }
}

/// Vanilla `Dolphin.ItemGoal.drop`: toss the held stack forward out of the mouth.
fn dolphin_drop_held_item(dolphin: &DolphinEntity) -> bool {
    let mut stack = ItemStack::empty();
    dolphin.with_equipment_slot(EquipmentSlot::MainHand, &mut |item| stack = item.clone());
    if stack.is_empty() {
        return false;
    }

    let Some(world) = dolphin.level() else {
        return false;
    };

    let (yaw, pitch) = dolphin.rotation();
    let yaw_rad = f64::from(yaw.to_radians());
    let pitch_rad = f64::from(pitch.to_radians());
    let direction = rand::random::<f32>() * TAU;
    let jitter = ITEM_DROP_JITTER * f64::from(rand::random::<f32>());
    let velocity = DVec3::new(
        ITEM_DROP_IMPULSE * -yaw_rad.sin() * pitch_rad.cos() + f64::from(direction.cos()) * jitter,
        ITEM_DROP_IMPULSE * pitch_rad.sin() * ITEM_DROP_Y_FACTOR,
        ITEM_DROP_IMPULSE * yaw_rad.cos() * pitch_rad.cos() + f64::from(direction.sin()) * jitter,
    );

    let eye_position =
        dolphin.position() + DVec3::new(0.0, dolphin.get_eye_height() - ITEM_DROP_EYE_OFFSET, 0.0);
    if let Some(item_entity) = world.spawn_item_with_velocity(eye_position, stack, velocity) {
        item_entity.set_pickup_delay(ITEM_DROP_PICKUP_DELAY);
        item_entity.set_thrower(dolphin.uuid());
    }

    dolphin.with_equipment_slot_mut(EquipmentSlot::MainHand, &mut |item| {
        *item = ItemStack::empty();
    });
    true
}

/// Vanilla `Dolphin.MoveToItemGoal`: swim to a nearby floating item.
pub(super) struct MoveToItemGoal {
    cooldown: i32,
}

impl MoveToItemGoal {
    pub(super) const fn new() -> Self {
        Self { cooldown: 0 }
    }

    fn dolphin(mob: &dyn PathfinderMob) -> Option<&DolphinEntity> {
        mob.as_entity_event_source().downcast_ref::<DolphinEntity>()
    }

    /// Vanilla `Dolphin.ALLOWED_ITEMS`: no pickup delay, alive, in water.
    fn nearby_items(dolphin: &DolphinEntity) -> Vec<SharedEntity> {
        let Some(world) = dolphin.level() else {
            return Vec::new();
        };
        let search_box = dolphin.bounding_box().inflate(ITEM_SEARCH_RADIUS);
        world.get_entities_in_aabb_matching(&search_box, |entity| {
            entity.downcast_ref::<ItemEntity>().is_some_and(|item| {
                !item.has_pickup_delay() && entity.is_alive() && entity.is_in_water()
            })
        })
    }
}

impl Goal for MoveToItemGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(dolphin) = Self::dolphin(mob) else {
            return false;
        };
        if self.cooldown > dolphin.tick_count() {
            return false;
        }
        !Self::nearby_items(dolphin).is_empty()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(dolphin) = Self::dolphin(mob) else {
            return;
        };
        if let Some(first) = Self::nearby_items(dolphin).first() {
            dolphin.move_to_pos(first.position(), ITEM_GOAL_SPEED);
            dolphin.play_sound(&sound_events::ENTITY_DOLPHIN_PLAY, 1.0, 1.0);
        }
        self.cooldown = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let Some(dolphin) = Self::dolphin(mob) else {
            return;
        };
        if dolphin_drop_held_item(dolphin) {
            self.cooldown = dolphin.tick_count() + rand::random_range(0..ITEM_GOAL_COOLDOWN_RANGE);
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(dolphin) = Self::dolphin(mob) else {
            return;
        };
        if let Some(first) = Self::nearby_items(dolphin).first() {
            dolphin.move_to_pos(first.position(), ITEM_GOAL_SPEED);
        }
    }
}

/// Vanilla `Dolphin.PlayWithItemsGoal`: while an item is held, toss it away.
pub(super) struct PlayWithItemsGoal;

impl PlayWithItemsGoal {
    pub(super) const fn new() -> Self {
        Self
    }

    fn dolphin(mob: &dyn PathfinderMob) -> Option<&DolphinEntity> {
        mob.as_entity_event_source().downcast_ref::<DolphinEntity>()
    }
}

impl Goal for PlayWithItemsGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(dolphin) = Self::dolphin(mob) else {
            return false;
        };
        let mut occupied = false;
        dolphin.with_equipment_slot(EquipmentSlot::MainHand, &mut |item| {
            occupied = !item.is_empty();
        });
        occupied
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(dolphin) = Self::dolphin(mob) {
            dolphin_drop_held_item(dolphin);
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if let Some(dolphin) = Self::dolphin(mob) {
            dolphin_drop_held_item(dolphin);
        }
    }
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::{DolphinEntity, MAX_MOISTNESS};
    use crate::entity::Entity;

    #[test]
    fn dolphin_moistness_and_fish_flags_round_trip() {
        init_vanilla_registry();
        let dolphin = DolphinEntity::new(&vanilla_entities::DOLPHIN, 1, DVec3::ZERO, Weak::new());

        assert_eq!(dolphin.moistness_level(), MAX_MOISTNESS);
        assert!(!dolphin.got_fish());
        dolphin.set_got_fish(true);
        dolphin.set_moistness_level(100);

        let mut nbt = NbtCompound::new();
        dolphin.save_additional(&mut nbt);
        assert_eq!(nbt.int("Moistness"), Some(100));
        assert_eq!(nbt.byte("GotFish"), Some(1));
        assert_eq!(dolphin.entity_type(), &vanilla_entities::DOLPHIN);
    }
}
