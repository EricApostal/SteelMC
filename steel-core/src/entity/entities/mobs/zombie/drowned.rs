//! Vanilla `Drowned` (`net.minecraft.world.entity.monster.zombie.Drowned`).
//!
//! Vanilla's `Drowned` extends `Zombie`, replaces `addBehaviourGoals` wholesale and
//! adds amphibious movement plus a trident ranged attack. Steel has no inheritance,
//! so this owns the shared `Zombie` body ([`ZombieEntity::new_with_variant`]) and
//! adds the drowned-only behaviour on top of it.

use std::f64::consts::FRAC_PI_2;
use std::iter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::wrap_degrees;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::blocks::shapes::is_offset_face_full;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_biome_tags::BiomeTag;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_blocks, vanilla_entities, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random;
use steel_utils::random::RandomSource;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::{Difficulty, InteractionHand};
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier,
};

use crate::entity::ai::behavior::random_pos::default_random_pos_towards;
use crate::entity::ai::control::MoveControlOperation;
use crate::entity::ai::goal::{
    Goal, GoalControls, HurtByTargetGoal, MeleeAttackGoal, MoveToBlockGoal,
    NearestAttackableTargetGoal, RandomStrollGoal, RangedAttackGoal, RangedAttackMob,
    WaterAvoidingRandomStrollGoal, ZombieAttackGoal, is_goal,
};
use crate::entity::ai::path::{Path, PathType};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::zombie::ZombieEntity;
use crate::entity::entities::mobs::zombie::zombie::ZombieVariant;
use crate::entity::registry::ENTITIES;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource as _, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::fluid::FluidStateExt as _;
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::{MoveResult, MoverType};
use crate::world::{LevelReader, World};

/// Vanilla `Drowned.NAUTILUS_SHELL_CHANCE`.
const NAUTILUS_SHELL_CHANCE: f32 = 0.03;
/// Vanilla `Drowned.ZOMBIE_NAUTILUS_JOCKEY_CHANCE`.
const ZOMBIE_NAUTILUS_JOCKEY_CHANCE: f32 = 0.5;
/// Vanilla `Drowned.DrownedGoToWaterGoal.getWaterPos`: `i < 10` attempts.
const WATER_POS_ATTEMPTS: usize = 10;
/// Vanilla `Drowned.DrownedGoToBeachGoal`: `getY() >= level.getSeaLevel() - 3`.
const BEACH_SEA_LEVEL_MARGIN: i32 = 3;
/// Vanilla `Drowned.DrownedSwimUpGoal.canUse`: `getY() < seaLevel - 2`.
const SWIM_UP_START_SEA_LEVEL_MARGIN: i32 = 2;
/// Vanilla `Drowned.DrownedSwimUpGoal.tick`: aims at `seaLevel - 1`.
const SWIM_UP_TARGET_SEA_LEVEL_MARGIN: i32 = 1;
/// Vanilla `Drowned.isDeepEnoughToSpawn`: `pos.getY() < seaLevel - 5`.
const DEEP_SPAWN_SEA_LEVEL_MARGIN: i32 = 5;
/// Vanilla `Drowned.checkDrownedSpawnRules`: `random.nextInt(15) == 0`.
const FREQUENT_DROWNED_SPAWN_ROLL: i32 = 15;
/// Vanilla `Drowned.checkDrownedSpawnRules`: `random.nextInt(40) == 0`.
const RARE_DROWNED_SPAWN_ROLL: i32 = 40;
/// Vanilla `Drowned.DrownedMoveControl.tick`: `rotlerp(..., 90.0F)` and the
/// horizontal drag/shove scales.
const MOVE_CONTROL_MAX_TURN: f32 = 90.0;
const MOVE_CONTROL_ACCELERATION_SCALE: f64 = 0.005;
const MOVE_CONTROL_VERTICAL_SCALE: f64 = 0.1;
/// Vanilla `Mth.lerp(0.125F, this.mob.getSpeed(), targetSpeed)`.
const MOVE_CONTROL_SPEED_LERP: f32 = 0.125;

/// Vanilla `Drowned`.
///
/// Steel has no inheritance, so this owns the shared `Zombie` body
/// ([`ZombieEntity::new_with_variant`]) and adds the drowned-only behaviour.
#[entity_behavior(class = "Drowned")]
pub struct DrownedEntity {
    zombie: ZombieEntity,
    searching_for_land: AtomicBool,
    /// Vanilla `Drowned.rangedAttackUncertainty`; `-1.0` means "use the
    /// `RangedAttackMob` default for the level's difficulty".
    ranged_attack_uncertainty: SyncMutex<f32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `DrownedEntity`.
unsafe impl DowncastType for DrownedEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/drowned");
}

