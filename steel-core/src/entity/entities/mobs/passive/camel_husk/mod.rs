//! Vanilla `CamelHusk` (`net.minecraft.world.entity.animal.camel.CamelHusk`).
//!
//! Vanilla `CamelHusk` extends `Camel` and overrides only a handful of things:
//! far-away despawn, mob control/leashing, the interaction's persistence flag,
//! its food tag, twelve sounds, the breeding gates and
//! `AbstractHorse.chargeSpeedModifier`. It declares no synced data of its own.
//!
//! Steel has no inheritance, so this type composes the shared camel body
//! ([`CamelEntity`]) exactly like [`crate::entity::entities::mobs::zombie::husk`]
//! composes the zombie body, and re-implements the overridden sounds/paths on
//! top of camel's public API (`is_camel_sitting`, `dash_cooldown`,
//! `refuse_to_move`, `reset_last_pose_change_tick`, …).
//!
//! The generated `CamelHuskEntityData` merely wraps `CamelEntityData`, so rather
//! than keeping a second copy of the camel's synced state this entity delegates
//! [`Entity::synced_data`] to the inner camel (the same layout husk-to-zombie).
//!
//! ## Deviations
//!
//! - GAP(camel-husk): `camel/brain.rs` resolves its body with
//!   `downcast_ref::<CamelEntity>()` (lines 247, 294, 339 and 353), which cannot
//!   match a composed `CamelHuskEntity`, so `RandomSitting` (sit/stand), the
//!   `CamelPanic` stand-up and `Camel.refuseToMove` stay inert here — needs the
//!   camel brain packages to resolve camel state through a capability trait (or a
//!   `pub(crate)` camel-state accessor) shared by `CamelEntity` and
//!   `CamelHuskEntity` instead of a concrete downcast.
//! - `Camel.getDashingSound` is only read by `handleStartJump` (the rider jump),
//!   which camel/mod.rs documents as having no Steel packet hook, so
//!   `ENTITY_CAMEL_HUSK_DASH` has no call site (the dash-ready sound of
//!   `Camel.tick` is implemented).
//! - GAP(camel-husk): `AgeableMob.canBeABaby` has no Steel hook. Steel encodes it
//!   the way `Parrot` does: `is_baby()` is always false and `set_baby()` is a
//!   no-op; `set_age` additionally clamps at 0 so the shared camel body cannot
//!   take its baby dimensions for a capability vanilla never allows.
//! - Vanilla gates `AgeableMob.addAdditionalSaveData`/`readAdditionalSaveData` on
//!   `canBeABaby()`, so a camel husk's `Age`/`ForcedAge`/`AgeLocked` never reach
//!   NBT. Persistence is delegated to the camel body, which always writes them;
//!   the values stay `0`/`0`/`false` because the husk never ages.

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
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_game_events, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::passive::camel::CamelEntity;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `Camel.tick`: the dash flag is cleared below this cooldown value.
const DASH_MINIMUM_DURATION_TICKS: i32 = 50;
/// Vanilla `Camel` feeding: `heal(2.0F)` in `Camel.handleEating`.
const FEED_HEAL_AMOUNT: f32 = 2.0;
/// Vanilla `Camel.handleEating` age-up when fed.
const FEED_AGE_UP_SECONDS: i32 = 10;
/// Vanilla `CamelHusk.playStepSound` volume.
const STEP_SOUND_VOLUME: f32 = 0.4;
/// Vanilla `CamelHusk.playStepSound` pitch.
const STEP_SOUND_PITCH: f32 = 1.0;
/// Vanilla `Camel.handleEating` eating-sound pitch spread.
const EATING_SOUND_PITCH_SPREAD: f32 = 0.2;
/// Vanilla `CamelHusk.chargeSpeedModifier`.
const CHARGE_SPEED_MODIFIER: f32 = 4.0;

