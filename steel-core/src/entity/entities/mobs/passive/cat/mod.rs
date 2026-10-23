//! Vanilla `Cat` entity: tameable, collared, variant-driven sounds and cat-only AI.

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::cat_sound_variant::{CatAge, CatSoundVariantRef};
use steel_registry::cat_variant::{CatVariantRef, SpawnCondition};
use steel_registry::data_components::vanilla_components::{
    CAT_COLLAR, CAT_SOUND_VARIANT, CAT_VARIANT, DYE,
};
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::shared_structs::BiomeConditionTarget;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::CatEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    DyeColor, REGISTRY, RegistryExt, RegistryReference, TaggedRegistryExt, vanilla_attributes,
    vanilla_entities,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier, UuidExt,
};
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::ai::control::MoveControlOperation;
use crate::entity::ai::goal::{
    BreedGoal, CatLieOnBlockGoal, CatSitOnBlockGoal, Feline, FloatGoal, FollowOwnerGoal,
    LeapAtTargetGoal, LookAtPlayerGoal, NonTameRandomTargetGoal, OcelotAttackGoal,
    SitWhenOrderedToGoal, Tameable, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::leash::Leashable;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;
use crate::world::spawn::difficulty::moon_brightness;

use goals::{CatAvoidEntityGoal, CatRelaxOnOwnerGoal, CatTemptGoal, TamableAnimalPanicGoal};

mod goals;

const CAT_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 0.3125, 0.0)];
const CAT_BABY_WIDTH: f32 = 0.3;
const CAT_BABY_HEIGHT: f32 = 0.35;
const CAT_BABY_EYE_HEIGHT: f32 = 0.34375;

