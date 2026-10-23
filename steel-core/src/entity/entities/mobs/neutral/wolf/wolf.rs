//! Vanilla `Wolf` (`net.minecraft.world.entity.animal.wolf.Wolf`).
//!
//! Vanilla extends `TamableAnimal`; Steel expresses the tameable half through the
//! [`Tameable`] capability trait the owner/beg/sit goals consume, and the wolf owns the
//! vanilla state behind it (owner, sitting pose, collar color, interest, anger, variants).

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::DYE;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::WolfEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_entities, vanilla_item_tags, vanilla_items,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    BegGoal, BreedGoal, FloatGoal, FollowOwnerGoal, HurtByTargetGoal, LeapAtTargetGoal,
    LookAtPlayerGoal, MeleeAttackGoal, NearestAttackableTargetGoal, NonTameRandomTargetGoal,
    OwnerHurtByTargetGoal, OwnerHurtTargetGoal, PanicGoal, RandomLookAroundGoal,
    ResetUniversalAngerTargetGoal, SitWhenOrderedToGoal, Tameable, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::neutral_mob::{NeutralMob, PersistentAnger};
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntityEventSource as _, EntityPose, EntitySyncedData, LivingEntity, LivingEntityBase, Mob,
    MobBase, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::{LevelReader, World};

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 1;
/// Vanilla `Wolf.DEFAULT_COLLAR_COLOR`.
const DEFAULT_COLLAR_COLOR: i32 = 14;
/// Vanilla `Wolf.PERSISTENT_ANGER_TIME` (`rangeOfSeconds(20, 39)`).
const PERSISTENT_ANGER_TIME_MIN: i64 = 20 * 20;
const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;
/// Vanilla `Wolf.tryToTame` success roll (`random.nextInt(3) == 0`).
const TAME_ROLL_BOUND: i32 = 3;
/// Vanilla `Wolf` taming feed heal amounts.
const FEED_HEAL: f32 = 2.0;
/// Vanilla `Wolf.getTailAngle` thresholds.
const TAIL_ANGLE_MAX_HEALTH: f32 = 20.0;
const TAIL_ANGLE_MAX: f32 = 1.5;
const TAIL_ANGLE_MIN: f32 = 0.4;
/// Vanilla `Wolf` shake animation length.
const SHAKE_ANIMATION_TICKS: f32 = 2.0;
/// Vanilla `Wolf` wet-shake start conditions.
const WET_SHAKE_WATER_HEIGHT: f64 = 0.4;
const WET_SHAKE_RANDOM_BOUND: i32 = 40;

/// Vanilla `Wolf`.
#[entity_behavior(class = "Wolf")]
pub struct WolfEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<WolfEntityData>,
    persistent_anger: PersistentAnger,
    /// Vanilla `TamableAnimal.ownerUUID`.
    owner_uuid: SyncMutex<Option<Uuid>>,
    /// Vanilla `TamableAnimal.orderedToSit`.
    ordered_to_sit: SyncMutex<bool>,
    /// Vanilla `Wolf.isWet`/`isShaking`/`shakeAnim`.
    is_wet: SyncMutex<bool>,
    is_shaking: SyncMutex<bool>,
    shake_anim: SyncMutex<f32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `WolfEntity`.
unsafe impl DowncastType for WolfEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/wolf");
}

impl WolfEntity {
    /// Creates a new wolf entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved wolf entity.
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
        mob_base.set_xp_reward(XP_REWARD);
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = WolfEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.interested.set(false);
        entity_data.collar_color.set(DEFAULT_COLLAR_COLOR);
        entity_data.anger_end_time.set(-1);