/// Vanilla `CamelHusk`.
///
/// Steel has no inheritance, so this owns the shared `Camel` body
/// ([`CamelEntity`]) and adds the camel-husk-only behavior.
#[entity_behavior(class = "CamelHusk")]
pub struct CamelHuskEntity {
    camel: CamelEntity,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CamelHuskEntity`.
unsafe impl DowncastType for CamelHuskEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/camel_husk");
}

impl CamelHuskEntity {
    /// Creates a new camel husk at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        // `CamelEntity` builds its own synced data, brain and `AbstractHorse`
        // parts from the husk's entity type, so attributes and dimensions come
        // from `CAMEL_HUSK` in the registry.
        Self {
            camel: CamelEntity::new(entity_type, id, position, world),
        }
    }

    /// Reconstructs a camel husk from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            camel: CamelEntity::from_saved(entity_type, load),
        }
    }

    /// The shared `Camel` body carrying the camel husk's state.
    #[must_use]
    pub const fn camel(&self) -> &CamelEntity {
        &self.camel
    }

    /// Returns whether the stack matches vanilla `ItemTags.CAMEL_HUSK_FOOD`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::CAMEL_HUSK_FOOD)
    }

    /// Vanilla `AbstractHorse.isMobControlled`, overridden by `CamelHusk`: a mob
    /// passenger (not a player) is steering.
    #[must_use]
    pub fn is_mob_controlled(&self) -> bool {
        self.first_passenger()
            .is_some_and(|passenger| passenger.is_mob())
    }

    /// Vanilla `Camel.sitDown` with the camel-husk sit sound
    /// (`CamelHusk.getSitDownSound`).
    pub fn sit_down(&self) {
        if self.camel.is_camel_sitting() {
            return;
        }

        self.make_sound(Some(&sound_events::ENTITY_CAMEL_HUSK_SIT));
        self.set_pose(EntityPose::Sitting);
        self.game_event(&vanilla_game_events::ENTITY_ACTION);
        if let Some(world) = self.level() {
            self.camel.reset_last_pose_change_tick(-world.game_time());
        }
    }

    /// Vanilla `Camel.standUp` with the camel-husk stand sound
    /// (`CamelHusk.getStandUpSound`).
    pub fn stand_up(&self) {
        if !self.camel.is_camel_sitting() {
            return;
        }

        self.make_sound(Some(&sound_events::ENTITY_CAMEL_HUSK_STAND));
        self.set_pose(EntityPose::Standing);
        self.game_event(&vanilla_game_events::ENTITY_ACTION);
        if let Some(world) = self.level() {
            self.camel.reset_last_pose_change_tick(world.game_time());
        }
    }

    /// Vanilla `Camel.tick`'s non-brain half: dash state, cooldown (with the
    /// camel-husk dash-ready sound), head clamp and the "sitting in water stands
    /// up" rule.
    fn camel_husk_tick(&self) {
        if self.camel.is_dashing()
            && self.camel.dash_cooldown() < DASH_MINIMUM_DURATION_TICKS
            && (self.on_ground() || self.is_in_water() || self.is_passenger())
        {
            self.camel.set_dashing(false);
        }

        if self.camel.dash_cooldown() > 0 {
            let cooldown = self.camel.dash_cooldown() - 1;
            self.camel.set_dash_cooldown(cooldown);
            if cooldown == 0 {
                self.play_sound(&sound_events::ENTITY_CAMEL_HUSK_DASH_READY, 1.0, 1.0);
            }
        }

        if self.camel.refuse_to_move() {
            self.clamp_head_rotation_to_body();
        }

        if self.camel.is_camel_sitting() && self.is_in_water() {
            self.camel.stand_up_instantly();
        }
    }

    /// Vanilla `Mob.clampHeadRotationToBody`, applied while a camel refuses to move.
    fn clamp_head_rotation_to_body(&self) {
        let y_body_rot = self.y_body_rot();
        let max_head_y_rot = self.max_head_y_rot();
        let delta = degrees_difference(y_body_rot, self.y_head_rot());
        self.set_y_head_rot(y_body_rot + delta.clamp(-max_head_y_rot, max_head_y_rot));
    }

    /// Vanilla `Camel.mobInteract`: saddle/ride, food, inventory or the baby's
    /// golden dandelion, with the camel husk's own food tag.
    fn camel_husk_mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if player.is_secondary_use_active() && !AgeableMob::is_baby(self) {
            // Vanilla `Camel.openCustomInventoryScreen` opens the mount menu from
            // `AbstractHorse.inventory`, which only the composed camel body owns;
            // the camel's inherited interaction branch is exactly this path.
            return Mob::mob_interact(&self.camel, player, hand);
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

    /// Vanilla `AbstractHorse.doPlayerRide` (inherited unchanged by `Camel`).
    fn do_player_ride(&self, player: &Player) {
        let (yaw, pitch) = self.rotation();
        player.set_rotation((yaw, pitch));

        if let Some(world) = self.level()
            && let Some(vehicle) = world.get_entity_by_id(self.id())
        {
            player.start_riding(&vehicle);
        }
    }

    /// Vanilla `Camel.handleEating` (+ `AbstractHorse.fedFood`'s item
    /// consumption): heal 2, breed an adult in love, grow a baby, and play the
    /// camel-husk eating sound.
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
            self.heal(FEED_HEAL_AMOUNT);
        }

        let could_set_in_love =
            self.camel.is_tamed() && self.get_age() == 0 && Animal::can_fall_in_love(self);
        if could_set_in_love {
            Animal::set_in_love(self, Some(player));
        }

        let could_age_up = self.can_age_up();
        if could_age_up {
            // VANILLA CLIENT-LOCAL: `ParticleTypes.HAPPY_VILLAGER` around the baby.
            self.age_up(FEED_AGE_UP_SECONDS, false);
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

        // Vanilla `Camel.handleEating` plays `getEatingSound()` at
        // `1.0F + (random.nextFloat() - random.nextFloat()) * 0.2F`.
        if !self.is_silent() {
            let pitch =
                1.0 + (rand::random::<f32>() - rand::random::<f32>()) * EATING_SOUND_PITCH_SPREAD;
            self.play_sound(&sound_events::ENTITY_CAMEL_HUSK_EAT, 1.0, pitch);
        }
        self.game_event(&vanilla_game_events::EAT);
        InteractionResult::SuccessServer
    }
}

impl Entity for CamelHuskEntity {
    fn base(&self) -> &EntityBase {
        self.camel.base()
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.camel.entity_type()
    }

    fn interact(
        &self,
        player: &Player,
        hand: InteractionHand,
        location: DVec3,
    ) -> InteractionResult {
        // Vanilla `CamelHusk.interact`: `setPersistenceRequired()` and then the
        // inherited `Mob.interact`.
        self.set_persistence_required();
        Mob::interact_mob(self, player, hand, location)
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        // Vanilla `Camel.tick` runs after `super.tick()`.
        self.camel_husk_tick();
    }

    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        self.camel.dimensions_for_pose(pose)
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        // Vanilla `CamelHusk` declares no synced data; `CamelHuskEntityData` wraps
        // `CamelEntityData`, which the inner camel already owns.
        self.camel.synced_data()
    }

    fn update_data_before_sync(&self) {
        self.camel.update_data_before_sync();
    }

    fn max_up_step(&self) -> f32 {
        self.camel.max_up_step()
    }

    fn sound_source(&self) -> SoundSource {
        self.camel.sound_source()
    }

    fn can_add_passenger(&self, passenger: &dyn Entity) -> bool {
        self.camel.can_add_passenger(passenger)
    }

    fn passenger_attachment_point(&self, passenger: &dyn Entity) -> DVec3 {
        self.camel.passenger_attachment_point(passenger)
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        self.camel.controlling_passenger()
    }

    fn play_step_sound(&self, _pos: BlockPos, block_state: BlockStateId) {
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;

        // Vanilla `CamelHusk.playStepSound`: sand-family blocks use the sand step.
        let sound = if block_state
            .get_block()
            .has_tag(&BlockTag::CAMEL_SAND_STEP_SOUND_BLOCKS)
        {
            &sound_events::ENTITY_CAMEL_HUSK_STEP_SAND
        } else {
            &sound_events::ENTITY_CAMEL_HUSK_STEP
        };
        self.play_sound(sound, STEP_SOUND_VOLUME, STEP_SOUND_PITCH);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.camel.save_additional(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.camel.load_additional(nbt);
    }
}

impl LivingEntity for CamelHuskEntity {
    fn living_base(&self) -> &LivingEntityBase {
        self.camel.living_base()
    }

    fn get_health(&self) -> f32 {
        self.camel.get_health()
    }

    fn set_health(&self, health: f32) {
        self.camel.set_health(health);
    }

    fn sound_volume(&self) -> f32 {
        self.camel.sound_volume()
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CAMEL_HUSK_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CAMEL_HUSK_DEATH)
    }

    fn get_age_scale(&self) -> f32 {
        self.camel.get_age_scale()
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
        LivingEntity::travel(&self.camel, entity, input)
    }

    fn tick_ridden(&self, controller: &Player, ridden_input: DVec3) {
        let (yaw, pitch) = controller.rotation();
        self.set_rotation((yaw, pitch * 0.5));
        self.base().set_old_yaw_to_current();
        let yaw = self.rotation().0;
        self.set_y_body_rot(yaw);
        self.set_y_head_rot(yaw);

        // Vanilla `Camel.tickRidden`: pressing forward while sitting stands up.
        if ridden_input.z > 0.0
            && self.camel.is_camel_sitting()
            && !self.camel.is_in_pose_transition()
        {
            self.stand_up();
        }
    }

    fn ridden_input(&self, controller: &Player, self_input: DVec3) -> DVec3 {
        LivingEntity::ridden_input(&self.camel, controller, self_input)
    }

    fn ridden_speed(&self, controller: &Player) -> f32 {
        LivingEntity::ridden_speed(&self.camel, controller)
    }

    fn equip_sound(&self, slot: EquipmentSlot, _stack: &ItemStack) -> Option<SoundEventRef> {
        // Vanilla `Camel.getEquipSound` -> `CamelHusk.getSaddleSound`.
        (slot == EquipmentSlot::Saddle).then_some(&sound_events::ENTITY_CAMEL_HUSK_SADDLE)
    }

    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        self.camel.can_use_slot(slot)
    }

    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        self.camel.can_dispenser_equip_into_slot(slot)
    }

    fn actually_hurt(&self, world: &World, source: &DamageSource, amount: f32) {
        // Vanilla `Camel.actuallyHurt` stands the camel up (silently) first.
        LivingEntity::actually_hurt(&self.camel, world, source, amount);
    }
}

impl AgeableMob for CamelHuskEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        self.camel.ageable_base()
    }

    fn is_age_locked(&self) -> bool {
        self.camel.is_age_locked()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.camel.set_age_locked(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.camel.set_synced_baby(baby);
    }

    fn age_boundary_changed(&self, baby: bool) {
        self.camel.age_boundary_changed(baby);
    }

    /// Vanilla `AgeableMob.isBaby` is `canBeABaby() && getAge() < 0`, and
    /// `CamelHusk.canBeABaby` is always false.
    fn is_baby(&self) -> bool {
        false
    }

    /// Vanilla `AgeableMob.setBaby` is gated on `canBeABaby()`.
    fn set_baby(&self, _baby: bool) {}

    /// Vanilla `AgeableMob.setAge` records the age, but a camel husk can never
    /// satisfy `canBeABaby()`; clamping at 0 keeps the composed camel body from
    /// selecting baby dimensions and the 0.6 age scale.
    fn set_age(&self, age: i32) {
        self.camel.set_age(age.max(0));
    }

    /// Vanilla `CamelHusk.getBreedOffspring` returns `null`.
    fn get_breed_offspring(
        &self,
        _world: &Arc<World>,
        _partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        None
    }
}

impl Animal for CamelHuskEntity {
    fn animal_base(&self) -> &AnimalBase {
        self.camel.animal_base()
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        Self::is_food(item_stack)
    }

    fn play_eating_sound(&self) {
        self.make_sound(Some(&sound_events::ENTITY_CAMEL_HUSK_EAT));
    }

    /// Vanilla `CamelHusk.canMate` is always false.
    fn can_mate(&self, _partner: &dyn Animal) -> bool {
        false
    }

    /// Vanilla `CamelHusk.canFallInLove` is always false.
    fn can_fall_in_love(&self) -> bool {
        false
    }
}

impl Mob for CamelHuskEntity {
    fn mob_base(&self) -> &MobBase {
        self.camel.mob_base()
    }

    /// Vanilla `AbstractHorse.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        self.camel.max_spawn_cluster_size()
    }

    fn mob_flags(&self) -> i8 {
        self.camel.mob_flags()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.camel.set_mob_flags(flags);
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        self.camel.brain()
    }

    /// Vanilla `Camel.customServerAiStep`: tick `CamelAi`.
    fn brain_tick(&self, entity: &SharedEntity) {
        Mob::brain_tick(&self.camel, entity);
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
        Some(&sound_events::ENTITY_CAMEL_HUSK_AMBIENT)
    }

    fn ambient_sound_interval(&self) -> i32 {
        self.camel.ambient_sound_interval()
    }

    fn max_head_y_rot(&self) -> f32 {
        self.camel.max_head_y_rot()
    }

    /// Vanilla `Camel.finalizeSpawn` (inherited): the pose clock restarts as a
    /// completed stand, then `AgeableMob.finalizeSpawn` runs.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.camel.reset_last_pose_change_tick_to_full_stand();
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.camel_husk_mob_interact(player, hand)
    }

    /// Vanilla `CamelHusk.canBeLeashed`: `!isMobControlled()`.
    fn mob_can_be_leashed(&self) -> bool {
        !self.is_mob_controlled()
    }

    /// Vanilla `CamelHusk.removeWhenFarAway` is always true.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        true
    }

    /// Vanilla `CamelHusk.chargeSpeedModifier`.
    fn charge_speed_modifier(&self) -> f32 {
        CHARGE_SPEED_MODIFIER
    }
}

impl PathfinderMob for CamelHuskEntity {}

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