impl DrownedEntity {
    /// Creates a new drowned at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a drowned from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let drowned = Self {
            zombie: ZombieEntity::new_with_variant(base, entity_type, ZombieVariant::Drowned),
            searching_for_land: AtomicBool::new(false),
            ranged_attack_uncertainty: SyncMutex::new(-1.0),
        };
        // Vanilla `Drowned`'s constructor.
        drowned.zombie.set_pathfinding_malus(PathType::Water, 0.0);
        drowned.register_drowned_goals();
        drowned
    }

    /// The shared `Zombie` body carrying the drowned's state.
    #[must_use]
    pub const fn zombie(&self) -> &ZombieEntity {
        &self.zombie
    }

    /// Vanilla `Drowned.addBehaviourGoals`; `Zombie.registerGoals` already added the
    /// float, look-at-player and random-look-around goals this keeps, so only the
    /// goals vanilla `Drowned` does not inherit are removed first.
    fn register_drowned_goals(&self) {
        self.remove_all_goals(|goal| {
            is_goal::<MeleeAttackGoal>(goal) || is_goal::<WaterAvoidingRandomStrollGoal>(goal)
        });
        self.mob_base()
            .target_selector()
            .lock()
            .remove_all_goals(self, |goal| {
                is_goal::<HurtByTargetGoal>(goal) || is_goal::<NearestAttackableTargetGoal>(goal)
            });

        let mut goal_selector = self.mob_base().goal_selector().lock();
        goal_selector.add_goal(1, DrownedGoToWaterGoal::new(1.0));
        goal_selector.add_goal(2, DrownedTridentAttackGoal::new(1.0, 40, 10.0));
        goal_selector.add_goal(2, DrownedAttackGoal::new(1.0, false));
        goal_selector.add_goal(5, DrownedGoToBeachGoal::new(1.0));
        goal_selector.add_goal(6, DrownedSwimUpGoal::new(1.0));
        goal_selector.add_goal(7, RandomStrollGoal::new(1.0));

        let mut target_selector = self.mob_base().target_selector().lock();
        // Vanilla `new HurtByTargetGoal(this, Drowned.class).setAlertOthers(ZombifiedPiglin.class)`:
        // drowned do not retaliate against other drowned. Steel has no
        // `ZombifiedPiglin` class yet (same documented omission as `zombie.rs`), and it
        // is not a `Drowned`, so the alert-others exclusion list stays empty.
        target_selector.add_goal(
            1,
            HurtByTargetGoal::new()
                .with_ignored_damage_types([<DrownedEntity as DowncastType>::TYPE_KEY])
                .set_alert_others(iter::empty()),
        );
        target_selector.add_goal(
            2,
            NearestAttackableTargetGoal::new_for_players_with_interval(10, true, false, ok_target),
        );
        target_selector.add_goal(
            3,
            NearestAttackableTargetGoal::new(false, |entity, _| {
                entity.entity_type() == &vanilla_entities::VILLAGER
                    || entity.entity_type() == &vanilla_entities::WANDERING_TRADER
            }),
        );
        target_selector.add_goal(
            3,
            NearestAttackableTargetGoal::new(true, |entity, _| {
                entity.entity_type() == &vanilla_entities::IRON_GOLEM
            }),
        );
        target_selector.add_goal(
            3,
            NearestAttackableTargetGoal::new_with_interval(10, true, false, |entity, _| {
                entity.entity_type() == &vanilla_entities::AXOLOTL
            }),
        );
        target_selector.add_goal(
            5,
            NearestAttackableTargetGoal::new_with_interval(10, true, false, |entity, _| {
                entity.entity_type() == &vanilla_entities::TURTLE
                    && entity.is_baby()
                    && entity.on_ground()
            }),
        );
    }

    /// Vanilla `Drowned.okTarget`.
    #[must_use]
    pub fn ok_target(&self, target: Option<&SharedEntity>) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        target.is_some_and(|target| {
            target
                .as_living_entity()
                .is_some_and(|target| ok_target(target, world.as_ref()))
        })
    }

    /// Vanilla `Drowned.wantsToSwim`.
    #[must_use]
    pub fn wants_to_swim(&self) -> bool {
        if self.is_searching_for_land() {
            return true;
        }

        self.zombie
            .target()
            .and_then(|target| target.as_living_entity().map(Entity::is_in_water))
            .unwrap_or(false)
    }

    /// Vanilla `Drowned.setSearchingForLand`.
    pub fn set_searching_for_land(&self, searching_for_land: bool) {
        self.searching_for_land
            .store(searching_for_land, Ordering::Release);
    }

    /// Vanilla `Drowned.isSearchingForLand`.
    #[must_use]
    pub fn is_searching_for_land(&self) -> bool {
        self.searching_for_land.load(Ordering::Acquire)
    }

    /// Vanilla `Drowned.closeToNextPos`.
    fn close_to_next_pos(&self) -> bool {
        let target = {
            let navigation = self.zombie.mob_base().navigation().lock();
            navigation.path().map(Path::target)
        };
        target.is_some_and(|pos| {
            self.distance_to_sqr(DVec3::new(
                f64::from(pos.x()),
                f64::from(pos.y()),
                f64::from(pos.z()),
            )) < 4.0
        })
    }

    /// Vanilla `Drowned.rangedAttackUncertainty(Level)`.
    #[must_use]
    pub fn ranged_attack_uncertainty(&self, world: &World) -> f32 {
        let uncertainty = *self.ranged_attack_uncertainty.lock();
        if uncertainty < 0.0 {
            default_ranged_attack_uncertainty(world)
        } else {
            uncertainty
        }
    }

    /// Vanilla `Drowned.setRangedAttackUncertainty`.
    pub fn set_ranged_attack_uncertainty(&self, ranged_attack_uncertainty: f32) {
        *self.ranged_attack_uncertainty.lock() = ranged_attack_uncertainty;
    }

    /// Vanilla `Drowned.finalizeSpawn` tail: the nautilus-shell offhand and the
    /// zombie-nautilus jockey.
    ///
    /// Steel's spawner finalizes one mob at a time and `Zombie::finalizeSpawn` owns its
    /// own RNG, so this tail draws from a fresh seeded [`LegacyRandom`] instead of
    /// vanilla's single shared `level.getRandom()` stream.
    fn apply_drowned_spawn_tail(&self, world: &Arc<World>, spawn_reason: EntitySpawnReason) {
        let mut random = LegacyRandom::from_seed(rand::random());

        if self
            .zombie
            .equipment_in_slot(EquipmentSlot::OffHand)
            .is_empty()
            && random.next_f32() < NAUTILUS_SHELL_CHANCE
        {
            self.zombie.living_base().equipment().lock().set(
                EquipmentSlot::OffHand,
                ItemStack::new(&vanilla_items::NAUTILUS_SHELL),
            );
            self.set_guaranteed_drop(EquipmentSlot::OffHand);
        }

        self.try_spawn_zombie_nautilus_jockey(world, spawn_reason, &mut random);
    }

    /// Vanilla `Drowned.finalizeSpawn` jockey block.
    fn try_spawn_zombie_nautilus_jockey(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        random: &mut impl Random,
    ) {
        if !matches!(
            spawn_reason,
            EntitySpawnReason::Natural | EntitySpawnReason::Structure
        ) {
            return;
        }
        if !self
            .zombie
            .equipment_in_slot(EquipmentSlot::MainHand)
            .is(&vanilla_items::TRIDENT)
        {
            return;
        }
        if random.next_f32() >= ZOMBIE_NAUTILUS_JOCKEY_CHANCE {
            return;
        }
        if self.zombie.is_baby() {
            return;
        }
        if world
            .biome_at(self.zombie.block_position())
            .is_some_and(|biome| biome.has_tag(&BiomeTag::MORE_FREQUENT_DROWNED_SPAWNS))
        {
            return;
        }

        let position = self.zombie.position();
        let yaw = self.zombie.rotation().0;
        let Some(nautilus) = ENTITIES.create(
            &vanilla_entities::ZOMBIE_NAUTILUS,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        ) else {
            return;
        };
        let Some(nautilus_mob) = nautilus.as_mob() else {
            return;
        };

        if spawn_reason == EntitySpawnReason::Structure {
            nautilus_mob.set_persistence_required();
        }
        nautilus_mob.snap_to(position, yaw, 0.0);
        nautilus_mob.finalize_spawn(world, spawn_reason, None);
        self.start_riding(&nautilus);
        if let Err(error) = world.try_add_entity(Arc::clone(&nautilus)) {
            log::warn!("failed to add drowned zombie nautilus jockey mount: {error}");
        }
    }

    /// Vanilla `Drowned.checkDrownedSpawnRules`, the `SpawnPlacements` predicate for
    /// `EntityType.DROWNED`.
    ///
    /// Steel's rule helpers need the live [`World`] (light layers, biome and sea
    /// level) rather than vanilla's `ServerLevelAccessor`, so the signature matches
    /// `world::spawn::placement::SpawnPredicate` with a `&World` level.
    #[must_use]
    pub fn check_drowned_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        random: &mut RandomSource,
    ) -> bool {
        if !level
            .get_block_state(pos.below())
            .get_fluid_state()
            .is_water()
            && !spawn_reason.is_spawner()
        {
            return false;
        }

        let can_monster_spawn = level.difficulty() != Difficulty::Peaceful
            && (spawn_reason.ignores_light_requirements()
                || <DrownedEntity as Monster>::is_dark_enough_to_spawn(level, pos, &mut *random))
            && (spawn_reason.is_spawner()
                || level.get_block_state(pos).get_fluid_state().is_water());
        let is_frequent_biome = level
            .biome_at(pos)
            .is_some_and(|biome| biome.has_tag(&BiomeTag::MORE_FREQUENT_DROWNED_SPAWNS));

        if !can_monster_spawn
            || (!spawn_reason.is_spawner() && spawn_reason != EntitySpawnReason::Reinforcement)
        {
            return if is_frequent_biome {
                random.next_i32_bounded(FREQUENT_DROWNED_SPAWN_ROLL) == 0 && can_monster_spawn
            } else {
                random.next_i32_bounded(RARE_DROWNED_SPAWN_ROLL) == 0
                    && is_deep_enough_to_spawn(level, pos)
                    && can_monster_spawn
            };
        }

        true
    }
}