        {
            // Vanilla `Wolf.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, PanicGoal::new(1.5));
            goal_selector.add_goal(2, SitWhenOrderedToGoal::new());
            goal_selector.add_goal(4, LeapAtTargetGoal::new(0.4));
            goal_selector.add_goal(5, MeleeAttackGoal::new(1.0, true));
            goal_selector.add_goal(6, FollowOwnerGoal::new(1.0, 10.0, 2.0));
            goal_selector.add_goal(7, BreedGoal::new(1.0));
            goal_selector.add_goal(8, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(
                9,
                BegGoal::new(8.0, |item_stack| {
                    !item_stack.is_empty()
                        && item_stack
                            .item()
                            .has_tag(&vanilla_item_tags::ItemTag::WOLF_FOOD)
                }),
            );
            goal_selector.add_goal(10, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(10, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, OwnerHurtByTargetGoal::new());
            target_selector.add_goal(2, OwnerHurtTargetGoal::new());
            target_selector.add_goal(3, HurtByTargetGoal::new().set_alert_others(empty()));
            target_selector.add_goal(
                4,
                NearestAttackableTargetGoal::new_for_players_with_interval(
                    10,
                    true,
                    false,
                    |_, _| true,
                ),
            );
            // Vanilla `Wolf.PREY_SELECTOR`: sheep, rabbits and foxes.
            target_selector.add_goal(5, NonTameRandomTargetGoal::new(false, is_wolf_prey));
            target_selector.add_goal(8, ResetUniversalAngerTargetGoal::new(true));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            persistent_anger: PersistentAnger::new(),
            owner_uuid: SyncMutex::new(None),
            ordered_to_sit: SyncMutex::new(false),
            is_wet: SyncMutex::new(false),
            is_shaking: SyncMutex::new(false),
            shake_anim: SyncMutex::new(0.0),
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

    /// Vanilla `Wolf.getCollarColor`.
    #[must_use]
    pub fn collar_color(&self) -> i32 {
        *self.entity_data.lock().collar_color.get()
    }

    /// Vanilla `Wolf.setCollarColor`.
    pub fn set_collar_color(&self, color: i32) {
        self.entity_data.lock().collar_color.set(color);
    }

    /// Vanilla `Wolf.isWet`.
    #[must_use]
    pub fn is_wet(&self) -> bool {
        *self.is_wet.lock()
    }

    /// Vanilla `Wolf.getTailAngle`.
    #[must_use]
    pub fn tail_angle(&self) -> f32 {
        if self.is_angry() {
            TAIL_ANGLE_MAX * (std::f32::consts::PI / 5.0).sin()
        } else {
            let health = self.get_health();
            let ratio = (TAIL_ANGLE_MAX_HEALTH - health) / TAIL_ANGLE_MAX_HEALTH;
            TAIL_ANGLE_MAX * (ratio * std::f32::consts::PI).sin()
        }
    }

    /// Vanilla `Wolf.getShakeAnim`.
    #[must_use]
    pub fn shake_anim(&self) -> f32 {
        *self.shake_anim.lock()
    }

    /// Vanilla `Wolf.getHeadRollAngle` (wet fur factor).
    #[must_use]
    pub fn head_roll_angle(&self) -> f32 {
        if !self.is_wet() {
            1.0
        } else {
            (0.75 + self.shake_anim() / 2.0 * 0.25).min(1.0)
        }
    }

    /// Vanilla `Wolf.tame`.
    pub fn tame(&self, player: &Player) {
        self.owner_uuid.lock().replace(Entity::uuid(player));
        self.set_target(None);
        self.mob_base().navigation().lock().stop();
        self.set_ordered_to_sit(true);
        self.entity_data.lock().anger_end_time.set(-1);
        self.broadcast_entity_event(EntityStatus::TamingSucceeded);
    }

    /// Vanilla `Wolf.isOwnedBy`.
    #[must_use]
    pub fn is_owned_by(&self, player: &Player) -> bool {
        self.owner_uuid
            .lock()
            .is_some_and(|uuid| uuid == Entity::uuid(player))
    }

    /// Vanilla `Wolf.setOrderedToSit`.
    pub fn set_ordered_to_sit(&self, ordered_to_sit: bool) {
        *self.ordered_to_sit.lock() = ordered_to_sit;
        self.set_in_sitting_pose(ordered_to_sit);
    }

    /// Vanilla `Wolf.tryToTame`.
    fn try_to_tame(&self, player: &Player) {
        if rand::random_range(0..TAME_ROLL_BOUND) == 0 {
            self.tame(player);
        } else {
            self.broadcast_entity_event(EntityStatus::TamingFailed);
        }
    }

    /// Vanilla `Wolf.cancelShake`.
    fn cancel_shake(&self) {
        *self.is_shaking.lock() = false;
        *self.shake_anim.lock() = 0.0;
    }

    /// Vanilla `Wolf.aiStep` wet-shake body.
    fn tick_wet_shake(&self) {
        if self.is_wet() && !*self.is_shaking.lock() && self.on_ground() {
            let is_in_water = self.is_in_water();
            if !is_in_water && rand::random_range(0..WET_SHAKE_RANDOM_BOUND) == 0 {
                *self.is_shaking.lock() = true;
                *self.shake_anim.lock() = 0.0;
            }
        }

        if *self.is_shaking.lock() {
            let mut anim = self.shake_anim.lock();
            *anim += 0.05;
            if *anim >= SHAKE_ANIMATION_TICKS {
                drop(anim);
                self.play_sound(
                    &sound_events::ENTITY_WOLF_SHAKE,
                    self.sound_volume(),
                    (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0,
                );
                *self.is_wet.lock() = false;
                self.cancel_shake();
            }
        }
    }
}

/// Vanilla `Wolf.PREY_SELECTOR`.
fn is_wolf_prey(target: &dyn LivingEntity, _world: &World) -> bool {
    let entity_type = target.entity_type();
    entity_type == &vanilla_entities::SHEEP
        || entity_type == &vanilla_entities::RABBIT
        || entity_type == &vanilla_entities::FOX
}

impl Tameable for WolfEntity {
    fn owner(&self) -> Option<SharedEntity> {
        let uuid = self.owner_uuid.lock().as_ref().copied()?;
        let world = self.level()?;
        world
            .players
            .get_by_uuid(&uuid)
            .map(|player| player.into_shared_entity())
    }

    fn is_tame(&self) -> bool {
        self.owner_uuid.lock().is_some()
    }

    fn is_ordered_to_sit(&self) -> bool {
        *self.ordered_to_sit.lock()
    }

    fn set_in_sitting_pose(&self, sitting: bool) {
        self.set_pose(if sitting {
            EntityPose::Sitting
        } else {
            EntityPose::Standing
        });
    }

    fn is_in_sitting_pose(&self) -> bool {
        self.pose() == EntityPose::Sitting
    }

    fn unable_to_move_to_owner(&self) -> bool {
        *self.ordered_to_sit.lock()
    }

    fn should_try_teleport_to_owner(&self) -> bool {
        !self.is_ordered_to_sit() && !self.is_passenger() && !self.is_in_water()
    }

    fn try_to_teleport_to_owner(&self) {
        let Some(owner) = self.owner() else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };
        // Vanilla `TamableAnimal.tryToTeleportToOwner`: land near the owner, never into water.
        let owner_position = owner.position();
        let _ = self.random_teleport(
            &world,
            owner_position.x + (rand::random::<f64>() - 0.5) * 2.0,
            owner_position.y,
            owner_position.z + (rand::random::<f64>() - 0.5) * 2.0,
            true,
        );
    }

    fn wants_to_attack(&self, target: &SharedEntity, owner: &SharedEntity) -> bool {
        !target.is_allied_to(owner.as_ref())
    }

    fn is_interested(&self) -> bool {
        *self.entity_data.lock().interested.get()
    }

    fn set_is_interested(&self, interested: bool) {
        self.entity_data.lock().interested.set(interested);
    }
}

impl NeutralMob for WolfEntity {
    fn persistent_anger(&self) -> &PersistentAnger {
        &self.persistent_anger
    }

    fn start_persistent_anger_timer(&self) {
        self.set_time_to_remain_angry(
            PERSISTENT_ANGER_TIME_MIN
                + rand::random_range(0..(PERSISTENT_ANGER_TIME_MAX - PERSISTENT_ANGER_TIME_MIN)),
        );
    }
}

impl Entity for WolfEntity {
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
        self.tick_wet_shake();
        self.tick_living_entity(&entity);
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

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        (
            &sound_events::ENTITY_WOLF_STEP,
            &sound_events::ENTITY_WOLF_STEP,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_GENERIC_SPLASH
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_WOLF_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("CollarColor", self.collar_color() as i8);
        if let Some(owner) = *self.owner_uuid.lock() {
            nbt.insert("Owner", owner.to_string());
        }
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_collar_color(
            nbt.byte("CollarColor")
                .map(i32::from)
                .unwrap_or(DEFAULT_COLLAR_COLOR),
        );
        if let Some(owner) = nbt.string("Owner")
            && let Ok(uuid) = owner.to_str().parse::<Uuid>()
        {
            self.owner_uuid.lock().replace(uuid);
        }
        self.read_persistent_anger_save_data(&nbt);
    }

    /// Vanilla `Wolf.hurtServer`: a wet wolf starts shaking when hurt.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = LivingEntity::hurt_server(self, world, source, amount);
        if was_hurt && self.is_wet() {
            *self.is_shaking.lock() = true;
            *self.shake_anim.lock() = 0.0;
        }
        was_hurt
    }
}

impl LivingEntity for WolfEntity {
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
        Some(&sound_events::ENTITY_WOLF_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WOLF_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.update_persistent_anger(true);
        self.mob_ai_step(entity)
    }
}

impl Mob for WolfEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    /// Vanilla `Wolf.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        8
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Wolf.getAmbientSound`: growl when angry, whine/pant when tame, else ambient.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        if self.is_angry() {
            Some(&sound_events::ENTITY_WOLF_GROWL)
        } else if rand::random_range(0..3) == 0 {
            if self.is_tame() && self.get_health() < TAIL_ANGLE_MAX_HEALTH {
                Some(&sound_events::ENTITY_WOLF_WHINE)
            } else {
                Some(&sound_events::ENTITY_WOLF_PANT)
            }
        } else {
            Some(&sound_events::ENTITY_WOLF_AMBIENT)
        }
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Wolf.mobInteract`: feeding, dyeing the collar, armor and taming.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };
        let has_infinite_materials = player.has_infinite_materials();

