//! Vanilla `Ocelot` entity: wild cat that can be fed into trusting the player.

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
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::OcelotEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_blocks, vanilla_entities,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::{Random as _, RandomSource};
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::control::MoveControlOperation;
use crate::entity::ai::goal::{
    BreedGoal, FloatGoal, LeapAtTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal,
    OcelotAttackGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntityEventSource as _, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData,
    aabb_contains_any_liquid,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::{LevelReader, World};

use goals::{OcelotAvoidEntityGoal, OcelotTemptGoal};

mod goals;

const OCELOT_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.3125, 0.0)];
const OCELOT_BABY_WIDTH: f32 = 0.3;
const OCELOT_BABY_HEIGHT: f32 = 0.35;
const OCELOT_BABY_EYE_HEIGHT: f32 = 0.34375;

const OCELOT_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    OCELOT_BABY_WIDTH,
    OCELOT_BABY_HEIGHT,
    OCELOT_BABY_EYE_HEIGHT,
    EntityAttachments::new(&OCELOT_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `Ocelot.CROUCH_SPEED_MOD`.
const CROUCH_SPEED_MOD: f64 = 0.6;
/// The same crouch speed vanilla checks in `Ocelot.customServerAiStep`.
const MOVEMENT_SPEED_MOD: f64 = CROUCH_SPEED_MOD;
/// Vanilla `Ocelot.WALK_SPEED_MOD`.
const WALK_SPEED_MOD: f64 = 0.8;
/// Vanilla `Ocelot.SPRINT_SPEED_MOD`.
const SPRINT_SPEED_MOD: f64 = 1.33;
/// Vanilla `Ocelot.getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 900;
/// Vanilla `Ocelot.mobInteract`: trusting needs the player within three blocks.
const TRUST_INTERACT_RANGE_SQR: f64 = 9.0;
/// Vanilla `Ocelot.checkOcelotSpawnRules` uses `random.nextInt(3) != 0`; see
/// [`OcelotEntity::check_ocelot_spawn_rules`].

#[entity_behavior(class = "Ocelot")]
/// Vanilla ocelot: an untameable wild cat with a trusting state.
pub struct OcelotEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<OcelotEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `OcelotEntity`.
unsafe impl DowncastType for OcelotEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ocelot");
}

impl OcelotEntity {
    /// Creates a new ocelot at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs an ocelot from persisted base entity state.
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
        let mut entity_data = OcelotEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `Ocelot.registerGoals`, including the avoid-players goal that
            // `reassessTrustingGoals` keeps while the ocelot is untrusting.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, FloatGoal::new(&mob_base));
            goal_selector.add_goal(
                3,
                OcelotTemptGoal::new(CROUCH_SPEED_MOD, |item_stack| {
                    REGISTRY
                        .items
                        .is_in_tag(item_stack.item(), &ItemTag::OCELOT_FOOD)
                }),
            );
            goal_selector.add_goal(
                4,
                OcelotAvoidEntityGoal::new(16.0, WALK_SPEED_MOD, SPRINT_SPEED_MOD),
            );
            goal_selector.add_goal(7, LeapAtTargetGoal::new(0.3));
            goal_selector.add_goal(8, OcelotAttackGoal::new());
            goal_selector.add_goal(9, BreedGoal::new(0.8));
            goal_selector.add_goal(10, WaterAvoidingRandomStrollGoal::new(0.8));
            goal_selector.add_goal(11, LookAtPlayerGoal::new(10.0));
        }

        {
            // Vanilla target goals: chickens, and baby turtles on land.
            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(
                1,
                NearestAttackableTargetGoal::new(false, |target, _| {
                    target.entity_type() == &vanilla_entities::CHICKEN
                }),
            );
            target_selector.add_goal(
                1,
                NearestAttackableTargetGoal::new_with_interval(10, false, false, |target, _| {
                    target.entity_type() == &vanilla_entities::TURTLE
                        && target.as_ageable_mob().is_some_and(AgeableMob::is_baby)
                        && !target.is_in_water()
                }),
            );
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

    /// Vanilla `Ocelot.isTrusting`.
    #[must_use]
    pub fn is_trusting(&self) -> bool {
        *self.entity_data.lock().trusting.get()
    }

    /// Vanilla `Ocelot.setTrusting`.
    pub fn set_trusting(&self, trusting: bool) {
        self.entity_data.lock().trusting.set(trusting);
    }

    /// Vanilla `Ocelot.isFood`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::OCELOT_FOOD)
    }

    /// Vanilla `Ocelot.spawnTrustingParticles`.
    ///
    /// GAP: particles are client-side in Steel; the heart/smoke burst is not emitted here.
    fn spawn_trusting_particles(&self, _success: bool) {}

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

    /// Vanilla `Ocelot.mobInteract`: feeding an untrusting ocelot may earn its trust.
    fn try_gain_trust(&self, player: &Player, hand: InteractionHand) -> bool {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if self.is_trusting()
            || !OcelotEntity::is_food(&item_stack)
            || player.position().distance_squared(self.position()) >= TRUST_INTERACT_RANGE_SQR
        {
            return false;
        }

        Mob::use_player_item(self, player, hand);
        if rand::random_range(0..3) == 0 {
            self.set_trusting(true);
            self.spawn_trusting_particles(true);
            self.broadcast_entity_event(EntityStatus::TrustingSucceeded);
        } else {
            self.spawn_trusting_particles(false);
            self.broadcast_entity_event(EntityStatus::TrustingFailed);
        }
        true
    }

    /// Vanilla `Ocelot.checkOcelotSpawnRules`, the `SpawnPlacements` predicate for
    /// `EntityType.OCELOT`.
    #[must_use]
    pub fn check_ocelot_spawn_rules(
        _entity_type: EntityTypeRef,
        _level: &World,
        _spawn_reason: EntitySpawnReason,
        _pos: BlockPos,
        random: &mut RandomSource,
    ) -> bool {
        random.next_i32_bounded(3) != 0
    }
}