/// Vanilla `RangedAttackMob.rangedAttackUncertainty` default:
/// `14 - level.getDifficulty().getId() * 4`.
fn default_ranged_attack_uncertainty(world: &World) -> f32 {
    14.0 - f32::from(world.difficulty() as u8) * 4.0
}

/// Vanilla `Drowned.isDeepEnoughToSpawn`.
fn is_deep_enough_to_spawn(level: &dyn LevelReader, pos: BlockPos) -> bool {
    pos.y() < level.sea_level() - DEEP_SPAWN_SEA_LEVEL_MARGIN
}

/// Vanilla `Drowned.okTarget`: friendly ground targets are only hunted in water
/// once daylight makes the drowned's land attack unreliable.
fn ok_target(target: &dyn LivingEntity, world: &World) -> bool {
    !world.is_bright_outside() || target.is_in_water()
}

/// Vanilla `Mth.rotLerp`: shortest-arc rotation lerp clamped to `max_delta`.
fn rot_lerp(from: f32, to: f32, max_delta: f32) -> f32 {
    from + wrap_degrees(to - from).clamp(-max_delta, max_delta)
}

/// The `DrownedEntity` behind a `&dyn PathfinderMob`, for the goals that drive
/// drowned-only state (vanilla reads it from their outer `Drowned` instance).
fn drowned_of(mob: &dyn PathfinderMob) -> Option<&DrownedEntity> {
    mob.downcast_ref::<DrownedEntity>()
}

