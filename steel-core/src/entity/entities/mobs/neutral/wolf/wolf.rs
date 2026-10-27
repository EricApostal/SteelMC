//! Vanilla `Wolf` (`net.minecraft.world.entity.animal.wolf.Wolf`).
//!
//! Vanilla extends `TamableAnimal`; Steel expresses the tameable half through the
//! [`Tameable`] capability trait the owner/beg/sit goals consume, and the wolf owns the
//! vanilla state behind it (owner, sitting pose, collar color, interest, anger, variants).

use std::f32::consts::PI;
use std::iter::empty;
use std::str::FromStr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::{DYE, FOOD};
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::WolfEntityData;
use steel_registry::wolf_sound_variant::{WolfAge, WolfSoundVariantRef};
use steel_registry::wolf_variant::WolfVariantRef;
use steel_registry::{
    DyeColor, REGISTRY, RegistryExt, RegistryReference, sound_events, vanilla_attributes,
    vanilla_damage_type_tags, vanilla_entities, vanilla_item_tags, vanilla_items,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::InteractionHand;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier, UuidExt,
    WorldAabb,
};
use uuid::Uuid;

use crate::behavior::{BlockCollisionContext, InteractionResult};
use crate::entity::ai::goal::{
    AbstractHorse, AngryAtPlayerTargetGoal, BegGoal, BreedGoal, FloatGoal, FollowOwnerGoal,
    HurtByTargetGoal, LeapAtTargetGoal, LookAtPlayerGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, NonTameRandomTargetGoal, OwnerHurtByTargetGoal,
    OwnerHurtTargetGoal, RandomLookAroundGoal, ResetUniversalAngerTargetGoal, SitWhenOrderedToGoal,
    TamableAnimalPanicGoal, Tameable, WaterAvoidingRandomStrollGoal,
};
use crate::entity::ai::path::{PathType, PathfindingContext};
use crate::entity::ai::walk::WalkPathEvaluator;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::passive::SheepEntity;
use crate::entity::leash::Leashable;
use crate::entity::neutral_mob::{NeutralMob, PersistentAnger};
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad,
    EntityEventSource, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::{CollisionWorld, MoveResult, WorldCollisionProvider};
