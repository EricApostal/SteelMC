//! Vanilla `Camel`: a two-seater mount with a dash, a sitting pose and a brain.
//!
//! Mirrors `net.minecraft.world.entity.animal.camel.Camel` (which extends
//! `AbstractHorse`). Vanilla registers **no** goals (`Camel.registerGoals()` is
//! empty) and drives camels entirely from `CamelAi`'s `CORE`/`IDLE` packages; the
//! brain provider lives in [`brain`] and `Camel.customServerAiStep` is the
//! `brain_tick` hook.
//!
//! Vanilla's `AbstractHorse` layer is shared with the horse family and lives in
//! [`crate::entity::entities::mobs::passive::equine`]; this module reuses its
//! [`AbstractHorseParts`] state plus `Tame`/`Temper`/`Owner`/`EatingHaystack`/
//! `Bred` persistence, and adds the camel-only dash, sitting pose and two-rider
//! seat.
//!
//! ## Deviations
//!
//! - The rider jump (`PlayerRideableJumping.onPlayerJump`/`handleStartJump`) has
//!   no Steel packet hook, so [`CamelEntity::dash_cooldown`] is never started by a
//!   dash. Every other dash path (the synced `DASH` flag, the cooldown countdown
//!   and the dash-ready sound) is implemented.
//! - `Camel.wouldNotSuffocateAtTargetPose` needs a block-shape query against the
//!   *other* pose's eye box; Steel has no such helper yet, so
//!   [`CamelEntity::can_camel_change_pose`] always allows the change.
//! - `Camel.getLeashOffset`/`getQuadLeashOffsets` have no hook on `Leashable`;
//!   `onElasticLeashPull` (stand up) is implemented.
//! - The `sitAnimationState`/`dashAnimationState` family, `setupAnimationStates`
//!   and `updateWalkAnimation` are client render state.
//! - GAP(horse-inventory): `Camel.openCustomInventoryScreen` needs the horse menu
//!   owned by the Menus slice; [`CamelEntity::open_custom_inventory_screen`] logs
//!   and returns, and the saddle persists as equipment.
//! - GAP(camel-eat-grass): vanilla camels inherit `AbstractHorse.aiStep`'s
//!   eating/standing counters; no camel surface reads them, so they are not ticked.

mod brain;

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_data::EntityPose;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::CamelEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_custom_stats,
    vanilla_game_events, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::passive::equine::{
    AbstractHorseParts, load_abstract_horse, save_abstract_horse,
};
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `Camel.DASH_COOLDOWN_TICKS`.
const DASH_COOLDOWN_TICKS: i32 = 55;
/// Vanilla `Camel.tick`: the dash flag is cleared below this cooldown value.
const DASH_MINIMUM_DURATION_TICKS: i32 = 50;
/// Vanilla `Camel.RUNNING_SPEED_BONUS`.
const RUNNING_SPEED_BONUS: f32 = 0.1;
/// Vanilla `Camel.SITDOWN_DURATION_TICKS`.
const SITDOWN_DURATION_TICKS: i64 = 40;
/// Vanilla `Camel.STANDUP_DURATION_TICKS`.
const STANDUP_DURATION_TICKS: i64 = 52;
/// Vanilla `Camel.SITTING_HEIGHT_DIFFERENCE`.
const SITTING_HEIGHT_DIFFERENCE: f32 = 1.43;
/// Vanilla `Camel.MAX_HEAD_Y_ROT`.
const MAX_HEAD_Y_ROT: f32 = 30.0;
/// Vanilla `Camel.getAgeScale` baby scale.
const BABY_SCALE: f32 = 0.6;
/// Vanilla `AbstractHorse.getSoundVolume`.
const SOUND_VOLUME: f32 = 0.8;
/// Vanilla `AbstractHorse.getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 400;
/// Vanilla `Camel.DEFAULT_LAST_POSE_CHANGE_TICK`.
const DEFAULT_LAST_POSE_CHANGE_TICK: i64 = 0;
/// Vanilla `LivingEntity.getAttributeValue(Attributes.STEP_HEIGHT)` fallback.
const DEFAULT_STEP_HEIGHT: f32 = 1.5;