/// Sets `Drowned.searchingForLand` from a goal's mob handle.
fn set_searching_for_land(mob: &dyn PathfinderMob, searching_for_land: bool) {
    if let Some(drowned) = drowned_of(mob) {
        drowned.set_searching_for_land(searching_for_land);
    }
}

/// Vanilla `Drowned.DrownedGoToWaterGoal`.
struct DrownedGoToWaterGoal {
    speed_modifier: f64,
    wanted_position: Option<DVec3>,
}

impl DrownedGoToWaterGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            wanted_position: None,
        }
    }

    /// Vanilla `Drowned.DrownedGoToWaterGoal.getWaterPos`.
    fn water_pos(mob: &dyn PathfinderMob, level: &World) -> Option<DVec3> {
        let pos = mob.block_position();
        let mut random = LegacyRandom::from_seed(rand::random());
        for _ in 0..WATER_POS_ATTEMPTS {
            let random_pos = pos.offset(
                random.next_i32_bounded(20) - 10,
                2 - random.next_i32_bounded(8),
                random.next_i32_bounded(20) - 10,
            );
            if level.get_block_state(random_pos).get_block() == &vanilla_blocks::WATER {
                let (x, y, z) = random_pos.get_bottom_center();
                return Some(DVec3::new(x, y, z));
            }
        }
        None
    }
}

impl Goal for DrownedGoToWaterGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        if !world.is_bright_outside() {
            return false;
        }
        if mob.is_in_water() {
            return false;
        }

        let Some(position) = Self::water_pos(mob, world.as_ref()) else {
            return false;
        };
        self.wanted_position = Some(position);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.mob_base().navigation().lock().is_done()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(position) = self.wanted_position {
            mob.move_to_pos(position, self.speed_modifier);
        }
    }
}

/// Vanilla `Drowned.DrownedTridentAttackGoal` (`RangedAttackGoal` plus the
/// main-hand trident gate and the item-use/aggressive flags).
struct DrownedTridentAttackGoal {
    ranged: RangedAttackGoal,
}

impl DrownedTridentAttackGoal {
    const fn new(speed_modifier: f64, attack_interval: i32, attack_radius: f32) -> Self {
        Self {
            ranged: RangedAttackGoal::new(speed_modifier, attack_interval, attack_radius),
        }
    }
}

impl Goal for DrownedTridentAttackGoal {
    fn controls(&self) -> GoalControls {
        self.ranged.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.ranged.can_use(mob)
            && mob
                .equipment_in_slot(EquipmentSlot::MainHand)
                .is(&vanilla_items::TRIDENT)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.ranged.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.ranged.start(mob);
        mob.set_aggressive(true);
        mob.start_using_item(InteractionHand::MainHand);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.ranged.stop(mob);
        mob.stop_using_item();
        mob.set_aggressive(false);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.ranged.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.ranged.tick(mob, entity);
    }
}