        if self.is_tame() {
            let is_food = !item_stack.is_empty()
                && item_stack
                    .item()
                    .has_tag(&vanilla_item_tags::ItemTag::WOLF_FOOD);
            if is_food && self.get_health() < self.get_max_health() {
                self.heal(FEED_HEAL);
                player
                    .inventory
                    .lock()
                    .mutate_item_in_hand(hand, |stack| stack.consume(1, has_infinite_materials));
                return InteractionResult::Success;
            }

            if item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::WOLF_COLLAR_DYES)
                && self.is_owned_by(player)
                && let Some(color) = item_stack.get(DYE)
            {
                let color_id = color.id();
                if color_id != self.collar_color() {
                    self.set_collar_color(color_id);
                    player.inventory.lock().mutate_item_in_hand(hand, |stack| {
                        stack.consume(1, has_infinite_materials)
                    });
                    return InteractionResult::Success;
                }
            }

            let interaction = Animal::mob_interact_animal(self, player, hand);
            if interaction.consumes_action() {
                return interaction;
            }

            if self.is_owned_by(player) {
                // Vanilla: an owned wolf toggles sitting.
                self.set_ordered_to_sit(!self.is_ordered_to_sit());
                self.set_target(None);
                self.mob_base().navigation().lock().stop();
                return InteractionResult::Success;
            }
            return interaction;
        }

        if item_stack.is(&vanilla_items::BONE) && !self.is_angry() {
            player
                .inventory
                .lock()
                .mutate_item_in_hand(hand, |stack| stack.consume(1, has_infinite_materials));
            self.try_to_tame(player);
            return InteractionResult::Success;
        }

        Animal::mob_interact_animal(self, player, hand)
    }
}