/// Vanilla `Camel.BABY_STANDING_DIMENSIONS`.
const CAMEL_BABY_STANDING_DIMENSIONS: EntityDimensions = EntityDimensions::new(0.95, 1.4, 1.38);
/// Vanilla `Camel.BABY_SITTING_DIMENSIONS`.
const CAMEL_BABY_SITTING_DIMENSIONS: EntityDimensions = EntityDimensions::new(0.95, 0.425, 0.41);
/// Vanilla `Camel.ADULT_SITTING_DIMENSIONS`: `EntityTypes.CAMEL` minus
/// [`SITTING_HEIGHT_DIFFERENCE`], with vanilla's fixed sitting eye height.
const CAMEL_ADULT_SITTING_DIMENSIONS: EntityDimensions =
    EntityDimensions::new(1.7, 2.375 - SITTING_HEIGHT_DIFFERENCE, 0.845);

/// Vanilla `Camel` runtime fields that are not synchronized entity data.
#[derive(Debug, Clone, Copy, Default)]
struct CamelState {
    /// Vanilla `Camel.dashCooldown`.
    dash_cooldown: i32,
}

#[entity_behavior(class = "Camel")]
/// Vanilla camel with dash, sitting-pose, two-rider seat and brain state.
pub struct CamelEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    /// Vanilla `AbstractHorse` non-synchronized state (temper, owner, …).
    parts: AbstractHorseParts,
    entity_data: SyncMutex<CamelEntityData>,
    brain: SyncMutex<Brain>,
    state: SyncMutex<CamelState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CamelEntity`.
unsafe impl DowncastType for CamelEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/camel");
}

impl CamelEntity {
    /// Creates a new camel at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a camel from persisted base entity state.
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
        let mut entity_data = CamelEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            // Vanilla `Camel` never gallops and keeps `AbstractHorse`'s temper/owner.
            parts: AbstractHorseParts::new(false),
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(Brain::default()),
            state: SyncMutex::new(CamelState::default()),
        };
        // Vanilla `Camel.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        *entity.brain.lock() = brain::make_brain(&entity);
        entity
    }

    /// Vanilla `Camel.isDashing`.
    #[must_use]
    pub fn is_dashing(&self) -> bool {
        *self.entity_data.lock().dash.get()
    }

    /// Vanilla `Camel.setDashing`.
    pub fn set_dashing(&self, is_dashing: bool) {
        self.entity_data.lock().dash.set(is_dashing);
    }

    /// Vanilla `Camel.getJumpCooldown`: remaining dash cooldown ticks.
    #[must_use]
    pub fn dash_cooldown(&self) -> i32 {
        self.state.lock().dash_cooldown
    }

    /// Sets the dash cooldown.
    pub fn set_dash_cooldown(&self, dash_cooldown: i32) {
        self.state.lock().dash_cooldown = dash_cooldown;
    }

    /// Vanilla `Camel.onSyncedDataUpdated(DASH)`: the dash starts the full
    /// [`DASH_COOLDOWN_TICKS`] cooldown unless one is already running.
    pub fn start_dash_cooldown(&self) {
        let mut state = self.state.lock();
        if state.dash_cooldown == 0 {
            state.dash_cooldown = DASH_COOLDOWN_TICKS;
        }
    }

    /// Vanilla `Camel.isCamelSitting` (`LAST_POSE_CHANGE_TICK < 0`).
    #[must_use]
    pub fn is_camel_sitting(&self) -> bool {
        *self.entity_data.lock().last_pose_change_tick.get() < 0
    }

    /// Vanilla `Camel.isCamelVisuallySitting`.
    #[must_use]
    pub fn is_camel_visually_sitting(&self) -> bool {
        (self.pose_time() < 0) != self.is_camel_sitting()
    }

    /// Vanilla `Camel.isInPoseTransition`.
    #[must_use]
    pub fn is_in_pose_transition(&self) -> bool {
        self.pose_time()
            < if self.is_camel_sitting() {
                SITDOWN_DURATION_TICKS
            } else {
                STANDUP_DURATION_TICKS
            }
    }

    /// Vanilla `Camel.isVisuallySittingDown`.
    #[must_use]
    pub fn is_visually_sitting_down(&self) -> bool {
        self.is_camel_sitting() && (0..SITDOWN_DURATION_TICKS).contains(&self.pose_time())
    }

    /// Vanilla `Camel.getPoseTime`: ticks since the last pose change.
    #[must_use]
    pub fn pose_time(&self) -> i64 {
        let Some(world) = self.level() else {
            return DEFAULT_LAST_POSE_CHANGE_TICK;
        };

        world.game_time() - self.entity_data.lock().last_pose_change_tick.get().abs()
    }

    /// Vanilla `Camel.resetLastPoseChangeTick`.
    pub fn reset_last_pose_change_tick(&self, synced_pose_tick_time: i64) {
        self.entity_data
            .lock()
            .last_pose_change_tick
            .set(synced_pose_tick_time);
    }

    /// Vanilla `Camel.resetLastPoseChangeTickToFullStand`.
    pub fn reset_last_pose_change_tick_to_full_stand(&self) {
        let Some(world) = self.level() else {
            return;
        };

        self.reset_last_pose_change_tick(0.max(world.game_time() - STANDUP_DURATION_TICKS - 1));
    }

    /// Vanilla `Camel.sitDown`: play the sit sound, switch to the sitting pose and
    /// record the pose change tick as negative.
    pub fn sit_down(&self) {
        if self.is_camel_sitting() {
            return;
        }

        self.make_sound(Some(&sound_events::ENTITY_CAMEL_SIT));
        self.set_pose(EntityPose::Sitting);
        self.game_event(&steel_registry::vanilla_game_events::ENTITY_ACTION);
        if let Some(world) = self.level() {
            self.reset_last_pose_change_tick(-world.game_time());
        }
    }

    /// Vanilla `Camel.standUp`.
    pub fn stand_up(&self) {
        if !self.is_camel_sitting() {
            return;
        }

        self.make_sound(Some(&sound_events::ENTITY_CAMEL_STAND));
        self.set_pose(EntityPose::Standing);
        self.game_event(&steel_registry::vanilla_game_events::ENTITY_ACTION);
        if let Some(world) = self.level() {
            self.reset_last_pose_change_tick(world.game_time());
        }
    }

    /// Vanilla `Camel.standUpInstantly`: standing pose with no transition.
    pub fn stand_up_instantly(&self) {
        self.set_pose(EntityPose::Standing);
        self.game_event(&steel_registry::vanilla_game_events::ENTITY_ACTION);
        self.reset_last_pose_change_tick_to_full_stand();
    }

    /// Vanilla `Camel.canCamelChangePose`.
    ///
    /// `// GAP:` `wouldNotSuffocateAtTargetPose` needs the other pose's eye-box
    /// block-shape query, which Steel does not expose yet, so the change is
    /// always allowed.
    #[must_use]
    pub fn can_camel_change_pose(&self) -> bool {
        true
    }

    /// Vanilla `Camel.refuseToMove`: sitting or in the middle of a pose change.
    #[must_use]
    pub fn refuse_to_move(&self) -> bool {
        self.is_camel_sitting() || self.is_in_pose_transition()
    }

    /// Vanilla `Camel.isTamed`: camels are always tamed.
    #[must_use]
    pub const fn is_tamed(&self) -> bool {
        true
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

    /// Returns whether the stack matches vanilla `ItemTags.CAMEL_FOOD`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::CAMEL_FOOD)
    }

    /// Vanilla `Camel.openCustomInventoryScreen`.
    ///
    /// `// GAP:` the horse inventory menu belongs to the Menus slice.
    fn open_custom_inventory_screen(&self, _player: &Player) {
        log::debug!(
            "camel inventory screen for entity {} is not implemented yet",
            self.id()
        );
    }

    /// Vanilla `Camel.getBodyAnchorAnimationYOffset`: the rider seat height during
    /// and after a sit/stand transition. `partial_ticks` is always `0` here.
    fn body_anchor_animation_y_offset(
        &self,
        is_front: bool,
        partial_ticks: f32,
        dimensions: EntityDimensions,
        scale: f32,
    ) -> f64 {
        let age_sit_y_offset = if AgeableMob::is_baby(self) {
            0.093_75
        } else {
            0.375
        };
        let mut base_sit_offset = f64::from(dimensions.height) - age_sit_y_offset;
        let sitting_height_difference = scale * SITTING_HEIGHT_DIFFERENCE;
        let vertical_drop = sitting_height_difference - scale * 0.2;
        let bottom_point = sitting_height_difference - vertical_drop;
        let is_in_transition = self.is_in_pose_transition();
        let is_sitting = self.is_camel_sitting();

        if is_in_transition {
            let animation_duration = if is_sitting {
                SITDOWN_DURATION_TICKS as f32
            } else {
                STANDUP_DURATION_TICKS as f32
            };
            let (half_point, flex_point_offset) = if is_sitting {
                // Vanilla: `halfPoint = 28; flexPointOffset = isFront ? 0.5F : 0.1F`.
                (28.0, if is_front { 0.5 } else { 0.1 })
            } else if is_front {
                (24.0, 0.6)
            } else {
                (32.0, 0.35)
            };

            let pose_time =
                (self.pose_time() as f32 + partial_ticks).clamp(0.0, animation_duration);
            let is_first_part = pose_time < half_point;
            let part = if is_first_part {
                pose_time / half_point
            } else {
                (pose_time - half_point) / (animation_duration - half_point)
            };
            let flex_point = sitting_height_difference - flex_point_offset * vertical_drop;
            let animated = if is_sitting {
                lerp(
                    part,
                    if is_first_part {
                        sitting_height_difference
                    } else {
                        flex_point
                    },
                    if is_first_part {
                        flex_point
                    } else {
                        bottom_point
                    },
                )
            } else {
                lerp(
                    part,
                    if is_first_part {
                        bottom_point - sitting_height_difference
                    } else {
                        bottom_point - flex_point
                    },
                    if is_first_part {
                        bottom_point - flex_point
                    } else {
                        0.0
                    },
                )
            };
            base_sit_offset += f64::from(animated);
        }

        if is_sitting && !is_in_transition {
            base_sit_offset += f64::from(bottom_point);
        }

        base_sit_offset
    }

    /// Vanilla `Camel.getAgeScale` used as the passenger-offset `scale` argument.
    fn passenger_scale(&self) -> f32 {
        self.get_age_scale() * LivingEntity::get_scale(self)
    }

    /// Vanilla `Camel.tick`'s non-brain half: dash state, cooldown, head clamp and
    /// the "sitting in water stands up" rule.
    fn camel_tick(&self) {
        if self.is_dashing()
            && self.dash_cooldown() < DASH_MINIMUM_DURATION_TICKS
            && (self.on_ground() || self.is_in_water() || self.is_passenger())
        {
            self.set_dashing(false);
        }

        if self.dash_cooldown() > 0 {
            let cooldown = {
                let mut state = self.state.lock();
                state.dash_cooldown -= 1;
                state.dash_cooldown
            };
            if cooldown == 0 {
                self.play_sound(&sound_events::ENTITY_CAMEL_DASH_READY, 1.0, 1.0);
            }
        }

        if self.refuse_to_move() {
            self.clamp_head_rotation_to_body();
        }

        if self.is_camel_sitting() && self.is_in_water() {
            self.stand_up_instantly();
        }
    }

    /// Vanilla `Mob.clampHeadRotationToBody`, applied while a camel refuses to move.
    fn clamp_head_rotation_to_body(&self) {
        let y_body_rot = self.y_body_rot();
        let delta = degrees_difference(y_body_rot, self.y_head_rot());
        self.set_y_head_rot(y_body_rot + delta.clamp(-MAX_HEAD_Y_ROT, MAX_HEAD_Y_ROT));
    }

    /// Vanilla `Camel.mobInteract`: saddle/ride, food, or the baby's golden dandelion.
    fn camel_mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if player.is_secondary_use_active() && !AgeableMob::is_baby(self) {
            self.open_custom_inventory_screen(player);
            return InteractionResult::Success;
        }

        let equippable_result =
            LivingEntity::interact_living_entity_with_equippable(self, player, hand);
        if equippable_result.consumes_action() {
            return equippable_result;
        }

        if Self::is_food(&item_stack) {
            return self.fed_food(player, hand);
        }

        if self.passengers().len() < 2 && !AgeableMob::is_baby(self) {
            self.do_player_ride(player);
            return InteractionResult::Consume;
        }

        if AgeableMob::is_baby(self)
            && player.is_holding(&mut |item_stack| item_stack.is(&vanilla_items::GOLDEN_DANDELION))
        {
            return AgeableMob::mob_interact_ageable(self, player, hand);
        }

        InteractionResult::Fail
    }

    /// Vanilla `AbstractHorse.doPlayerRide`, minus the (absent) eating/standing state.
    fn do_player_ride(&self, player: &Player) {
        let (yaw, pitch) = self.rotation();
        player.set_rotation((yaw, pitch));

        if let Some(world) = self.level()
            && let Some(vehicle) = world.get_entity_by_id(self.id())
        {
            player.start_riding(&vehicle);
        }
    }

    /// Vanilla `Camel.handleEating` (+ `AbstractHorse.fedFood`'s item consumption):
    /// heal 2, breed an adult in love, grow a baby, and play the eating sound.
    fn fed_food(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if !Self::is_food(&item_stack) {
            return InteractionResult::Fail;
        }

        let could_heal = self.get_health() < self.get_max_health();
        if could_heal {
            self.heal(2.0);
        }

        let could_set_in_love =
            self.is_tamed() && self.get_age() == 0 && Animal::can_fall_in_love(self);
        if could_set_in_love {
            Animal::set_in_love(self, Some(player));
        }

        let could_age_up = self.can_age_up();
        if could_age_up {
            // VANILLA CLIENT-LOCAL: `ParticleTypes.HAPPY_VILLAGER` around the baby.
            self.age_up(10, false);
        }

        if !could_heal && !could_set_in_love && !could_age_up {
            return InteractionResult::Fail;
        }

        {
            let mut inventory = player.inventory.lock();
            inventory
                .get_item_in_hand_mut(hand)
                .consume(1, player.has_infinite_materials());
        }

        self.make_sound(Some(&sound_events::ENTITY_CAMEL_EAT));
        self.game_event(&steel_registry::vanilla_game_events::EAT);
        InteractionResult::SuccessServer
    }
}