/// Vanilla `Drowned.DrownedAttackGoal` (`ZombieAttackGoal` plus the `okTarget`
/// gate).
struct DrownedAttackGoal {
    zombie_attack: ZombieAttackGoal,
}

impl DrownedAttackGoal {
    const fn new(speed_modifier: f64, track_target: bool) -> Self {
        Self {
            zombie_attack: ZombieAttackGoal::new(speed_modifier, track_target),
        }
    }
}

impl Goal for DrownedAttackGoal {
    fn controls(&self) -> GoalControls {
        self.zombie_attack.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.zombie_attack.can_use(mob) && goal_ok_target(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.zombie_attack.can_continue_to_use(mob) && goal_ok_target(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.zombie_attack.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.zombie_attack.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.zombie_attack.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.zombie_attack.tick(mob, entity);
    }
}

/// Vanilla `Drowned.okTarget(this.getTarget())` for the goal's mob handle.
fn goal_ok_target(mob: &dyn PathfinderMob) -> bool {
    let Some(drowned) = drowned_of(mob) else {
        return false;
    };
    drowned.ok_target(mob.target().as_ref())
}

/// Vanilla `Drowned.DrownedGoToBeachGoal` (`MoveToBlockGoal` plus the night/water
/// gate; the target validity predicate is passed to the embedded goal).
struct DrownedGoToBeachGoal {
    move_to_block: MoveToBlockGoal,
}

impl DrownedGoToBeachGoal {
    fn new(speed_modifier: f64) -> Self {
        // Vanilla `new DrownedGoToBeachGoal(this, 1.0)`: `MoveToBlockGoal(this, speed, 8, 2)`.
        Self {
            move_to_block: MoveToBlockGoal::with_vertical_search_range(
                speed_modifier,
                BEACH_SEARCH_RANGE,
                BEACH_VERTICAL_SEARCH_RANGE,
                beach_target_predicate,
            ),
        }
    }
}

impl Goal for DrownedGoToBeachGoal {
    fn controls(&self) -> GoalControls {
        self.move_to_block.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !self.move_to_block.can_use(mob) {
            return false;
        }

        let Some(world) = mob.level() else {
            return false;
        };
        !world.is_bright_outside()
            && mob.is_in_water()
            && mob.position().y >= f64::from(world.sea_level() - BEACH_SEA_LEVEL_MARGIN)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.move_to_block.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        set_searching_for_land(mob, false);
        self.move_to_block.start(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to_block.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to_block.tick(mob, entity);
    }
}

/// Vanilla `Drowned.DrownedSwimUpGoal`.
struct DrownedSwimUpGoal {
    speed_modifier: f64,
    stuck: bool,
}

impl DrownedSwimUpGoal {
    const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            stuck: false,
        }
    }
}

impl Goal for DrownedSwimUpGoal {
    fn controls(&self) -> GoalControls {
        // Vanilla `DrownedSwimUpGoal` never calls `setFlags`, so it claims no controls
        // and can be interrupted by any move goal.
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        !world.is_bright_outside()
            && mob.is_in_water()
            && mob.position().y < f64::from(world.sea_level() - SWIM_UP_START_SEA_LEVEL_MARGIN)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob) && !self.stuck
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        set_searching_for_land(mob, true);
        self.stuck = false;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        set_searching_for_land(mob, false);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(world) = mob.level() else {
            return;
        };
        let sea_level = world.sea_level();
        let position = mob.position();
        if position.y >= f64::from(sea_level - SWIM_UP_TARGET_SEA_LEVEL_MARGIN) {
            return;
        }
        let navigation_done = mob.mob_base().navigation().lock().is_done();
        let close_to_next_pos = drowned_of(mob).is_some_and(DrownedEntity::close_to_next_pos);
        if !navigation_done && !close_to_next_pos {
            return;
        }

        // Vanilla `DefaultRandomPos.getPosTowards(mob, 4, 8, seaLevel target, PI/2)`.
        // Steel's goal-module `random_pos` helpers are private to `ai::goal`, so this
        // uses the behaviour-local equivalent, which substitutes a walkability test
        // for the full pathfinder malus while keeping vanilla's 10-attempt contract.
        let Some(next_pos) = default_random_pos_towards(
            mob,
            SWIM_UP_HORIZONTAL_DIST,
            SWIM_UP_VERTICAL_DIST,
            DVec3::new(
                position.x,
                f64::from(sea_level - SWIM_UP_TARGET_SEA_LEVEL_MARGIN),
                position.z,
            ),
            FRAC_PI_2,
        ) else {
            self.stuck = true;
            return;
        };

        mob.move_to_pos(next_pos, self.speed_modifier);
    }
}