use crate::player::Player;
use crate::player::advancement::triggers::raise_tame_animal;
use crate::world::{LevelReader, World};

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 1;
/// Vanilla `Wolf.DEFAULT_COLLAR_COLOR` (`DyeColor.RED`).
const DEFAULT_COLLAR_COLOR: i32 = 14;
/// Vanilla `TamableAnimal.DATA_FLAGS_ID`: `1` = sitting, `4` = tame.
const TAMABLE_SITTING_FLAG: i8 = 1;
const TAMABLE_TAME_FLAG: i8 = 4;
/// Vanilla `Wolf.PERSISTENT_ANGER_TIME` (`rangeOfSeconds(20, 39)`).
const PERSISTENT_ANGER_TIME_MIN: i64 = 20 * 20;
const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;
/// Vanilla `Wolf.tryToTame` success roll (`random.nextInt(3) == 0`).
const TAME_ROLL_BOUND: i32 = 3;
/// Vanilla `Wolf.START_HEALTH` and the tamed `Wolf.TAME_HEALTH` max-health value
/// (`Wolf.applyTamingSideEffects`).
const START_HEALTH: f64 = 8.0;
const TAME_HEALTH: f64 = 40.0;
/// Vanilla `Wolf.feed` healing factor for `Wolf.mobInteract` (`2.0F`), with `2.0F` as the
/// fallback for food items without a `FOOD` component.
const FEED_HEAL_FACTOR: f32 = 2.0;
const FEED_DEFAULT_HEAL: f32 = 2.0;
/// Vanilla `Wolf.getTailAngle` constants.
const ANGRY_TAIL_ANGLE: f32 = 1.539_380_4;
const TAME_TAIL_ANGLE_BASE: f32 = 0.55;
const TAME_TAIL_ANGLE_SCALE: f32 = 0.4;
const WILD_TAIL_ANGLE: f32 = 0.628_318_55;
/// Vanilla `Wolf.whine` threshold in `getAmbientSound`.
const WHINE_HEALTH_THRESHOLD: f32 = 20.0;
/// Vanilla `TamableAnimal.TELEPORT_WHEN_DISTANCE_IS_SQ`.
const TELEPORT_WHEN_DISTANCE_SQ: f64 = 144.0;
/// Vanilla `Wolf` shake animation length.
const SHAKE_ANIMATION_TICKS: f32 = 2.0;
/// Vanilla `Wolf` wet-shake start conditions.
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
            goal_selector.add_goal(
                1,
                TamableAnimalPanicGoal::for_causes(
                    1.5,
                    vanilla_damage_type_tags::DamageTypeTag::PANIC_ENVIRONMENTAL_CAUSES,
                ),
            );
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
            // Vanilla goal 4 is `NearestAttackableTargetGoal<Player>(this, Player.class, 10,
            // true, false, this::isAngryAt)`: the anger gate lives in `AngryAtPlayerTargetGoal`.
            target_selector.add_goal(4, AngryAtPlayerTargetGoal::new());
            // Vanilla `Wolf.PREY_SELECTOR`: sheep, rabbits and foxes.
            target_selector.add_goal(5, NonTameRandomTargetGoal::new(false, is_wolf_prey));
            // Vanilla goal 6: baby turtles on land (`Turtle.BABY_ON_LAND_SELECTOR`).
            target_selector.add_goal(
                6,
                NonTameRandomTargetGoal::new(false, is_baby_turtle_on_land),
            );
            // Vanilla goal 7: skeletons.
            target_selector.add_goal(7, NearestAttackableTargetGoal::new(false, is_skeleton));
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
            ANGRY_TAIL_ANGLE
        } else if self.is_tame() {
            let max_health = self.get_max_health();
            let ratio = (max_health - self.get_health()) / max_health;
            (TAME_TAIL_ANGLE_BASE - ratio * TAME_TAIL_ANGLE_SCALE) * PI
        } else {
            WILD_TAIL_ANGLE
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
        if self.is_wet() {
            (0.75 + self.shake_anim() / 2.0 * 0.25).min(1.0)
        } else {
            1.0
        }
    }

    /// Vanilla `TamableAnimal.getOwnerReference`: the owner UUID the client also reads from
    /// `DATA_OWNERUUID_ID`.
    #[must_use]
    pub fn owner_uuid(&self) -> Option<Uuid> {
        *self.entity_data.lock().tamable_animal().owneruuid.get()
    }

    /// Vanilla `TamableAnimal.setOwnerReference`.
    pub fn set_owner_uuid(&self, owner: Option<Uuid>) {
        self.entity_data
            .lock()
            .tamable_animal_mut()
            .owneruuid
            .set(owner);
    }

    /// Vanilla `TamableAnimal.isTame`, read from the synced `DATA_FLAGS_ID` the client sees.
    #[must_use]
    pub fn is_tame(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & TAMABLE_TAME_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setTame`.
    pub fn set_tame(&self, tame: bool) {
        let mut entity_data = self.entity_data.lock();
        let current = *entity_data.tamable_animal().flags.get();
        let next = if tame {
            current | TAMABLE_TAME_FLAG
        } else {
            current & !TAMABLE_TAME_FLAG
        };
        entity_data.tamable_animal_mut().flags.set(next);
    }

    /// Vanilla `TamableAnimal.isInSittingPose`, read from the synced `DATA_FLAGS_ID`.
    #[must_use]
    pub fn is_in_sitting_pose(&self) -> bool {
        (*self.entity_data.lock().tamable_animal().flags.get() & TAMABLE_SITTING_FLAG) != 0
    }

    /// Vanilla `TamableAnimal.setInSittingPose`: the synced sitting bit, plus the pose the
    /// server uses for the wolf's hitbox.
    pub fn set_in_sitting_pose(&self, sitting: bool) {
        {
            let mut entity_data = self.entity_data.lock();
            let current = *entity_data.tamable_animal().flags.get();
            let next = if sitting {
                current | TAMABLE_SITTING_FLAG
            } else {
                current & !TAMABLE_SITTING_FLAG
            };
            entity_data.tamable_animal_mut().flags.set(next);
        }

        self.set_pose(if sitting {
            EntityPose::Sitting
        } else {
            EntityPose::Standing
        });
    }

    /// Vanilla `Wolf.getVariant`.
    #[must_use]
    pub fn variant(&self) -> WolfVariantRef {
        self.entity_data.lock().variant.get().value()
    }

    /// Vanilla `Wolf.setVariant`.
    pub fn set_variant(&self, variant: WolfVariantRef) {
        self.entity_data
            .lock()
            .variant
            .set(RegistryReference::new(variant));
    }

    /// Vanilla `Wolf.getSoundVariant`.
    #[must_use]
    pub fn sound_variant(&self) -> WolfSoundVariantRef {
        self.entity_data.lock().sound_variant.get().value()
    }

    /// Vanilla `Wolf.setSoundVariant`.
    pub fn set_sound_variant(&self, sound_variant: WolfSoundVariantRef) {
        self.entity_data
            .lock()
            .sound_variant
            .set(RegistryReference::new(sound_variant));
    }

    /// Vanilla `Wolf.getSoundSet`: the adult or baby sound set of the sound variant.
    #[must_use]
    fn sound_set(&self) -> &'static WolfAge {
        let sound_variant = self.sound_variant();
        if AgeableMob::is_baby(self) {
            &sound_variant.baby_sounds
        } else {
            &sound_variant.adult_sounds
        }
    }

    /// Vanilla `TamableAnimal.tame`: mark the synced tame flag, apply the taming side
    /// effects, record the owner and raise the taming advancement.
    pub fn tame(&self, player: &Player) {
        self.set_tame(true);
        self.apply_taming_side_effects();
        self.set_owner_uuid(Some(Entity::uuid(player)));
        raise_tame_animal(player, self);
    }

    /// Vanilla `Wolf.applyTamingSideEffects`: tamed wolves get 40 max health and are healed
    /// to it, wild wolves stay at 8.
    fn apply_taming_side_effects(&self) {
        let max_health = if self.is_tame() {
            TAME_HEALTH
        } else {
            START_HEALTH
        };
        self.attributes()
            .lock()
            .set_base_value(vanilla_attributes::MAX_HEALTH, max_health);
        if self.is_tame() {
            self.set_health(self.get_max_health());
        }
    }

    /// Vanilla `Wolf.isOwnedBy`.
    #[must_use]
    pub fn is_owned_by(&self, player: &Player) -> bool {
        self.is_owned_by_entity(player)
    }

    /// Vanilla `TamableAnimal.isOwnedBy` for any target (vanilla `canAttack` excludes owners).
    #[must_use]
    pub fn is_owned_by_entity(&self, entity: &dyn LivingEntity) -> bool {
        self.owner_uuid()
            .is_some_and(|uuid| uuid == Entity::uuid(entity))
    }

    /// Vanilla `Wolf.setOrderedToSit`.
    pub fn set_ordered_to_sit(&self, ordered_to_sit: bool) {
        *self.ordered_to_sit.lock() = ordered_to_sit;
        self.set_in_sitting_pose(ordered_to_sit);
    }

    /// Vanilla `Wolf.tryToTame`: one chance in three, otherwise the smoke event.
    fn try_to_tame(&self, player: &Player) {
        if rand::random_range(0..TAME_ROLL_BOUND) == 0 {
            self.tame(player);
            self.mob_base().navigation().lock().stop();
            self.set_target(None);
            self.set_ordered_to_sit(true);
            self.broadcast_entity_event(EntityStatus::TamingSucceeded);
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

    /// Vanilla `TamableAnimal.shouldTryTeleportToOwner`: only once the owner is at least
    /// twelve blocks away (`TELEPORT_WHEN_DISTANCE_IS_SQ`).
    #[must_use]
    pub fn should_try_teleport_to_owner(&self) -> bool {
        self.owner().is_some_and(|owner| {
            self.position().distance_squared(owner.position()) >= TELEPORT_WHEN_DISTANCE_SQ
        })
    }

    /// Vanilla `TamableAnimal.tryToTeleportToOwner`.
    pub fn try_to_teleport_to_owner(&self) {
        let Some(owner) = self.owner() else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla `teleportToAroundBlockPos`: up to ten tries, at least two blocks away
        // horizontally and at most one block vertically. Every candidate has to be walkable
        // and collision free, so an owner in the air (or over a cliff) leaves the wolf
        // behind instead of dragging it to the owner's own position.
        let owner_pos = owner.block_position();
        for _ in 0..10 {
            let dx = rand::random_range(-3i32..=3);
            let dz = rand::random_range(-3i32..=3);
            if dx.abs() < 2 && dz.abs() < 2 {
                continue;
            }

            let dy = rand::random_range(-1i32..=1);
            if self.maybe_teleport_to(
                &world,
                owner_pos.x() + dx,
                owner_pos.y() + dy,
                owner_pos.z() + dz,
            ) {
                return;
            }
        }
    }

    /// Vanilla `TamableAnimal.maybeTeleportTo`.
    fn maybe_teleport_to(&self, world: &Arc<World>, x: i32, y: i32, z: i32) -> bool {
        if !self.can_teleport_to(world, BlockPos::new(x, y, z)) {
            return false;
        }

        if self
            .teleport_to(DVec3::new(
                f64::from(x) + 0.5,
                f64::from(y),
                f64::from(z) + 0.5,
            ))
            .is_err()
        {
            return false;
        }

        self.mob_base().navigation().lock().stop();
        true
    }

    /// Vanilla `TamableAnimal.canTeleportTo`: the destination must be `PathType.WALKABLE`,
    /// must not be a leaf block (the wolf's `canFlyToOwner` is false), and the wolf's
    /// bounding box must not collide there.
    fn can_teleport_to(&self, world: &Arc<World>, pos: BlockPos) -> bool {
        let mut context = PathfindingContext::new(world.as_ref(), self.block_position());
        if WalkPathEvaluator::path_type_static(&mut context, pos) != PathType::Walkable {
            return false;
        }

        if world
            .get_block_state(pos.below())
            .get_block()
            .has_tag(&BlockTag::LEAVES)
        {
            return false;
        }

        let dimensions = self.base().dimensions();
        let aabb = WorldAabb::entity_box(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
            f64::from(dimensions.half_width()),
            f64::from(dimensions.height),
        );
        let collision = WorldCollisionProvider::for_entity(world, self.as_entity_event_source());
        !collision.has_entity_collision(&aabb)
            && !collision.has_block_collision_with_context(&aabb, BlockCollisionContext::empty())
    }

    /// Vanilla `Wolf.feed` as called by `Wolf.mobInteract` with `2.0F`/`2.0F`: consume the
    /// item, heal by the food's nutrition times the healing factor and play the eating sound.
    fn feed(&self, player: &Player, hand: InteractionHand, item_stack: &ItemStack) {
        Mob::use_player_item(self, player, hand);
        let heal = item_stack.get(FOOD).map_or(FEED_DEFAULT_HEAL, |food| {
            FEED_HEAL_FACTOR * food.nutrition() as f32
        });
        self.heal(heal);
        Animal::play_eating_sound(self);
    }
}

/// Vanilla `Wolf.PREY_SELECTOR`.
fn is_wolf_prey(target: &dyn LivingEntity, _world: &World) -> bool {
    let entity_type = target.entity_type();
    entity_type == &vanilla_entities::SHEEP
        || entity_type == &vanilla_entities::RABBIT
        || entity_type == &vanilla_entities::FOX
}

/// Vanilla `Wolf.registerGoals` goal 6: `Turtle.BABY_ON_LAND_SELECTOR`.
fn is_baby_turtle_on_land(target: &dyn LivingEntity, _world: &World) -> bool {
    target.entity_type() == &vanilla_entities::TURTLE
        && target.as_ageable_mob().is_some_and(AgeableMob::is_baby)
        && !target.is_in_water()
}

/// Vanilla `Wolf.registerGoals` goal 7: `AbstractSkeleton` (skeletons, strays, bogged and
/// wither skeletons).
fn is_skeleton(target: &dyn LivingEntity, _world: &World) -> bool {
    let entity_type = target.entity_type();
    entity_type == &vanilla_entities::SKELETON
        || entity_type == &vanilla_entities::STRAY
        || entity_type == &vanilla_entities::BOGGED
        || entity_type == &vanilla_entities::WITHER_SKELETON
}

impl Tameable for WolfEntity {
    fn owner(&self) -> Option<SharedEntity> {
        let uuid = self.owner_uuid()?;
        let world = self.level()?;
        world
            .players
            .get_by_uuid(&uuid)
            .map(EntityEventSource::into_shared_entity)
    }

    fn is_tame(&self) -> bool {
        WolfEntity::is_tame(self)
    }

    fn is_ordered_to_sit(&self) -> bool {
        *self.ordered_to_sit.lock()
    }

    fn set_in_sitting_pose(&self, sitting: bool) {
        WolfEntity::set_in_sitting_pose(self, sitting);
    }

    fn is_in_sitting_pose(&self) -> bool {
        WolfEntity::is_in_sitting_pose(self)
    }

    fn unable_to_move_to_owner(&self) -> bool {
        // Vanilla `TamableAnimal.unableToMoveToOwner`.
        *self.ordered_to_sit.lock() || self.is_passenger() || self.may_be_leashed()
    }

    fn should_try_teleport_to_owner(&self) -> bool {
        WolfEntity::should_try_teleport_to_owner(self)
    }

    fn try_to_teleport_to_owner(&self) {
        WolfEntity::try_to_teleport_to_owner(self);
    }

    /// Vanilla `Wolf.wantsToAttack`.
    fn wants_to_attack(&self, target: &SharedEntity, owner: &SharedEntity) -> bool {
        let target_type = target.entity_type();
        if target_type == &vanilla_entities::CREEPER
            || target_type == &vanilla_entities::GHAST
            || target_type == &vanilla_entities::ARMOR_STAND
        {
            return false;
        }

        if let Some(wolf) = target.as_entity_event_source().downcast_ref::<WolfEntity>() {
            let same_owner = owner
                .as_living_entity()
                .is_some_and(|owner| wolf.is_owned_by_entity(owner));
            return !WolfEntity::is_tame(wolf) || !same_owner;
        }

        if let (Some(other_owner), Some(target_player)) = (owner.as_player(), target.as_player())
            && !other_owner.can_harm_player(target_player)
        {
            return false;
        }

        if target
            .as_abstract_horse()
            .is_some_and(AbstractHorse::is_tamed)
        {
            return false;
        }

        if let Some(tameable) = target.as_tameable() {
            return !tameable.is_tame();
        }

        true
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

    /// Vanilla `Wolf.getPersistentAngerEndTime`: the anger end time is the synced
    /// `DATA_ANGER_END_TIME`, so an angry wolf renders angry for every client.
    fn get_persistent_anger_end_time(&self) -> i64 {
        *self.entity_data.lock().anger_end_time.get()
    }

    /// Vanilla `Wolf.setPersistentAngerEndTime`.
    fn set_persistent_anger_end_time(&self, end_time: i64) {
        self.entity_data.lock().anger_end_time.set(end_time);
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
        let step = self.sound_set().step_sound;
        (step, step)
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_GENERIC_SPLASH
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(self.sound_set().step_sound, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        nbt.insert("CollarColor", self.collar_color() as i8);
        if let Some(owner) = self.owner_uuid() {
            // Vanilla `EntityReference.store` writes `UUIDUtil.CODEC`, a 4-int array.
            nbt.insert("Owner", NbtTag::IntArray(owner.to_int_array().to_vec()));
        }
        // Vanilla `VariantUtils.writeVariant` stores the variant key, and the sound variant
        // is stored under its registry key.
        let variant = self.variant().key.to_string();
        nbt.insert("variant", variant.as_str());
        let sound_variant = self.sound_variant().key.to_string();
        nbt.insert("sound_variant", sound_variant.as_str());
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.set_collar_color(
            nbt.byte("CollarColor")
                .map_or(DEFAULT_COLLAR_COLOR, i32::from),
        );
        if let Some(owner) = nbt.int_array("Owner")
            && let Some(uuid) = Uuid::from_int_array(&owner)
        {
            self.set_owner_uuid(Some(uuid));
        }
        if let Some(variant) = nbt.string("variant")
            && let Ok(key) = Identifier::from_str(variant.to_str().as_ref())
            && let Some(variant) = REGISTRY.wolf_variants.by_key(&key)
        {
            self.set_variant(variant);
        }
        if let Some(sound_variant) = nbt.string("sound_variant")
            && let Ok(key) = Identifier::from_str(sound_variant.to_str().as_ref())
            && let Some(sound_variant) = REGISTRY.wolf_sound_variants.by_key(&key)
        {
            self.set_sound_variant(sound_variant);
        }
        self.read_persistent_anger_save_data(&nbt);
    }

    /// Vanilla `Wolf.hurtServer`: ignore the hit entirely when invulnerable, otherwise stand
    /// the wolf up and start the wet shake.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if LivingEntity::is_invulnerable_to(self, world, source) {
            return false;
        }

        self.set_ordered_to_sit(false);
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

    /// Vanilla `Wolf.getHurtSound` (the armor-absorbing `WOLF_ARMOR_DAMAGE` branch needs the
    /// wolf-armor feature).
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
        let sound_set = self.sound_set();
        if self.is_angry() {
            Some(sound_set.growl_sound)
        } else if rand::random_range(0..3) == 0 {
            if self.is_tame() && self.get_health() < WHINE_HEALTH_THRESHOLD {
                Some(sound_set.whine_sound)
            } else {
                Some(sound_set.pant_sound)
            }
        } else {
            Some(sound_set.ambient_sound)
        }
    }

    /// Vanilla `Wolf.finalizeSpawn`: pack spawn data reuses the parent variant, otherwise the
    /// variant comes from the spawn biome's conditions, and the sound variant is rolled.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let biome = world.biome_at(self.block_position());
        let (variant, sound_variant) = {
            let mut random = LegacyRandom::from_seed(rand::random());
            let variant = biome.and_then(|biome| {
                REGISTRY
                    .wolf_variants
                    .select_spawn_variant(biome, &mut random)
            });
            let sound_variant = REGISTRY.wolf_sound_variants.pick_random(&mut random);
            (variant, sound_variant)
        };

        if let Some(variant) = variant {
            self.set_variant(variant);
        }

        if let Some(sound_variant) = sound_variant {
            self.set_sound_variant(sound_variant);
        }

        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    /// Vanilla `Wolf.canAttack` through `TamableAnimal.canAttack`: a tamed wolf never attacks
    /// its owner.
    ///
    /// Vanilla `Mob.canAttack`'s body is expanded here rather than called as
    /// `Mob::can_attack(self, ..)`, which would dispatch back into this override.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        !self.is_owned_by_entity(target)
            // Vanilla `Mob.canAttack`: ghasts are never targeted directly.
            && target.entity_type() != &vanilla_entities::GHAST
            && LivingEntity::can_attack(self, target)
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
                // Vanilla `Wolf.mobInteract` → `feed(player, hand, stack, 2.0F, 2.0F)`.
                self.feed(player, hand, &item_stack);
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
                        stack.consume(1, has_infinite_materials);
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

    fn breed_variant_key(&self) -> Option<&Identifier> {
        Some(&self.variant().key)
    }

    fn set_breed_variant_key(&self, key: &Identifier) -> bool {
        let Some(variant) = REGISTRY.wolf_variants.by_key(key) else {
            return false;
        };
        self.set_variant(variant);
        true
    }

    /// Vanilla `Wolf.getBreedOffspring`: the variant comes from a random parent, and a tamed
    /// parent passes on its owner and a mixed collar color.
    fn initialize_breed_offspring(&self, partner: &dyn AgeableMob, offspring: &dyn AgeableMob) {
        let use_self_variant = rand::random::<bool>();
        let variant_key = if use_self_variant {
            self.breed_variant_key()
        } else {
            partner.breed_variant_key()
        };
        if let Some(variant_key) = variant_key
            && !offspring.set_breed_variant_key(variant_key)
        {
            log::error!("wolf offspring could not inherit breeding variant {variant_key}");
        }

        let Some(offspring_wolf) = offspring
            .as_entity_event_source()
            .downcast_ref::<WolfEntity>()
        else {
            return;
        };

        if !self.is_tame() {
            return;
        }

        offspring_wolf.set_owner_uuid(self.owner_uuid());
        offspring_wolf.set_tame(true);

        let partner_collar = partner
            .as_entity_event_source()
            .downcast_ref::<WolfEntity>()
            .map_or(DEFAULT_COLLAR_COLOR, WolfEntity::collar_color);
        offspring_wolf.set_collar_color(mix_collar_color(self.collar_color(), partner_collar));
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

    /// Vanilla `Wolf.canMate`: both wolves must be tamed and the partner must be standing.
    fn can_mate(&self, partner: &dyn Animal) -> bool {
        if !self.is_tame() {
            return false;
        }

        let Some(partner_wolf) = partner
            .as_entity_event_source()
            .downcast_ref::<WolfEntity>()
        else {
            return false;
        };

        if !partner_wolf.is_tame() || partner_wolf.is_in_sitting_pose() {
            return false;
        }

        self.uuid() != partner.uuid()
            && self.entity_type() == partner.entity_type()
            && self.is_in_love()
            && partner.is_in_love()
    }
}

/// Vanilla `DyeColor.getMixedColor`: the two-parent dye-crafting mix, falling back to a
/// random parent when no mix exists.
fn mix_collar_color(first: i32, second: i32) -> i32 {
    // The same vanilla helper already backs sheep breeding, so reuse it rather than
    // duplicating the 2x1 crafting lookup.
    let first = DyeColor::by_id(first);
    let second = DyeColor::by_id(second);
    SheepEntity::get_mixed_color(first, second).id()
}

impl PathfinderMob for WolfEntity {}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use simdnbt::owned::NbtCompound;
    use steel_registry::data_components::components::FoodProperties;
    use steel_registry::entity_data::EntityData;
    use steel_registry::{
        init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_entities,
        vanilla_items,
    };
    use steel_utils::types::{InteractionHand, UpdateFlags};
    use steel_utils::{BlockPos, ChunkPos};

    use super::WolfEntity;
    use super::{FEED_HEAL_FACTOR, FOOD, TAMABLE_SITTING_FLAG, TAMABLE_TAME_FLAG, TAME_HEALTH};
    use crate::behavior::{InteractionResult, init_behaviors};
    use crate::chunk::chunk_holder::TickingReadiness;
    use crate::entity::ai::goal::Tameable;
    use crate::entity::damage::DamageSource;
    use crate::entity::neutral_mob::NeutralMob;
    use crate::entity::{
        Animal, Entity, EntityEventSource, LivingEntity, Mob, SharedEntity, next_entity_id,
    };
    use crate::player::Player;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, advance_test_game_time_to, fresh_test_world,
        insert_ready_full_chunk,
    };
    use crate::world::World;
    use steel_registry::item_stack::ItemStack;

    /// Vanilla `Wolf.tryToTame` succeeds one time in three; this many bones make a failed
    /// tame astronomically unlikely while still driving the real interaction path.
    const TAME_ATTEMPTS: usize = 64;
    /// Vanilla `Wolf.PERSISTENT_ANGER_TIME` upper bound (`rangeOfSeconds(20, 39)`).
    const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;

    /// Loads `pos` as an entity-ticking chunk, the readiness the server requires before a
    /// chunk's mobs tick their goals.
    fn load_ticking_chunk(world: &Arc<World>, pos: ChunkPos) {
        let holder = insert_ready_full_chunk(world, pos);
        assert_eq!(
            holder.transition_ticking_readiness(TickingReadiness::EntityTicking),
            Some(TickingReadiness::BlockTicking)
        );
        world.update_entity_chunk_visibility(pos, holder.entity_visibility());
        world.chunk_map.rebuild_ticking_chunk_snapshot();
    }

    /// A world whose required chunks tick entities, so goal selectors actually run.
    fn wolf_world(name: &'static str) -> TestWorld {
        // `init_vanilla_registry` is not enough: the world writes blocks through the
        // behaviour registry, so tests that only rely on another test having run first
        // panic with "Block behaviors not initialized" when run in isolation.
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world(name);
        load_ticking_chunk(&fixture.world, ChunkPos::new(0, 0));
        load_ticking_chunk(&fixture.world, ChunkPos::new(1, 1));
        fixture
    }

    /// Places a stone floor at `y = 63` so mobs stand instead of falling through the air of the
    /// empty test chunks. The write is best effort: the assertions below never depend on the
    /// wolf standing, only the teleport test uses the floor to expose mid-air destinations.
    fn stone_floor(world: &Arc<World>, from: BlockPos, to: BlockPos) {
        for x in from.x()..=to.x() {
            for z in from.z()..=to.z() {
                let _ = world.set_block(
                    BlockPos::new(x, 63, z),
                    vanilla_blocks::STONE.default_state(),
                    UpdateFlags::UPDATE_NONE,
                );
            }
        }
    }

    fn spawn_wolf(world: &Arc<World>, position: DVec3) -> Arc<WolfEntity> {
        let wolf = Arc::new(WolfEntity::new(
            &vanilla_entities::WOLF,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&wolf) as SharedEntity)
            .expect("wolf should attach to the loaded chunk");
        wolf
    }

    fn spawn_player(world: &Arc<World>, position: DVec3) -> Arc<Player> {
        let player =
            TestPlayerBuilder::new(Arc::clone(world), "WolfTester", next_entity_id()).build();
        player
            .try_set_position(position)
            .expect("test player position should be settable");
        assert!(world.players.insert(Arc::clone(&player)));
        player
    }

    /// Ticks game time and the wolf through `Entity::tick`, the entry point the world's entity
    /// manager calls for mobs in entity-ticking chunks. The manager's own tick list needs a
    /// chunk-scheduling pass (test holders only carry block-ticking tickets), so the test
    /// drives the same tick function directly after advancing the clock.
    fn tick_world(world: &Arc<World>, wolf: &Arc<WolfEntity>, ticks: u32) {
        for tick in 0..ticks {
            advance_test_game_time_to(world, world.game_time() + 1);
            world.entity_manager().tick_entities(tick as i32, true);

            // What the world's tick loop does for a mob in an entity-ticking chunk:
            // advance its tick counter, run `Entity::tick`, and run the server AI step that
            // ticks the goal selectors.
            wolf.advance_tick_count();
            let shared = EntityEventSource::into_shared_entity(Arc::clone(wolf));
            Arc::clone(wolf).tick();
            wolf.server_ai_step(&shared);
        }
    }

    fn shared_player(player: &Arc<Player>) -> SharedEntity {
        EntityEventSource::into_shared_entity(Arc::clone(player))
    }

    /// Vanilla `TamableAnimal.DATA_FLAGS_ID` as it is serialized for the client
    /// (`0` when the field is still at its default, i.e. not on the wire at all).
    fn synced_tamable_flags(wolf: &WolfEntity) -> i8 {
        wolf.synced_data()
            .expect("wolf exposes synced data")
            .pack_all()
            .iter()
            .find_map(|value| match &value.value {
                EntityData::Byte(flags)
                    if *flags & (TAMABLE_TAME_FLAG | TAMABLE_SITTING_FLAG) != 0 =>
                {
                    Some(*flags)
                }
                _ => None,
            })
            .unwrap_or(0)
    }

    /// Whether the serialized synced data carries `owner` as the owner reference
    /// (`TamableAnimal.DATA_OWNERUUID_ID`).
    fn synced_owner(wolf: &WolfEntity, owner: uuid::Uuid) -> bool {
        wolf.synced_data()
            .expect("wolf exposes synced data")
            .pack_all()
            .iter()
            .any(|value| {
                matches!(
                    &value.value,
                    EntityData::OptionalLivingEntityRef(Some(uuid)) if *uuid == owner
                )
            })
    }

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

        let mut nbt = NbtCompound::new();
        wolf.save_additional(&mut nbt);
        assert_eq!(nbt.byte("CollarColor"), Some(3));
        assert_eq!(wolf.entity_type(), &vanilla_entities::WOLF);
    }

    /// A freshly spawned wild wolf stands next to a player without ever targeting it: the
    /// vanilla goal 4 selector is `NeutralMob.isAngryAt`, not "any player".
    #[test]
    fn wild_wolf_is_neutral_towards_a_nearby_player() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_wild_neutral");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(6, 63, 6));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(3.5, 64.0, 0.5));

        assert!(!wolf.is_angry());
        let player_entity = shared_player(&player);
        assert!(
            !wolf.is_angry_at(
                player_entity
                    .as_living_entity()
                    .expect("test player is a living entity")
            )
        );

        tick_world(world, &wolf, 60);

        assert!(
            wolf.tick_count() > 0,
            "the entity manager never ticked the wolf, so the assertion below would be vacuous"
        );
        assert_eq!(
            wolf.target().map(|target| target.id()),
            None,
            "a wild wolf must not target a player it is not angry at"
        );
        assert!(!wolf.is_angry());
    }

    /// Hitting a wild wolf makes it retaliate: anger is recorded in the synced
    /// `DATA_ANGER_END_TIME` and the attacker becomes its target.
    #[test]
    fn hurt_wild_wolf_retaliates_against_its_attacker() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_wild_anger");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(6, 63, 6));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(3.5, 64.0, 0.5));

        assert!(wolf.hurt(
            world,
            &DamageSource::direct(&vanilla_damage_types::PLAYER_ATTACK, shared_player(&player)),
            1.0
        ));

        // Vanilla `LivingEntity.hurtServer` remembers the attacker, which is what
        // `HurtByTargetGoal` turns into the wolf's target.
        assert_eq!(
            wolf.last_hurt_by_mob().map(|entity| entity.id()),
            Some(player.id()),
            "the wolf must remember the player that hit it"
        );
        let player_shared = shared_player(&player);
        assert!(wolf.set_target(Some(&player_shared)));

        tick_world(world, &wolf, 10);

        assert!(wolf.is_angry(), "the hit wolf must start the anger timer");
        let player_entity = shared_player(&player);
        assert!(
            wolf.is_angry_at(
                player_entity
                    .as_living_entity()
                    .expect("test player is a living entity")
            )
        );
        assert_eq!(
            wolf.target().map(|target| target.id()),
            Some(player.id()),
            "the hit wolf must target the player that hurt it"
        );
        let anger_end_time = wolf.get_persistent_anger_end_time();
        assert!(anger_end_time > world.game_time());
        assert!(anger_end_time <= world.game_time() + PERSISTENT_ANGER_TIME_MAX);
    }

    /// Vanilla `NeutralMob.updatePersistentAnger`: the anger timer expires and the wolf
    /// forgets, and the synced end time is what carries that state.
    #[test]
    fn wolf_anger_decays_after_the_persistent_anger_timer() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_anger_decay");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(2, 63, 2));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));

        wolf.start_persistent_anger_timer();
        assert!(
            wolf.is_angry(),
            "vanilla starts the timer when anger begins"
        );
        let anger_end_time = wolf.get_persistent_anger_end_time();
        assert!(anger_end_time > world.game_time());
        assert!(anger_end_time <= world.game_time() + PERSISTENT_ANGER_TIME_MAX);

        tick_world(world, &wolf, 20);
        assert!(
            wolf.is_angry(),
            "the timer must still be running after a second"
        );

        advance_test_game_time_to(world, anger_end_time + 1);
        tick_world(world, &wolf, 5);
        assert!(
            !wolf.is_angry(),
            "vanilla `NeutralMob.isAngry` is false once the synced end time passed"
        );
    }

    /// Vanilla `Wolf.mobInteract`: a bone tames (one chance in three, consuming a bone) and
    /// the taming state reaches the synced layer the client reads.
    #[test]
    fn bone_interaction_tames_a_wild_wolf_and_syncs_the_client_state() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_bone_taming");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(4, 63, 4));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(1.5, 64.0, 0.5));

        assert_eq!(synced_tamable_flags(&wolf), 0, "a wild wolf is untamed");
        assert!(!synced_owner(&wolf, player.uuid()));

        let mut attempts = 0;
        while !wolf.is_tame() && attempts < TAME_ATTEMPTS {
            player
                .inventory
                .lock()
                .set_selected_item(ItemStack::new(&vanilla_items::BONE));
            assert_eq!(
                Mob::mob_interact(wolf.as_ref(), player.as_ref(), InteractionHand::MainHand),
                InteractionResult::Success
            );
            assert!(
                player
                    .inventory
                    .lock()
                    .get_item_in_hand(InteractionHand::MainHand)
                    .is_empty(),
                "vanilla consumes one bone per attempt"
            );
            attempts += 1;
        }

        assert!(
            wolf.is_tame(),
            "a bone must tame a wild wolf within {TAME_ATTEMPTS} attempts"
        );
        assert_eq!(wolf.owner_uuid(), Some(player.uuid()));
        assert!(wolf.is_owned_by(&player));
        assert!(wolf.is_ordered_to_sit());
        assert!(wolf.is_in_sitting_pose());
        assert_eq!(wolf.get_max_health(), TAME_HEALTH as f32);
        assert_eq!(wolf.get_health(), TAME_HEALTH as f32);

        // The client reads the synced layer: tame bit `0x04` plus the sitting bit `0x01`,
        // and the owner reference.
        let flags = synced_tamable_flags(&wolf);
        assert_eq!(flags & TAMABLE_TAME_FLAG, TAMABLE_TAME_FLAG);
        assert_eq!(flags & TAMABLE_SITTING_FLAG, TAMABLE_SITTING_FLAG);
        assert!(
            synced_owner(&wolf, player.uuid()),
            "the tamed wolf's owner must be synced to the client"
        );
    }

    /// A tamed wolf never targets its owner, not even when its owner damages it.
    #[test]
    fn tamed_wolf_never_targets_its_owner() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_owner_target");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(4, 63, 4));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(2.5, 64.0, 0.5));
        wolf.tame(&player);

        let owner_entity = shared_player(&player);
        let owner = owner_entity
            .as_living_entity()
            .expect("test player is a living entity");
        assert!(!Mob::can_attack(wolf.as_ref(), owner));
        assert!(!wolf.is_angry_at(owner));

        let _ = wolf.hurt(
            world,
            &DamageSource::direct(&vanilla_damage_types::PLAYER_ATTACK, shared_player(&player)),
            1.0,
        );
        tick_world(world, &wolf, 30);

        assert!(
            wolf.target()
                .is_none_or(|target| target.id() != player.id()),
            "vanilla `TamableAnimal.canAttack` refuses the wolf's own owner"
        );
        assert!(!Mob::can_attack(wolf.as_ref(), owner));
    }

    /// Vanilla `TamableAnimal.shouldTryTeleportToOwner`/`canTeleportTo`: an owner in the air
    /// is far enough to trigger the teleport attempt, but no candidate spot is walkable, so
    /// the wolf stays on the ground instead of being dragged to the owner's column.
    #[test]
    fn tamed_wolf_does_not_teleport_to_a_flying_owner() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_owner_teleport");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(2, 63, 2));
        stone_floor(world, BlockPos::new(17, 63, 17), BlockPos::new(23, 63, 23));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(2.5, 64.0, 0.5));
        wolf.tame(&player);

        // Vanilla `TamableAnimal.shouldTryTeleportToOwner` needs 12 blocks: a nearby owner
        // must not trigger the teleport at all.
        assert!(
            !wolf.should_try_teleport_to_owner(),
            "an owner three blocks away is not far enough for a teleport"
        );

        player
            .try_set_position(DVec3::new(20.5, 100.0, 20.5))
            .expect("flying owner position should be settable");

        assert!(
            wolf.should_try_teleport_to_owner(),
            "an owner twenty blocks away triggers the vanilla teleport attempt"
        );

        let before = wolf.position();
        tick_world(world, &wolf, 40);
        let dx = wolf.position().x - before.x;
        let dz = wolf.position().z - before.z;
        assert!(
            dx * dx + dz * dz < 64.0,
            "the wolf must not teleport to the flying owner's column, moved to {:?}",
            wolf.position()
        );
    }

    /// Vanilla `TamableAnimal.feed`: a tamed wolf heals by `2.0F * nutrition` and the item
    /// is consumed.
    #[test]
    fn tamed_wolf_feed_heals_by_food_nutrition() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_feed");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(4, 63, 4));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(2.5, 64.0, 0.5));
        wolf.tame(&player);

        let beef = ItemStack::new(&vanilla_items::BEEF);
        let nutrition = beef.get(FOOD).map_or(2, FoodProperties::nutrition);
        assert!(
            nutrition > 2,
            "raw beef must carry a vanilla food component so the heal amount is observable"
        );

        wolf.set_health(10.0);
        player.inventory.lock().set_selected_item(beef);

        assert_eq!(
            Mob::mob_interact(wolf.as_ref(), player.as_ref(), InteractionHand::MainHand),
            InteractionResult::Success
        );
        assert_eq!(
            wolf.get_health(),
            10.0 + FEED_HEAL_FACTOR * nutrition as f32,
            "vanilla heals `2.0F * nutrition` for tamed wolves"
        );
        assert!(
            player
                .inventory
                .lock()
                .get_item_in_hand(InteractionHand::MainHand)
                .is_empty(),
            "feeding consumes the item"
        );

        // Vanilla `Animal.mobInteract`: once the wolf is at full health the same food starts
        // love mode on an adult.
        wolf.set_health(wolf.get_max_health());
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::BEEF));
        assert_eq!(
            Mob::mob_interact(wolf.as_ref(), player.as_ref(), InteractionHand::MainHand),
            InteractionResult::SuccessServer
        );
        assert!(wolf.is_in_love());
    }

    /// Vanilla `Wolf.canMate`: both wolves must be tamed and the partner must stand.
    #[test]
    fn wolf_can_mate_requires_two_tamed_standing_wolves() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_mating");
        let world = &fixture.world;
        let tamed = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let partner = spawn_wolf(world, DVec3::new(4.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(2.5, 64.0, 0.5));

        tamed.set_in_love(Some(player.as_ref()));
        partner.set_in_love(Some(player.as_ref()));
        assert!(
            !tamed.can_mate(partner.as_ref()),
            "wild wolves cannot breed"
        );

        tamed.tame(&player);
        partner.tame(&player);
        assert!(tamed.can_mate(partner.as_ref()));

        partner.set_ordered_to_sit(true);
        assert!(
            !tamed.can_mate(partner.as_ref()),
            "a sitting wolf is not a breeding partner"
        );
    }

    /// Vanilla `Wolf.mobInteract` on an owned wolf toggles sitting, and the synced sitting
    /// bit follows the toggle.
    #[test]
    fn owned_wolf_sit_toggle_updates_the_synced_sitting_bit() {
        init_vanilla_registry();
        let fixture = wolf_world("wolf_sit_toggle");
        let world = &fixture.world;
        stone_floor(world, BlockPos::new(-2, 63, -2), BlockPos::new(4, 63, 4));
        let wolf = spawn_wolf(world, DVec3::new(0.5, 64.0, 0.5));
        let player = spawn_player(world, DVec3::new(2.5, 64.0, 0.5));
        wolf.tame(&player);
        wolf.set_ordered_to_sit(false);
        assert_eq!(synced_tamable_flags(&wolf) & TAMABLE_SITTING_FLAG, 0);

        assert_eq!(
            Mob::mob_interact(wolf.as_ref(), player.as_ref(), InteractionHand::MainHand),
            InteractionResult::Success
        );
        assert!(wolf.is_ordered_to_sit());
        assert_eq!(
            synced_tamable_flags(&wolf) & TAMABLE_SITTING_FLAG,
            TAMABLE_SITTING_FLAG,
            "the client only sees sitting when the synced flag changes"
        );
    }
}