impl Entity for CamelEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        // Vanilla `Camel.tick` runs after `super.tick()`.
        self.camel_tick();
    }

    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if pose == EntityPose::Sitting {
            let dimensions = if AgeableMob::is_baby(self) {
                CAMEL_BABY_SITTING_DIMENSIONS
            } else {
                CAMEL_ADULT_SITTING_DIMENSIONS
            };
            return dimensions.scale(scale);
        }

        if AgeableMob::is_baby(self) {
            CAMEL_BABY_STANDING_DIMENSIONS.scale(scale)
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
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;

        if block_state
            .get_block()
            .has_tag(&BlockTag::CAMEL_SAND_STEP_SOUND_BLOCKS)
        {
            self.play_sound(&sound_events::ENTITY_CAMEL_STEP_SAND, 1.0, 1.0);
        } else {
            self.play_sound(&sound_events::ENTITY_CAMEL_STEP, 1.0, 1.0);
        }
    }

    fn can_add_passenger(&self, _passenger: &dyn Entity) -> bool {
        // Vanilla `Camel.canAddPassenger` (the interaction path caps riders at two).
        self.passengers().len() <= 2
    }

    fn passenger_attachment_point(&self, passenger: &dyn Entity) -> DVec3 {
        // Vanilla `Camel.getPassengerAttachmentPoint`.
        let dimensions = self.base.dimensions();
        let scale = self.passenger_scale();
        let index = self.passenger_index(passenger).unwrap_or_default();
        let driver = index == 0;
        let mut offset = 0.5_f32;
        let height = if self.is_removed() {
            0.01
        } else {
            self.body_anchor_animation_y_offset(driver, 0.0, dimensions, scale)
        };

        if self.passengers().len() > 1 {
            if !driver {
                offset = -0.7;
            }

            if passenger.as_ageable_mob().is_some() {
                offset += 0.2;
            }
        }

        let (sin, cos) = (-self.rotation().0.to_radians()).sin_cos();
        let z = f64::from(offset * scale);
        DVec3::new(z * f64::from(sin), height, z * f64::from(cos))
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        {
            let entity_data = self.entity_data.lock();
            save_abstract_horse(&self.parts, &entity_data.abstract_horse, nbt);
        }
        nbt.insert(
            "LastPoseTick",
            *self.entity_data.lock().last_pose_change_tick.get(),
        );
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        load_abstract_horse(
            &self.parts,
            &mut self.entity_data.lock().abstract_horse,
            &nbt,
        );

        let pose_tick = nbt
            .long("LastPoseTick")
            .unwrap_or(DEFAULT_LAST_POSE_CHANGE_TICK);
        if pose_tick < 0 {
            self.set_pose(EntityPose::Sitting);
        }

        self.reset_last_pose_change_tick(pose_tick);
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        // Vanilla `AbstractHorse.getControllingPassenger` with `Camel.isTamed`: any
        // player passenger of a saddled camel controls it.
        if self.is_saddled()
            && let Some(passenger) = self.first_passenger()
            && passenger.as_player().is_some()
        {
            return Some(passenger);
        }

        self.controlling_passenger_mob()
    }
}