/// Vanilla `new DrownedGoToBeachGoal(this, 1.0)` → `MoveToBlockGoal(this, 1.0, 8, 2)`.
const BEACH_SEARCH_RANGE: i32 = 8;
const BEACH_VERTICAL_SEARCH_RANGE: i32 = 2;
/// Vanilla `Drowned.DrownedSwimUpGoal` `getPosTowards` distances.
const SWIM_UP_HORIZONTAL_DIST: i32 = 4;
const SWIM_UP_VERTICAL_DIST: i32 = 8;

/// Vanilla `Drowned.DrownedGoToBeachGoal.isValidTarget`: the block must have two
/// empty blocks above it and a full collision face on top
/// (`BlockState.entityCanStandOn`).
fn beach_target_predicate(level: &dyn LevelReader, pos: BlockPos) -> bool {
    let above = pos.above();
    if !level.get_block_state(above).is_air() || !level.get_block_state(above.above()).is_air() {
        return false;
    }

    is_offset_face_full(
        level.get_block_state(pos).get_collision_shape_at(pos),
        Direction::Up,
    )
}

impl RangedAttackMob for DrownedEntity {
    fn perform_ranged_attack(&self, _target: &SharedEntity, _power: f32) -> bool {
        // Vanilla `Drowned.performRangedAttack` throws the held trident (or a fresh
        // `Items.TRIDENT`) as a `ThrownTrident` owned by this drowned, aimed at
        // `(xd, yd + distanceToTarget * 0.2, zd)` with power `1.6`, and then always
        // plays `DROWNED_SHOOT` at volume `1.0` and pitch
        // `1 / (random * 0.4 + 0.8)` — the sound is not conditioned on the projectile.
        //
        // The projectile cannot be released yet, so this returns `false` (no projectile
        // left the hand) while still emitting the vanilla attack sound:
        //   * `ENTITIES` has no factory for `vanilla_entities::TRIDENT`; the thrown
        //     trident entity (`ThrownTrident`) is a separate slice, so this is
        //     vanilla's "the entity could not be created" case;
        //   * `RangedAttackMob::perform_ranged_attack` receives no `SharedEntity` for
        //     the shooter, so `Projectile::set_owner_entity` cannot be handed this
        //     drowned and the trident would immediately hit its own thrower.
        let mut random = LegacyRandom::from_seed(rand::random());
        self.play_sound(
            &sound_events::ENTITY_DROWNED_SHOOT,
            DROWNED_SHOOT_VOLUME,
            1.0 / (random.next_f32() * DROWNED_SHOOT_PITCH_SCALE + DROWNED_SHOOT_PITCH_BASE),
        );
        false
    }
}

/// Vanilla `Drowned.performRangedAttack`: `playSound(DROWNED_SHOOT, 1.0F, …)`.
const DROWNED_SHOOT_VOLUME: f32 = 1.0;
/// Vanilla `Drowned.performRangedAttack`: `1.0F / (getRandom().nextFloat() * 0.4F + 0.8F)`.
const DROWNED_SHOOT_PITCH_SCALE: f32 = 0.4;
const DROWNED_SHOOT_PITCH_BASE: f32 = 0.8;

impl Entity for DrownedEntity {
    fn base(&self) -> &EntityBase {
        self.zombie.base()
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.zombie.entity_type()
    }