impl Entity for OcelotEntity {
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
            OCELOT_BABY_DIMENSIONS.scale(scale)
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

    fn play_step_sound(&self, _pos: BlockPos, block_state: BlockStateId) {
        self.play_block_step_sound(block_state);
    }

    fn is_stepping_carefully(&self) -> bool {
        // Vanilla `Ocelot.isSteppingCarefully`.
        self.is_crouching() || self.is_suppressing_bounce()
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        // Vanilla `Ocelot.addAdditionalSaveData`.
        nbt.insert("Trusting", i8::from(self.is_trusting()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);

        self.set_trusting(nbt.byte("Trusting").is_some_and(|value| value != 0));
    }
}

impl LivingEntity for OcelotEntity {
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
        Some(&sound_events::ENTITY_OCELOT_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_OCELOT_DEATH)
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

impl AgeableMob for OcelotEntity {
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

impl Animal for OcelotEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        OcelotEntity::is_food(item_stack)
    }

    fn remove_when_far_away_animal(&self, _dist_sqr: f64) -> bool {
        // Vanilla `Ocelot.removeWhenFarAway`.
        !self.is_trusting() && self.tick_count() > 2400
    }
}

impl Mob for OcelotEntity {
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

        // Vanilla pose switch driven by the move-control speed modifier.
        let wanted_speed = {
            let controls = self.mob_base.controls().lock();
            let move_control = controls.move_control;
            (move_control.operation() == MoveControlOperation::MoveTo)
                .then(|| move_control.speed_modifier())
        };

        match wanted_speed {
            Some(speed) if speed == MOVEMENT_SPEED_MOD => {
                self.set_pose(EntityPose::Sneaking);
                self.set_sprinting(false);
            }
            Some(speed) if speed == SPRINT_SPEED_MOD => {
                self.set_pose(EntityPose::Standing);
                self.set_sprinting(true);
            }
            _ => {
                self.set_pose(EntityPose::Standing);
                self.set_sprinting(false);
            }
        }
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_OCELOT_AMBIENT)
    }

    fn ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        // Vanilla `Ocelot.checkSpawnObstruction`: unobstructed, dry, above sea level and
        // standing on grass or leaves.
        //
        // The `Mob::check_spawn_obstruction` default body is inlined: a UFCS call
        // here would resolve back to this override and recurse.
        let aabb = self.bounding_box();
        if aabb_contains_any_liquid(world, aabb) {
            return false;
        }
        let obstructed = world.has_entity_in_aabb_matching(&aabb, |entity| {
            entity.id() != self.id()
                && !entity.is_removed()
                && !entity.is_spectator()
                && entity.blocks_building()
                && !entity.is_passenger_of_same_vehicle(self.as_entity_event_source())
        });
        if obstructed {
            return false;
        }

        let pos = self.block_position();
        if pos.y() < LevelReader::sea_level(world.as_ref()) {
            return false;
        }

        let below = world.get_block_state(pos.below()).get_block();
        below == &vanilla_blocks::GRASS_BLOCK
            || below.has_tag(&steel_registry::vanilla_block_tags::BlockTag::LEAVES)
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
        if self.try_gain_trust(player, hand) {
            return InteractionResult::Success;
        }

        Animal::mob_interact_animal(self, player, hand)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for OcelotEntity {}