impl LivingEntity for CamelEntity {
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
        SOUND_VOLUME
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CAMEL_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CAMEL_DEATH)
    }

    fn get_age_scale(&self) -> f32 {
        if AgeableMob::is_baby(self) {
            BABY_SCALE
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

    fn travel(&self, entity: &SharedEntity, input: DVec3) -> Option<MoveResult> {
        // Vanilla `Camel.travel`: a camel that refuses to move keeps only vertical
        // movement and no input.
        if self.refuse_to_move() && self.on_ground() {
            let movement = self.velocity();
            self.set_velocity(DVec3::new(0.0, movement.y, 0.0));
            return self.default_travel(entity, DVec3::new(0.0, input.y, 0.0));
        }

        self.default_travel(entity, input)
    }

    fn tick_ridden(&self, controller: &Player, ridden_input: DVec3) {
        let (yaw, pitch) = controller.rotation();
        self.set_rotation((yaw, pitch * 0.5));
        self.base.set_old_yaw_to_current();
        let yaw = self.rotation().0;
        self.set_y_body_rot(yaw);
        self.set_y_head_rot(yaw);

        // Vanilla `Camel.tickRidden`: pressing forward while sitting stands up.
        if ridden_input.z > 0.0 && self.is_camel_sitting() && !self.is_in_pose_transition() {
            self.stand_up();
        }
    }

    fn ridden_input(&self, controller: &Player, self_input: DVec3) -> DVec3 {
        if self.refuse_to_move() {
            return DVec3::ZERO;
        }

        // Vanilla `AbstractHorse.getRiddenInput`.
        let input = LivingEntity::travel_input(controller);
        let sideways = f64::from(input.sideways()) * 0.5;
        let mut forward = f64::from(input.forward());
        if forward <= 0.0 {
            forward *= 0.25;
        }

        let _ = self_input;
        DVec3::new(sideways, 0.0, forward)
    }

    fn ridden_speed(&self, controller: &Player) -> f32 {
        // Vanilla `Camel.getRiddenSpeed`.
        let movement_speed = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::MOVEMENT_SPEED) as f32;
        if controller.is_sprinting() && self.dash_cooldown() == 0 {
            movement_speed + RUNNING_SPEED_BONUS
        } else {
            movement_speed
        }
    }

    fn equip_sound(&self, slot: EquipmentSlot, _stack: &ItemStack) -> Option<SoundEventRef> {
        // Vanilla `Camel.getEquipSound`: the saddle uses `CAMEL_SADDLE`.
        (slot == EquipmentSlot::Saddle).then_some(&sound_events::ENTITY_CAMEL_SADDLE)
    }

    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        slot != EquipmentSlot::Saddle || (Entity::is_alive(self) && !AgeableMob::is_baby(self))
    }

    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        slot == EquipmentSlot::Saddle || Mob::can_pick_up_loot(self)
    }

    fn actually_hurt(&self, world: &World, source: &DamageSource, amount: f32) {
        // Vanilla `Camel.actuallyHurt`: the camel stands up before taking damage.
        self.stand_up_instantly();

        // `LivingEntity::actually_hurt` default body, inlined: a UFCS call here
        // would resolve back to this override and recurse.
        if self.is_invulnerable_to(world, source) {
            return;
        }

        let damage = self.get_damage_after_armor_absorb(source, amount);
        let damage = self.get_damage_after_magic_absorb(source, damage);
        let original_damage = damage;
        let damage = (damage - self.get_absorption_amount()).max(0.0);
        self.set_absorption_amount(self.get_absorption_amount() - (original_damage - damage));

        let absorbed_damage = original_damage - damage;
        if (0.0..f32::MAX).contains(&absorbed_damage)
            && let Some(damage_causer) = source.causing_entity()
            && let Some(player) = damage_causer.as_player()
        {
            player.award_custom_stat_with_count(
                &vanilla_custom_stats::DAMAGE_DEALT_ABSORBED,
                (absorbed_damage * 10.0).round() as i32,
            );
        }

        if damage != 0.0 {
            self.set_health(self.get_health() - damage);
            self.set_absorption_amount(self.get_absorption_amount() - damage);
            self.game_event(&vanilla_game_events::ENTITY_DAMAGE);
        }
    }
}