impl AgeableMob for WolfEntity {
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

impl Animal for WolfEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Wolf.isFood` uses the `#minecraft:wolf_food` tag.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is_empty()
            && item_stack
                .item()
                .has_tag(&vanilla_item_tags::ItemTag::WOLF_FOOD)
    }

    /// Vanilla `Wolf.checkWolfSpawnRules`.
    fn check_animal_spawn_rules(
        level: &dyn LevelReader,
        _spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        level
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&BlockTag::WOLVES_SPAWNABLE_ON)
            && Self::is_bright_enough_to_spawn(level, pos)
    }
}

impl PathfinderMob for WolfEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::WolfEntity;
    use crate::entity::Entity;
    use crate::entity::ai::goal::Tameable;

    #[test]
    fn wolf_taming_state_drives_sitting_and_nbt() {
        init_vanilla_registry();
        let wolf = WolfEntity::new(&vanilla_entities::WOLF, 1, DVec3::ZERO, Weak::new());

        assert!(!Tameable::is_tame(&wolf));
        assert!(!Tameable::is_ordered_to_sit(&wolf));
        assert_eq!(wolf.collar_color(), 14);

        wolf.set_collar_color(3);
        wolf.set_ordered_to_sit(true);
        assert!(Tameable::is_ordered_to_sit(&wolf));
        assert!(Tameable::is_in_sitting_pose(&wolf));

        let mut nbt = simdnbt::owned::NbtCompound::new();
        wolf.save_additional(&mut nbt);
        assert_eq!(nbt.byte("CollarColor"), Some(3));
        assert_eq!(wolf.entity_type(), &vanilla_entities::WOLF);
    }
}