const CAT_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    CAT_BABY_WIDTH,
    CAT_BABY_HEIGHT,
    CAT_BABY_EYE_HEIGHT,
    EntityAttachments::new(&CAT_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `Cat.TEMPT_SPEED_MOD`.
const TEMPT_SPEED_MOD: f64 = 0.6;
/// Vanilla `Cat.WALK_SPEED_MOD`.
const WALK_SPEED_MOD: f64 = 0.8;
/// The crouch pose speed vanilla checks in `Cat.customServerAiStep`.
const MOVEMENT_SPEED_MOD: f64 = 0.6;
/// Vanilla `Cat.SPRINT_SPEED_MOD`.
const SPRINT_SPEED_MOD: f64 = 1.33;
/// Vanilla `TamableAnimal.DATA_FLAGS_ID` bit 2: tamed.
const TAME_FLAG: i8 = 4;
/// Vanilla `TamableAnimal.DATA_FLAGS_ID` bit 0: in the sitting pose.
const SITTING_FLAG: i8 = 1;
/// Vanilla `Cat.DEFAULT_COLLAR_COLOR`.
const DEFAULT_COLLAR_COLOR: DyeColor = DyeColor::Red;
/// Vanilla `Cat.getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 120;
/// Vanilla `Cat.tick`: an untamed, food-tempted cat begs every 100 ticks.
const BEG_FOR_FOOD_INTERVAL: i32 = 100;
/// Vanilla `Cat.handleLieDown`: lying cats purr every 5 ticks.
const LIE_DOWN_SOUND_INTERVAL: i32 = 5;

#[entity_behavior(class = "Cat")]
/// Vanilla cat: tameable, collared, with variant-driven sounds.
pub struct CatEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<CatEntityData>,
    /// Vanilla `TamableAnimal.orderedToSit` (saved as `Sitting`, not synced).
    ordered_to_sit: AtomicBool,
    /// Vanilla `Cat.isLyingOnTopOfSleepingPlayer` (client rendering state).
    lying_on_top_of_sleeping_player: AtomicBool,
    /// Vanilla `Cat.lieDownAmount`.
    lie_down_amount: SyncMutex<f32>,
    /// Vanilla `Cat.lieDownAmountO`.
    lie_down_amount_old: SyncMutex<f32>,
    /// Vanilla `Cat.lieDownAmountTail`.
    lie_down_amount_tail: SyncMutex<f32>,
    /// Vanilla `Cat.lieDownAmountOTail`.
    lie_down_amount_tail_old: SyncMutex<f32>,
    /// Vanilla `Cat.relaxStateOneAmount`.
    relax_state_one_amount: SyncMutex<f32>,
    /// Vanilla `Cat.relaxStateOneAmountO`.
    relax_state_one_amount_old: SyncMutex<f32>,
    /// Vanilla `Cat.temptGoal.isRunning`, mirrored so `tick` can tell whether the cat begs.
    tempt_running: Arc<AtomicBool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CatEntity`.
unsafe impl DowncastType for CatEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/cat");
}

impl CatEntity {
    /// Creates a new cat at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a cat from persisted base entity state.
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
        let mut entity_data = CatEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let tempt_running = Arc::new(AtomicBool::new(false));

        {
            // Vanilla `Cat.registerGoals`, including the untamed avoid-players goal that
            // vanilla re-adds from `reassessTameGoals` (an untamed cat keeps it).
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, TamableAnimalPanicGoal::new(1.5));
            goal_selector.add_goal(2, SitWhenOrderedToGoal::new());
            goal_selector.add_goal(3, CatRelaxOnOwnerGoal::new());
            goal_selector.add_goal(
                4,
                CatTemptGoal::new(TEMPT_SPEED_MOD, Arc::clone(&tempt_running)),
            );
            goal_selector.add_goal(
                4,
                CatAvoidEntityGoal::new(16.0, WALK_SPEED_MOD, SPRINT_SPEED_MOD),
            );
            goal_selector.add_goal(5, CatLieOnBlockGoal::new(1.1, 8));
            goal_selector.add_goal(6, FollowOwnerGoal::new(1.0, 10.0, 5.0));
            goal_selector.add_goal(7, CatSitOnBlockGoal::new(0.8));
            goal_selector.add_goal(8, LeapAtTargetGoal::new(0.3));
            goal_selector.add_goal(9, OcelotAttackGoal::new());
            goal_selector.add_goal(10, BreedGoal::new(0.8));
            goal_selector.add_goal(11, WaterAvoidingRandomStrollGoal::new(0.8));
            goal_selector.add_goal(12, LookAtPlayerGoal::new(10.0));
        }

        {
            // Vanilla target goals: untamed cats hunt rabbits and baby turtles.
            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(
                1,
                NonTameRandomTargetGoal::new(false, |target, _| {
                    target.entity_type() == &vanilla_entities::RABBIT
                }),
            );
            target_selector.add_goal(
                1,
                NonTameRandomTargetGoal::new(false, |target, _| {
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
            ordered_to_sit: AtomicBool::new(false),
            lying_on_top_of_sleeping_player: AtomicBool::new(false),
            lie_down_amount: SyncMutex::new(0.0),
            lie_down_amount_old: SyncMutex::new(0.0),
            lie_down_amount_tail: SyncMutex::new(0.0),
            lie_down_amount_tail_old: SyncMutex::new(0.0),
            relax_state_one_amount: SyncMutex::new(0.0),
            relax_state_one_amount_old: SyncMutex::new(0.0),
            tempt_running,
        }
    }

    /// Vanilla `Cat.getVariant`.
    #[must_use]
    pub fn variant(&self) -> CatVariantRef {
        self.entity_data.lock().variant.get().value()
    }

    /// Vanilla `Cat.setVariant`.
    pub fn set_variant(&self, variant: CatVariantRef) {
        self.entity_data
            .lock()
            .variant
            .set(RegistryReference::new(variant));
    }

    /// Vanilla `Cat.getSoundVariant`.
    #[must_use]
    pub fn sound_variant(&self) -> CatSoundVariantRef {
        self.entity_data.lock().sound_variant.get().value()
    }

    /// Vanilla `Cat.setSoundVariant`.
    pub fn set_sound_variant(&self, sound_variant: CatSoundVariantRef) {
        self.entity_data
            .lock()
            .sound_variant
            .set(RegistryReference::new(sound_variant));
    }

    /// Vanilla `Cat.getSoundSet`: babies use the baby sound set.
    #[must_use]
    pub fn sound_set(&self) -> &'static CatAge {
        let sound_variant = self.sound_variant();
        if AgeableMob::is_baby(self) {
            &sound_variant.baby_sounds
        } else {
            &sound_variant.adult_sounds
        }
    }

    /// Vanilla `Cat.getCollarColor`.
    #[must_use]
    pub fn collar_color(&self) -> DyeColor {
        DyeColor::by_id(*self.entity_data.lock().collar_color.get())
    }

    /// Vanilla `Cat.setCollarColor`.
    pub fn set_collar_color(&self, color: DyeColor) {
        self.entity_data.lock().collar_color.set(color.id());
    }

    /// Vanilla `Cat.setLying`.
    pub fn set_lying(&self, lying: bool) {
        self.entity_data.lock().is_lying.set(lying);
    }

    /// Vanilla `Cat.isLying`.
    #[must_use]
    pub fn is_lying(&self) -> bool {
        *self.entity_data.lock().is_lying.get()
    }

    /// Vanilla `Cat.isRelaxStateOne`.
    #[must_use]
    pub fn is_relax_state_one(&self) -> bool {
        *self.entity_data.lock().relax_state_one.get()
    }

    /// Vanilla `Cat.setRelaxStateOne`.
    pub fn set_relax_state_one(&self, relax_state_one: bool) {
        self.entity_data.lock().relax_state_one.set(relax_state_one);
    }

    /// Vanilla `Cat.isLyingOnTopOfSleepingPlayer`.
    #[must_use]
    pub fn is_lying_on_top_of_sleeping_player(&self) -> bool {
        self.lying_on_top_of_sleeping_player.load(Ordering::Relaxed)
    }

    /// Vanilla `Cat.getLieDownAmount`.
    #[must_use]
    pub fn lie_down_amount(&self, partial_tick: f32) -> f32 {
        lerp(
            partial_tick,
            *self.lie_down_amount_old.lock(),
            *self.lie_down_amount.lock(),
        )
    }

    /// Vanilla `Cat.getLieDownAmountTail`.
    #[must_use]
    pub fn lie_down_amount_tail(&self, partial_tick: f32) -> f32 {
        lerp(
            partial_tick,
            *self.lie_down_amount_tail_old.lock(),
            *self.lie_down_amount_tail.lock(),
        )
    }

    /// Vanilla `Cat.getRelaxStateOneAmount`.
    #[must_use]
    pub fn relax_state_one_amount(&self, partial_tick: f32) -> f32 {
        lerp(
            partial_tick,
            *self.relax_state_one_amount_old.lock(),
            *self.relax_state_one_amount.lock(),
        )
    }

    /// Vanilla `Cat.hiss`.
    pub fn hiss(&self) {
        self.play_sound(self.sound_set().hiss_sound, 1.0, 1.0);
    }

    /// Vanilla `TamableAnimal.getOwnerReference`: the owner UUID, when one is set.
    #[must_use]
    pub fn owner_uuid(&self) -> Option<Uuid> {
        *self.entity_data.lock().tamable_animal().owneruuid.get()
    }

    /// Vanilla `TamableAnimal.setOwner`.
    pub fn set_owner_uuid(&self, owner: Option<Uuid>) {
        self.entity_data
            .lock()
            .tamable_animal_mut()
            .owneruuid
            .set(owner);
    }

    /// Vanilla `TamableAnimal.isTame`.
    #[must_use]
    pub fn is_tame(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & TAME_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setTame`.
    pub fn set_tame(&self, tame: bool) {
        let mut entity_data = self.entity_data.lock();
        let current = *entity_data.tamable_animal().flags.get();
        let next = if tame {
            current | TAME_FLAG
        } else {
            current & !TAME_FLAG
        };
        entity_data.tamable_animal_mut().flags.set(next);
    }

    /// Vanilla `TamableAnimal.isOrderedToSit`.
    #[must_use]
    pub fn is_ordered_to_sit(&self) -> bool {
        self.ordered_to_sit.load(Ordering::Relaxed)
    }

    /// Vanilla `TamableAnimal.setOrderedToSit`.
    pub fn set_ordered_to_sit(&self, ordered_to_sit: bool) {
        self.ordered_to_sit.store(ordered_to_sit, Ordering::Relaxed);
    }

    /// Vanilla `TamableAnimal.isInSittingPose`.
    #[must_use]
    pub fn is_in_sitting_pose(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & SITTING_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setInSittingPose`.
    pub fn set_in_sitting_pose(&self, sitting: bool) {
        let mut entity_data = self.entity_data.lock();
        let current = *entity_data.tamable_animal().flags.get();
        let next = if sitting {
            current | SITTING_FLAG
        } else {
            current & !SITTING_FLAG
        };
        entity_data.tamable_animal_mut().flags.set(next);
    }

    /// Vanilla `TamableAnimal.isOwnedBy`.
    #[must_use]
    pub fn is_owned_by(&self, player: &Player) -> bool {
        self.owner_uuid() == Some(player.uuid())
    }

    /// Vanilla `TamableAnimal.unableToMoveToOwner`.
    #[must_use]
    pub fn unable_to_move_to_owner(&self) -> bool {
        self.is_ordered_to_sit()
            || self.is_passenger()
            || self.may_be_leashed()
            || self
                .owner_uuid()
                .and_then(|uuid| self.level()?.players.get_by_uuid(&uuid))
                .is_some_and(|owner| owner.is_spectator())
    }

    /// Vanilla `TamableAnimal.shouldTryTeleportToOwner`.
    #[must_use]
    pub fn should_try_teleport_to_owner(&self) -> bool {
        self.owner_position().is_some_and(|owner_pos| {
            let (x, y, z) = owner_pos.get_center();
            self.position().distance_squared(DVec3::new(x, y, z)) >= 144.0
        })
    }

    /// Vanilla `TamableAnimal.tryToTeleportToOwner`.
    pub fn try_to_teleport_to_owner(&self) {
        let Some(owner_pos) = self.owner_position() else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla `teleportToAroundBlockPos`: up to 10 tries at least two blocks away.
        for _ in 0..10 {
            let dx = rand::random_range(-3i32..=3);
            let dz = rand::random_range(-3i32..=3);
            if dx.abs() < 2 && dz.abs() < 2 {
                continue;
            }

            let dy = rand::random_range(-1i32..=1);
            let target = DVec3::new(
                f64::from(owner_pos.x() + dx) + 0.5,
                f64::from(owner_pos.y() + dy),
                f64::from(owner_pos.z() + dz) + 0.5,
            );
            if self.random_teleport(&world, target.x, target.y, target.z, false) {
                return;
            }
        }
    }

    /// Vanilla `TamableAnimal.tame`.
    pub fn tame(&self, player: &Player) {
        self.set_tame(true);
        self.set_owner_uuid(Some(player.uuid()));
        // GAP: `CriteriaTriggers.TAME_ANIMAL` needs the advancement trigger layer.
    }

    /// Vanilla `Cat.tryToTame`: one chance in three, otherwise the smoke event.
    fn try_to_tame(&self, player: &Player) {
        if rand::random_range(0..3) == 0 {
            self.tame(player);
            self.set_ordered_to_sit(true);
            self.broadcast_entity_event(EntityStatus::TamingSucceeded);
        } else {
            self.broadcast_entity_event(EntityStatus::TamingFailed);
        }
    }

    /// Vanilla `TamableAnimal.getOwner`: the live player when it is present in this world.
    fn owner_position(&self) -> Option<BlockPos> {
        let uuid = self.owner_uuid()?;
        let world = self.level()?;
        let owner = world.players.get_by_uuid(&uuid)?;
        Some(owner.block_position())
    }

    /// Vanilla `Cat.feed`: consume the stack, heal and play the eating sound.
    fn feed(&self, player: &Player, hand: InteractionHand) {
        Mob::use_player_item(self, player, hand);
        // GAP: `FoodProperties.nutrition` is not modelled, so vanilla's
        // `healingFactor * nutrition` collapses to the `defaultHeal` of 1.0.
        self.heal(1.0);
        Animal::play_eating_sound(self);
    }

    /// Vanilla `Cat.tick`: begging sound while tempted, then the lie-down animation.
    fn tick_cat(&self) {
        if self.tempt_running.load(Ordering::Relaxed)
            && !self.is_tame()
            && self.tick_count().rem_euclid(BEG_FOR_FOOD_INTERVAL) == 0
        {
            self.play_sound(self.sound_set().beg_for_food_sound, 1.0, 1.0);
        }

        self.handle_lie_down();
    }

    /// Vanilla `Cat.handleLieDown`.
    fn handle_lie_down(&self) {
        let lying_or_relaxed = self.is_lying() || self.is_relax_state_one();
        if lying_or_relaxed && self.tick_count().rem_euclid(LIE_DOWN_SOUND_INTERVAL) == 0 {
            let pitch = 0.6 + 0.4 * (rand::random::<f32>() - rand::random::<f32>());
            self.play_sound(self.sound_set().purr_sound, 1.0, pitch);
        }

        self.update_lie_down_amount();
        self.update_relax_state_one_amount();

        let mut on_sleeping_player = false;
        if self.is_lying()
            && let Some(world) = self.level()
        {
            // Vanilla `new AABB(cat.blockPosition()).inflate(2.0, 2.0, 2.0)`.
            let pos = self.block_position();
            let center = DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            );
            let search_box =
                steel_utils::WorldAabb::of_size(center, 1.0, 1.0, 1.0).inflate_xyz(2.0, 2.0, 2.0);
            on_sleeping_player = !world
                .get_entities_in_aabb_matching(&search_box, |entity| {
                    entity.as_player().is_some_and(LivingEntity::is_sleeping)
                })
                .is_empty();
        }
        self.lying_on_top_of_sleeping_player
            .store(on_sleeping_player, Ordering::Relaxed);
    }

    /// Vanilla `Cat.updateLieDownAmount`.
    fn update_lie_down_amount(&self) {
        *self.lie_down_amount_old.lock() = *self.lie_down_amount.lock();
        *self.lie_down_amount_tail_old.lock() = *self.lie_down_amount_tail.lock();

        if self.is_lying() {
            let mut amount = self.lie_down_amount.lock();
            *amount = (*amount + 0.15).min(1.0);
            let mut tail = self.lie_down_amount_tail.lock();
            *tail = (*tail + 0.08).min(1.0);
        } else {
            let mut amount = self.lie_down_amount.lock();
            *amount = (*amount - 0.22).max(0.0);
            let mut tail = self.lie_down_amount_tail.lock();
            *tail = (*tail - 0.13).max(0.0);
        }
    }

    /// Vanilla `Cat.updateRelaxStateOneAmount`.
    fn update_relax_state_one_amount(&self) {
        *self.relax_state_one_amount_old.lock() = *self.relax_state_one_amount.lock();
        let mut amount = self.relax_state_one_amount.lock();
        if self.is_relax_state_one() {
            *amount = (*amount + 0.1).min(1.0);
        } else {
            *amount = (*amount - 0.13).max(0.0);
        }
    }

    fn set_variant_by_key(&self, key: &Identifier) -> bool {
        let Some(variant) = REGISTRY.cat_variants.by_key(key) else {
            return false;
        };
        self.set_variant(variant);
        true
    }

    fn set_sound_variant_by_key(&self, key: &Identifier) {
        if let Some(sound_variant) = REGISTRY.cat_sound_variants.by_key(key) {
            self.set_sound_variant(sound_variant);
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

    /// Vanilla `Cat.isFood`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::CAT_FOOD)
    }
}

/// Vanilla `Mth.lerp` for the animation amounts.
fn lerp(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

impl Tameable for CatEntity {
    fn owner(&self) -> Option<SharedEntity> {
        let uuid = self.owner_uuid()?;
        let world = self.level()?;
        world
            .players
            .get_by_uuid(&uuid)
            .map(|player| -> SharedEntity { player })
    }

    fn is_tame(&self) -> bool {
        CatEntity::is_tame(self)
    }

    fn is_ordered_to_sit(&self) -> bool {
        CatEntity::is_ordered_to_sit(self)
    }

    fn set_in_sitting_pose(&self, sitting: bool) {
        CatEntity::set_in_sitting_pose(self, sitting);
    }

    fn is_in_sitting_pose(&self) -> bool {
        CatEntity::is_in_sitting_pose(self)
    }

    fn unable_to_move_to_owner(&self) -> bool {
        CatEntity::unable_to_move_to_owner(self)
    }

    fn should_try_teleport_to_owner(&self) -> bool {
        CatEntity::should_try_teleport_to_owner(self)
    }

    fn try_to_teleport_to_owner(&self) {
        CatEntity::try_to_teleport_to_owner(self);
    }

    fn wants_to_attack(&self, _target: &SharedEntity, _owner: &SharedEntity) -> bool {
        // Vanilla `TamableAnimal.wantsToAttack` returns true unconditionally; cats add no
        // `Cat.wantsToAttack` override.
        true
    }
}

impl Feline for CatEntity {
    fn is_lying(&self) -> bool {
        CatEntity::is_lying(self)
    }

    fn set_lying(&self, lying: bool) {
        CatEntity::set_lying(self, lying);
    }
}

impl Entity for CatEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        if let Some(variant) = item_stack.get(CAT_VARIANT) {
            self.set_variant(variant.value());
        }
        if let Some(sound_variant) = item_stack.get(CAT_SOUND_VARIANT) {
            self.set_sound_variant(sound_variant.value());
        }
        if let Some(collar) = item_stack.get(CAT_COLLAR) {
            self.set_collar_color(*collar);
        }
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        self.tick_cat();
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if AgeableMob::is_baby(self) {
            CAT_BABY_DIMENSIONS.scale(scale)
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

    fn is_tame_owned_by(&self, owner: &dyn LivingEntity) -> bool {
        self.owner_uuid() == Some(owner.uuid())
    }

    fn is_stepping_carefully(&self) -> bool {
        // Vanilla `Cat.isSteppingCarefully`: crouching cats also step carefully.
        self.is_crouching() || self.is_suppressing_bounce()
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        // Vanilla `TamableAnimal.addAdditionalSaveData`: `Owner` + `Sitting`.
        if let Some(owner) = self.owner_uuid() {
            nbt.insert("Owner", NbtTag::IntArray(owner.to_int_array().to_vec()));
        }
        nbt.insert("Sitting", i8::from(self.is_ordered_to_sit()));
        // Vanilla `Cat.addAdditionalSaveData`: `variant`, `sound_variant`, `CollarColor`.
        nbt.insert("variant", self.variant().key.to_string());
        nbt.insert("sound_variant", self.sound_variant().key.to_string());
        nbt.insert(
            "CollarColor",
            i8::try_from(self.collar_color().id()).unwrap_or(14),
        );
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);

        self.set_owner_uuid(
            nbt.int_array("Owner")
                .and_then(|array| Uuid::from_int_array(&array)),
        );
        self.set_ordered_to_sit(nbt.byte("Sitting").is_some_and(|value| value != 0));
        self.set_in_sitting_pose(self.is_ordered_to_sit());

        if let Some(variant) = nbt.string("variant")
            && let Ok(key) = Identifier::from_str(variant.to_str().as_ref())
        {
            self.set_variant_by_key(&key);
        }
        if let Some(sound_variant) = nbt.string("sound_variant")
            && let Ok(key) = Identifier::from_str(sound_variant.to_str().as_ref())
        {
            self.set_sound_variant_by_key(&key);
        }

        self.set_collar_color(
            nbt.byte("CollarColor")
                .map_or(DEFAULT_COLLAR_COLOR, |id| DyeColor::by_id(i32::from(id))),
        );
    }
}

impl LivingEntity for CatEntity {
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
        Some(self.sound_set().hurt_sound)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(self.sound_set().death_sound)
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

impl AgeableMob for CatEntity {
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

    fn breed_variant_key(&self) -> Option<&Identifier> {
        Some(&self.variant().key)
    }

    fn set_breed_variant_key(&self, key: &Identifier) -> bool {
        self.set_variant_by_key(key)
    }

    fn initialize_breed_offspring(&self, partner: &dyn AgeableMob, offspring: &dyn AgeableMob) {
        // Vanilla `Cat.getBreedOffspring`: variant comes from a random parent, and a tamed
        // parent passes on its owner and a mixed collar color.
        let use_self_variant = rand::random::<bool>();
        let variant_key = if use_self_variant {
            self.breed_variant_key()
        } else {
            partner.breed_variant_key()
        };
        if let Some(variant_key) = variant_key
            && !offspring.set_breed_variant_key(variant_key)
        {
            log::error!("cat offspring could not inherit breeding variant {variant_key}");
        }

        let Some(offspring_cat) = offspring
            .as_entity_event_source()
            .downcast_ref::<CatEntity>()
        else {
            return;
        };

        if !self.is_tame() {
            return;
        }

        offspring_cat.set_owner_uuid(self.owner_uuid());
        offspring_cat.set_tame(true);

        let partner_collar = partner
            .as_entity_event_source()
            .downcast_ref::<CatEntity>()
            .map_or(DEFAULT_COLLAR_COLOR, CatEntity::collar_color);
        offspring_cat.set_collar_color(mix_collar_color(self.collar_color(), partner_collar));
    }
}

/// Vanilla `DyeColor.getMixedColor`: the two-parent dye-crafting mix, falling back to a
/// random parent when no mix exists.
fn mix_collar_color(first: DyeColor, second: DyeColor) -> DyeColor {
    // The same vanilla helper already backs sheep breeding, so reuse it rather than
    // duplicating the 2x1 crafting lookup.
    crate::entity::entities::mobs::passive::SheepEntity::get_mixed_color(first, second)
}

impl Animal for CatEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        CatEntity::is_food(item_stack)
    }

    fn can_mate(&self, partner: &dyn Animal) -> bool {
        // Vanilla `Cat.canMate`: both cats must be tamed, then the base animal rules.
        if !self.is_tame() {
            return false;
        }

        if !partner
            .as_entity_event_source()
            .downcast_ref::<CatEntity>()
            .is_some_and(CatEntity::is_tame)
        {
            return false;
        }

        self.uuid() != partner.uuid()
            && self.entity_type() == partner.entity_type()
            && self.is_in_love()
            && partner.is_in_love()
    }

    fn play_eating_sound(&self) {
        // Vanilla `Cat.playEatingSound`.
        self.play_sound(self.sound_set().eat_sound, 1.0, 1.0);
    }

    fn remove_when_far_away_animal(&self, _dist_sqr: f64) -> bool {
        // Vanilla `Cat.removeWhenFarAway`.
        !self.is_tame() && self.tick_count() > 2400
    }
}

impl Mob for CatEntity {
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
        let sounds = self.sound_set();
        if self.is_tame() {
            if self.is_in_love() {
                Some(sounds.purr_sound)
            } else if rand::random_range(0..4) == 0 {
                Some(sounds.purreow_sound)
            } else {
                Some(sounds.ambient_sound)
            }
        } else {
            Some(sounds.stray_ambient_sound)
        }
    }

    fn ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let biome = world.biome_at(self.block_position());
        let mut random = LegacyRandom::from_seed(rand::random());

        if let Some(biome) = biome
            && let Some(variant) = pick_spawn_conditioned_cat_variant(world, biome, &mut random)
        {
            self.set_variant(variant);
        }

        // Vanilla `CatSoundVariants.pickRandomSoundVariant`: a uniform registry pick.
        let sound_variant_count = REGISTRY.cat_sound_variants.len();
        if sound_variant_count > 0 {
            let index = random.next_i32_bounded(sound_variant_count as i32) as usize;
            if let Some(sound_variant) = REGISTRY.cat_sound_variants.by_id(index) {
                self.set_sound_variant(sound_variant);
            }
        }

        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if self.is_tame() {
            if self.is_owned_by(player) {
                let dye = item_stack.get(DYE).copied();
                if let Some(color) = dye
                    && REGISTRY
                        .items
                        .is_in_tag(item_stack.item(), &ItemTag::CAT_COLLAR_DYES)
                    && color != self.collar_color()
                {
                    self.set_collar_color(color);
                    Mob::use_player_item(self, player, hand);
                    self.set_persistence_required();
                    return InteractionResult::Success;
                }

                if CatEntity::is_food(&item_stack) && self.get_health() < self.get_max_health() {
                    self.feed(player, hand);
                    return InteractionResult::Success;
                }

                let parent_interaction = Animal::mob_interact_animal(self, player, hand);
                if !parent_interaction.consumes_action() {
                    self.set_ordered_to_sit(!self.is_ordered_to_sit());
                    return InteractionResult::Success;
                }

                return parent_interaction;
            }
        } else if CatEntity::is_food(&item_stack) {
            Mob::use_player_item(self, player, hand);
            self.try_to_tame(player);
            self.set_persistence_required();
            Animal::play_eating_sound(self);
            return InteractionResult::Success;
        }

        let interaction = Animal::mob_interact_animal(self, player, hand);
        if interaction.consumes_action() {
            self.set_persistence_required();
        }
        interaction
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for CatEntity {}

/// Vanilla `PriorityProvider.pick` over cat-variant spawn conditions.
///
/// Mirrors [`steel_registry::shared_structs::pick_spawn_conditioned_entry`] for the
/// richer cat-variant condition set (`CatVariant` carries its own
/// `SpawnCondition` enum).
///
/// GAP(cat-structure-conditions): `SpawnCondition::Structure` needs a
/// structure-location query that `World` does not expose, so structure-gated
/// entries never match; the unconditioned priority-0 entry every other variant
/// carries is unaffected.
fn pick_spawn_conditioned_cat_variant(
    world: &World,
    biome: steel_registry::biome::BiomeRef,
    random: &mut impl Random,
) -> Option<CatVariantRef> {
    let moon = moon_brightness(world.overworld_clock_time());
    let mut selected: Vec<CatVariantRef> = Vec::new();
    let mut highest_priority = i32::MIN;

    for (_, variant) in REGISTRY.cat_variants.iter() {
        for entry in variant.spawn_conditions {
            let matches = match &entry.condition {
                None => true,
                Some(SpawnCondition::Biome { biomes }) => biome_condition_matches(biomes, biome),
                Some(SpawnCondition::MoonBrightness { min, max }) => {
                    min.is_none_or(|min| moon >= min) && max.is_none_or(|max| moon <= max)
                }
                Some(SpawnCondition::Structure { .. }) => false,
            };
            if !matches {
                continue;
            }

            match entry.priority.cmp(&highest_priority) {
                std::cmp::Ordering::Greater => {
                    selected.clear();
                    selected.push(variant);
                    highest_priority = entry.priority;
                }
                std::cmp::Ordering::Equal => selected.push(variant),
                std::cmp::Ordering::Less => {}
            }
        }
    }

    let bound = i32::try_from(selected.len()).ok()?;
    if bound == 0 {
        return None;
    }

    let index = random.next_i32_bounded(bound) as usize;
    selected.get(index).copied()
}

/// Vanilla `SpawnCondition.Biome#test` for a `#tag`/`namespace:path` target.
fn biome_condition_matches(target: &str, biome: steel_registry::biome::BiomeRef) -> bool {
    let (is_tag, id) = match target.strip_prefix('#') {
        Some(tag) => (true, tag),
        None => (false, target),
    };
    let Ok(id) = Identifier::from_str(id) else {
        return false;
    };
    let target = if is_tag {
        BiomeConditionTarget::Tag(id)
    } else {
        BiomeConditionTarget::Direct(id)
    };
    target.matches_biome(biome)
}