impl AgeableMob for CamelEntity {
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

impl Animal for CamelEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        CamelEntity::is_food(item_stack)
    }

    fn play_eating_sound(&self) {
        self.make_sound(Some(&sound_events::ENTITY_CAMEL_EAT));
    }
}

impl Mob for CamelEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `Camel.customServerAiStep`: tick `CamelAi`, which ends with
    /// `CamelAi.updateActivity`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
    }

    fn tick_goal_selectors(&self, _entity: &SharedEntity) {
        // Vanilla `Camel.registerGoals` is empty: the brain owns all camel AI.
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn custom_server_ai_step(&self) {
        Animal::custom_server_ai_step_animal(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CAMEL_AMBIENT)
    }

    fn ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn max_head_y_rot(&self) -> f32 {
        MAX_HEAD_Y_ROT
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `Camel.finalizeSpawn`: `CamelAi.initMemories` is empty, then the
        // pose clock is reset to a completed stand.
        self.reset_last_pose_change_tick_to_full_stand();
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.camel_mob_interact(player, hand)
    }

    fn mob_can_be_leashed(&self) -> bool {
        // Vanilla `Animal.canBeLeashed`.
        !AgeableMob::is_baby(self)
    }
}

impl PathfinderMob for CamelEntity {}

/// Vanilla `Mth.degreesDifference`.
fn degrees_difference(a: f32, b: f32) -> f32 {
    let difference = (b - a) % 360.0;
    if difference >= 180.0 {
        difference - 360.0
    } else if difference < -180.0 {
        difference + 360.0
    } else {
        difference
    }
}

/// Vanilla `Mth.lerp`.
fn lerp(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}