    fn base_tick(&self) {
        self.zombie.base_tick();
    }

    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        // Vanilla `Drowned.BABY_DIMENSIONS` matches `Zombie`'s exactly.
        self.zombie.dimensions_for_pose(pose)
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        // Vanilla `Drowned` declares no synced data, so `DrownedEntityData` matches
        // the `Zombie` layout the shared body writes to.
        self.zombie.synced_data()
    }

    fn update_data_before_sync(&self) {
        self.zombie.update_data_before_sync();
    }

    fn max_up_step(&self) -> f32 {
        self.zombie.max_up_step()
    }

    fn sound_source(&self) -> SoundSource {
        self.zombie.sound_source()
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        self.zombie.fall_sounds()
    }

    fn swim_sound(&self) -> SoundEventRef {
        self.zombie.swim_sound()
    }

    fn play_step_sound(&self, pos: BlockPos, block_state: BlockStateId) {
        self.zombie.play_step_sound(pos, block_state);
    }

    /// Vanilla `Drowned.isPushedByFluid`.
    fn is_pushed_by_fluid(&self) -> bool {
        !self.zombie.is_swimming()
    }

    /// Vanilla `Drowned.updateSwimming`.
    fn update_swimming(&self) {
        // Vanilla guards this with `!level().isClientSide()`; Steel has no client side.
        self.zombie.set_shared_swimming(
            self.zombie.is_effective_ai() && self.zombie.is_under_water() && self.wants_to_swim(),
        );
    }

    /// Vanilla `Drowned.rideTick`: the drowned copies its mount's body rotation.
    fn ride_tick(self: Arc<Self>) {
        Arc::clone(&self).default_ride_tick();
        self.reset_fall_distance();

        if let Some(vehicle) = PathfinderMob::controlled_pathfinder_vehicle(&*self)
            && let Some(vehicle) = vehicle.as_living_entity()
        {
            self.set_y_body_rot(vehicle.y_body_rot());
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.zombie.save_additional(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.zombie.load_additional(nbt);
    }

    fn killed_entity(
        &self,
        world: &World,
        entity: &dyn LivingEntity,
        source: &DamageSource,
    ) -> bool {
        self.zombie.killed_entity(world, entity, source)
    }
}

impl LivingEntity for DrownedEntity {
    fn living_base(&self) -> &LivingEntityBase {
        self.zombie.living_base()
    }

    fn get_health(&self) -> f32 {
        self.zombie.get_health()
    }

    fn set_health(&self, health: f32) {
        self.zombie.set_health(health);
    }

    fn hurt_sound(&self, source: &DamageSource) -> Option<SoundEventRef> {
        self.zombie.hurt_sound(source)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        self.zombie.death_sound()
    }

    /// Vanilla `Drowned.travelInWater`: under water and wanting to swim, the drowned
    /// shoves itself along its look vector instead of taking the generic water path.
    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        base_gravity: f64,
        is_falling: bool,
        old_y: f64,
    ) -> Option<MoveResult> {
        if self.zombie.is_under_water() && self.wants_to_swim() {
            self.zombie
                .move_relative(DROWNED_SWIM_RELATIVE_SPEED, input);
            let result =
                Arc::clone(entity).move_entity(MoverType::SelfMovement, self.zombie.velocity())?;
            self.zombie
                .set_velocity(self.zombie.velocity() * DROWNED_SWIM_DRAG);
            return Some(result);
        }

        self.zombie
            .travel_in_water(entity, input, base_gravity, is_falling, old_y)
    }

    fn before_actually_hurt(&self, _source: &DamageSource, _amount: f32) {
        self.zombie.on_about_to_be_hurt();
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn base_experience_reward(&self) -> i32 {
        self.zombie.base_experience_reward()
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

/// Vanilla `Drowned.travelInWater`: `moveRelative(0.01F, input)`.
const DROWNED_SWIM_RELATIVE_SPEED: f32 = 0.01;
/// Vanilla `Drowned.travelInWater`: `setDeltaMovement(scale(0.9))`.
const DROWNED_SWIM_DRAG: f64 = 0.9;

impl Mob for DrownedEntity {
    fn mob_base(&self) -> &MobBase {
        self.zombie.mob_base()
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Drowned.DrownedMoveControl.tick`.
    fn tick_move_control(&self) {
        if !(self.wants_to_swim() && self.zombie.is_in_water()) {
            if !self.zombie.on_ground() {
                self.zombie
                    .set_velocity(self.zombie.velocity() + DVec3::new(0.0, -0.008, 0.0));
            }
            Mob::tick_move_control(&self.zombie);
            return;
        }

        let target_above = self
            .zombie
            .target()
            .is_some_and(|target| target.position().y > self.zombie.position().y);
        if target_above || self.is_searching_for_land() {
            self.zombie
                .set_velocity(self.zombie.velocity() + DVec3::new(0.0, 0.002, 0.0));
        }

        let (operation, wanted_position, speed_modifier) = {
            let controls = self.zombie.mob_base().controls().lock();
            (
                controls.move_control.operation(),
                controls.move_control.wanted_position(),
                controls.move_control.speed_modifier(),
            )
        };
        if operation != MoveControlOperation::MoveTo
            || self.zombie.mob_base().navigation().lock().is_done()
        {
            self.zombie.set_mob_speed(0.0);
            return;
        }

        let position = self.zombie.position();
        let xd = wanted_position.x - position.x;
        let yd = wanted_position.y - position.y;
        let zd = wanted_position.z - position.z;
        let dd = (xd * xd + yd * yd + zd * zd).sqrt();
        if dd <= 0.0 {
            // Vanilla divides by `dd` unguarded; reaching it means the wanted position
            // is already the current position, so idle instead of producing NaN.
            self.zombie.set_mob_speed(0.0);
            return;
        }
        let yd = yd / dd;

        let y_rot_d = (zd.atan2(xd) as f32).to_degrees() - 90.0;
        let (yaw, pitch) = self.zombie.rotation();
        let new_yaw = rot_lerp(yaw, y_rot_d, MOVE_CONTROL_MAX_TURN);
        self.zombie.set_rotation((new_yaw, pitch));
        self.zombie.living_base().set_y_body_rot(new_yaw);

        let movement_speed = self
            .zombie
            .attributes()
            .lock()
            .required_value(vanilla_attributes::MOVEMENT_SPEED);
        let target_speed = (speed_modifier * movement_speed) as f32;
        let new_speed = self.zombie.get_speed()
            + (target_speed - self.zombie.get_speed()) * MOVE_CONTROL_SPEED_LERP;
        self.zombie.set_mob_speed(new_speed);

        let push = DVec3::new(xd, yd, zd) * f64::from(new_speed);
        self.zombie.set_velocity(
            self.zombie.velocity()
                + DVec3::new(
                    push.x * MOVE_CONTROL_ACCELERATION_SCALE,
                    push.y * MOVE_CONTROL_VERTICAL_SCALE,
                    push.z * MOVE_CONTROL_ACCELERATION_SCALE,
                ),
        );
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        self.zombie.ambient_sound()
    }

    fn mob_flags(&self) -> i8 {
        self.zombie.mob_flags()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.zombie.set_mob_flags(flags);
    }

    fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        self.zombie.can_hold_item(item_stack)
    }

    /// Vanilla `Drowned.wantsToPickUp`: drowned never pick up spears.
    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        !item_stack.item().has_tag(&ItemTag::SPEARS) && self.zombie.wants_to_pick_up(item_stack)
    }

    /// Vanilla `Drowned.canReplaceCurrentItem`: the nautilus-shell offhand is never
    /// replaced.
    fn can_replace_current_item(
        &self,
        new_item_stack: &ItemStack,
        current_item_stack: &ItemStack,
        slot: EquipmentSlot,
    ) -> bool {
        !current_item_stack.is(&vanilla_items::NAUTILUS_SHELL)
            && self
                .zombie
                .can_replace_current_item(new_item_stack, current_item_stack, slot)
    }

    /// Vanilla `Drowned.getPreferredWeaponType`.
    fn get_preferred_weapon_type(&self) -> Option<Identifier> {
        Some(ItemTag::DROWNED_PREFERRED_WEAPONS)
    }

    /// Vanilla `Drowned.checkSpawnObstruction`: `level.isUnobstructed(this)` only
    /// tests other blocking entities, unlike `Mob`'s liquid-inclusive default.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        let aabb = self.bounding_box();
        !world.has_entity_in_aabb_matching(&aabb, |entity| {
            entity.id() != self.id()
                && !entity.is_removed()
                && entity.blocks_building()
                && !entity.is_passenger_of_same_vehicle(self.as_entity_event_source())
        })
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        self.zombie.remove_when_far_away(dist_sqr)
    }

    fn after_hurt_target(&self, world: &World, target: &SharedEntity, was_hurt: bool) {
        Mob::after_hurt_target(&self.zombie, world, target, was_hurt);
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.zombie.finalize_spawn(world, spawn_reason, group_data);
        self.apply_drowned_spawn_tail(world, spawn_reason);
        group_data
    }
}

impl Monster for DrownedEntity {}

impl PathfinderMob for DrownedEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::vanilla_item_tags::ItemTag;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};

    use super::DrownedEntity;
    use crate::entity::{Entity, Mob};
    use crate::inventory::equipment::EquipmentSlot;
    use steel_registry::item_stack::ItemStack;

    fn drowned() -> DrownedEntity {
        init_vanilla_registry();
        DrownedEntity::new(&vanilla_entities::DROWNED, 1, DVec3::ZERO, Weak::new())
    }

    #[test]
    fn drowned_never_converts_in_water_and_converts_to_drowned() {
        let drowned = drowned();

        assert_eq!(drowned.entity_type(), &vanilla_entities::DROWNED);
        assert!(!drowned.zombie().converts_in_water());
        assert_eq!(
            drowned.zombie().converts_to_when_drowning().key,
            vanilla_entities::DROWNED.key
        );
    }

    #[test]
    fn drowned_keeps_its_nautilus_shell_and_refuses_spears() {
        let drowned = drowned();

        assert!(!drowned.wants_to_pick_up(&ItemStack::new(&vanilla_items::IRON_SPEAR)));
        assert!(drowned.wants_to_pick_up(&ItemStack::new(&vanilla_items::TRIDENT)));
        assert!(!drowned.can_replace_current_item(
            &ItemStack::new(&vanilla_items::IRON_SWORD),
            &ItemStack::new(&vanilla_items::NAUTILUS_SHELL),
            EquipmentSlot::OffHand,
        ));
        assert_eq!(
            drowned.get_preferred_weapon_type(),
            Some(ItemTag::DROWNED_PREFERRED_WEAPONS)
        );
    }

    #[test]
    fn drowned_registers_its_own_goal_and_target_sets() {
        let drowned = drowned();

        let goal_count = drowned
            .zombie()
            .mob_base()
            .goal_selector()
            .lock()
            .available_goal_count();
        let target_count = drowned
            .zombie()
            .mob_base()
            .target_selector()
            .lock()
            .available_goal_count();

        // Float(0), GoToWater(1), TridentAttack(2), Attack(2), GoToBeach(5),
        // SwimUp(6), RandomStroll(7), LookAtPlayer(8), RandomLookAround(8).
        assert_eq!(goal_count, 9);
        // HurtByTarget(1), Player(2), Villager(3), IronGolem(3), Axolotl(3), Turtle(5).
        assert_eq!(target_count, 6);
    }
}
