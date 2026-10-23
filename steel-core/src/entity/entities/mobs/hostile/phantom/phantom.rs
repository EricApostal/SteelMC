//! Vanilla `Phantom` (`net.minecraft.world.entity.monster.Phantom`).
//!
//! Vanilla flies with `Phantom.PhantomMoveControl`/`PhantomLookControl` and a flying path
//! navigation; Steel has neither, so the phantom drives its own velocity toward the vanilla
//! `moveTargetPoint` (documented deviation: no obstacle avoidance while swooping).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PhantomEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::chunk::heightmap::HeightmapType;
use crate::entity::ai::goal::{Goal, GoalControls};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Phantom.PhantomAttackPlayerTargetGoal` scan range (squared).
const TARGET_RANGE_SQR: f64 = 64.0 * 64.0;
/// Vanilla `Phantom` sweep phase bounds (`(8 + random.nextInt(4)) * 20`).
const SWEEP_TICKS_MIN: i32 = 8;
const SWEEP_TICKS_RANGE: i32 = 4;
/// Vanilla `PhantomAttackStrategyGoal.start` initial sweep delay (`adjustedTickDelay(10)`).
const INITIAL_SWEEP_DELAY: i32 = 5;
/// Vanilla `Phantom` circle radii.
const CIRCLE_DISTANCE_MIN: f32 = 5.0;
const CIRCLE_DISTANCE_RANGE: f32 = 10.0;
const CIRCLE_DISTANCE_MAX: f32 = 15.0;
const CIRCLE_HEIGHT_MIN: f32 = -4.0;
const CIRCLE_HEIGHT_RANGE: f32 = 9.0;
/// Vanilla `Phantom` anchor heights.
const ANCHOR_ABOVE_TARGET_MIN: i32 = 20;
const ANCHOR_ABOVE_TARGET_RANGE: i32 = 20;
const ANCHOR_ABOVE_GROUND_MIN: i32 = 10;
const ANCHOR_ABOVE_GROUND_RANGE: i32 = 20;
/// Vanilla `Phantom` swoop sound volume.
const SWOOP_SOUND_VOLUME: f32 = 10.0;
/// Vanilla `Phantom` size clamp (`Mth.clamp(size, 0, 64)`).
const MAX_PHANTOM_SIZE: i32 = 64;
/// Vanilla `Phantom.updatePhantomSizeInfo`: attack damage is `6 + size`.
const PHANTOM_BASE_ATTACK_DAMAGE: f64 = 6.0;
/// Vanilla `Phantom` flying travel drag.
const PHANTOM_FLY_DRAG: f64 = 0.2;

/// Vanilla `Phantom.AttackPhase`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AttackPhase {
    /// Circling above the anchor point.
    #[default]
    Circle,
    /// Diving at the target.
    Swoop,
}

/// Vanilla `Phantom`.
#[entity_behavior(class = "Phantom")]
pub struct PhantomEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<PhantomEntityData>,
    /// Vanilla `Phantom.moveTargetPoint`.
    move_target_point: SyncMutex<DVec3>,
    /// Vanilla `Phantom.anchorPoint`.
    anchor_point: SyncMutex<Option<BlockPos>>,
    /// Vanilla `Phantom.attackPhase`.
    attack_phase: SyncMutex<AttackPhase>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PhantomEntity`.
unsafe impl DowncastType for PhantomEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/phantom");
}

impl PhantomEntity {
    /// Creates a new phantom entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved phantom entity.
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
        let mut entity_data = PhantomEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.id_size.set(0);

        {
            // Vanilla `Phantom.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, PhantomAttackStrategyGoal::new());
            goal_selector.add_goal(2, PhantomSweepAttackGoal::new());
            goal_selector.add_goal(3, PhantomCircleAroundAnchorGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, PhantomAttackPlayerTargetGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            move_target_point: SyncMutex::new(DVec3::ZERO),
            anchor_point: SyncMutex::new(None),
            attack_phase: SyncMutex::new(AttackPhase::Circle),
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

    /// Vanilla `Phantom.getPhantomSize`.
    #[must_use]
    pub fn phantom_size(&self) -> i32 {
        *self.entity_data.lock().id_size.get()
    }

    /// Vanilla `Phantom.setPhantomSize` plus `updatePhantomSizeInfo`.
    pub fn set_phantom_size(&self, size: i32) {
        let clamped = size.clamp(0, MAX_PHANTOM_SIZE);
        self.entity_data.lock().id_size.set(clamped);
        self.attributes().lock().set_base_value(
            vanilla_attributes::ATTACK_DAMAGE,
            PHANTOM_BASE_ATTACK_DAMAGE + f64::from(clamped),
        );
    }

    /// Vanilla `Phantom.anchorPoint`.
    #[must_use]
    pub fn anchor_point(&self) -> Option<BlockPos> {
        *self.anchor_point.lock()
    }

    /// Sets the vanilla `anchorPoint`.
    pub fn set_anchor_point(&self, anchor: Option<BlockPos>) {
        *self.anchor_point.lock() = anchor;
    }

    /// Vanilla `Phantom.moveTargetPoint`.
    #[must_use]
    pub fn move_target_point(&self) -> DVec3 {
        *self.move_target_point.lock()
    }

    /// Sets the vanilla `moveTargetPoint`.
    pub fn set_move_target_point(&self, point: DVec3) {
        *self.move_target_point.lock() = point;
    }

    /// Vanilla `Phantom.attackPhase`.
    #[must_use]
    pub fn attack_phase(&self) -> AttackPhase {
        *self.attack_phase.lock()
    }

    /// Sets the vanilla `attackPhase`.
    pub fn set_attack_phase(&self, phase: AttackPhase) {
        *self.attack_phase.lock() = phase;
    }

    /// Vanilla `Phantom` flying travel: accelerate toward `moveTargetPoint` with drag.
    fn tick_flying(&self) {
        let target = self.move_target_point();
        let delta = target - self.position();
        let Some(direction) = delta.try_normalize() else {
            return;
        };
        let velocity = self.velocity() * (1.0 - PHANTOM_FLY_DRAG) + direction * PHANTOM_FLY_DRAG;
        self.set_velocity(velocity);
    }

    fn anchor_or_block(&self) -> BlockPos {
        self.anchor_point().unwrap_or_else(|| self.block_position())
    }
}

/// Vanilla `Phantom.PhantomAttackPlayerTargetGoal`.
struct PhantomAttackPlayerTargetGoal {
    next_scan_tick: i32,
}

impl PhantomAttackPlayerTargetGoal {
    const fn new() -> Self {
        Self { next_scan_tick: 10 }
    }

    fn phantom(mob: &dyn PathfinderMob) -> Option<&PhantomEntity> {
        mob.as_entity_event_source().downcast_ref::<PhantomEntity>()
    }
}

impl Goal for PhantomAttackPlayerTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if Self::phantom(mob).is_none() {
            return false;
        }
        if self.next_scan_tick > 0 {
            self.next_scan_tick -= 1;
            return false;
        }
        self.next_scan_tick = 30;

        let Some(world) = mob.level() else {
            return false;
        };
        let Some(player) =
            world.nearest_player(mob.position(), TARGET_RANGE_SQR.sqrt(), |player| {
                !player.is_spectator()
            })
        else {
            return false;
        };

        let player_entity: SharedEntity = player.into_shared_entity();
        mob.set_target(Some(&player_entity));
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target()
            .and_then(|target| target.as_living_entity().map(LivingEntity::is_alive))
            .unwrap_or(false)
    }
}

/// Vanilla `Phantom.PhantomAttackStrategyGoal`.
struct PhantomAttackStrategyGoal {
    next_sweep_tick: i32,
}

impl PhantomAttackStrategyGoal {
    const fn new() -> Self {
        Self { next_sweep_tick: 0 }
    }

    fn phantom(mob: &dyn PathfinderMob) -> Option<&PhantomEntity> {
        mob.as_entity_event_source().downcast_ref::<PhantomEntity>()
    }
}

impl Goal for PhantomAttackStrategyGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target()
            .and_then(|target| target.as_living_entity().map(LivingEntity::is_alive))
            .unwrap_or(false)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(phantom) = Self::phantom(mob) else {
            return;
        };
        self.next_sweep_tick = INITIAL_SWEEP_DELAY;
        phantom.set_attack_phase(AttackPhase::Circle);
        set_anchor_above_target(phantom);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let Some(phantom) = Self::phantom(mob) else {
            return;
        };
        if let Some(anchor) = phantom.anchor_point()
            && let Some(world) = mob.level()
        {
            // Vanilla `getHeightmapPos(MOTION_BLOCKING, anchor).above(10 + random.nextInt(20))`.
            let height = LevelReader::height_at(
                world.as_ref(),
                HeightmapType::MotionBlocking,
                anchor.x(),
                anchor.z(),
            );
            phantom.set_anchor_point(Some(BlockPos::new(
                anchor.x(),
                height + ANCHOR_ABOVE_GROUND_MIN + rand::random_range(0..ANCHOR_ABOVE_GROUND_RANGE),
                anchor.z(),
            )));
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(phantom) = Self::phantom(mob) else {
            return;
        };
        if phantom.attack_phase() != AttackPhase::Circle {
            return;
        }

        self.next_sweep_tick -= 1;
        if self.next_sweep_tick > 0 {
            return;
        }

        phantom.set_attack_phase(AttackPhase::Swoop);
        set_anchor_above_target(phantom);
        self.next_sweep_tick = (SWEEP_TICKS_MIN + rand::random_range(0..SWEEP_TICKS_RANGE)) * 20;
        phantom.play_sound(
            &sound_events::ENTITY_PHANTOM_SWOOP,
            SWOOP_SOUND_VOLUME,
            0.95 + rand::random::<f32>() * 0.1,
        );
    }
}

/// Vanilla `PhantomAttackStrategyGoal.setAnchorAboveTarget`.
fn set_anchor_above_target(phantom: &PhantomEntity) {
    if phantom.anchor_point().is_none() {
        return;
    }
    let Some(target) = phantom.target() else {
        return;
    };
    let mut y = target.block_position().y()
        + ANCHOR_ABOVE_TARGET_MIN
        + rand::random_range(0..ANCHOR_ABOVE_TARGET_RANGE);
    if let Some(world) = phantom.level() {
        y = y.max(LevelReader::sea_level(world.as_ref()) + 1);
    }
    phantom.set_anchor_point(Some(BlockPos::new(
        target.block_position().x(),
        y,
        target.block_position().z(),
    )));
}

/// Vanilla `Phantom.PhantomCircleAroundAnchorGoal`.
struct PhantomCircleAroundAnchorGoal {
    angle: f32,
    distance: f32,
    height: f32,
    clockwise: f32,
}

impl PhantomCircleAroundAnchorGoal {
    const fn new() -> Self {
        Self {
            angle: 0.0,
            distance: CIRCLE_DISTANCE_MIN,
            height: 0.0,
            clockwise: 1.0,
        }
    }

    fn phantom(mob: &dyn PathfinderMob) -> Option<&PhantomEntity> {
        mob.as_entity_event_source().downcast_ref::<PhantomEntity>()
    }

    fn select_next(&mut self, phantom: &PhantomEntity) {
        if phantom.anchor_point().is_none() {
            phantom.set_anchor_point(Some(phantom.block_position()));
        }
        self.angle += self.clockwise * 15.0 * (f32::consts::PI / 180.0);
        let anchor = phantom.anchor_or_block();
        phantom.set_move_target_point(DVec3::new(
            f64::from(anchor.x()) + f64::from(self.distance * self.angle.cos()),
            f64::from(anchor.y()) + f64::from(CIRCLE_HEIGHT_MIN + self.height),
            f64::from(anchor.z()) + f64::from(self.distance * self.angle.sin()),
        ));
    }
}

impl Goal for PhantomCircleAroundAnchorGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(phantom) = Self::phantom(mob) else {
            return false;
        };
        phantom.target().is_none() || phantom.attack_phase() == AttackPhase::Circle
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(phantom) = Self::phantom(mob) else {
            return;
        };
        self.distance = CIRCLE_DISTANCE_MIN + rand::random::<f32>() * CIRCLE_DISTANCE_RANGE;
        self.height = CIRCLE_HEIGHT_MIN + rand::random::<f32>() * CIRCLE_HEIGHT_RANGE;
        self.clockwise = if rand::random::<bool>() { 1.0 } else { -1.0 };
        self.select_next(phantom);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(phantom) = Self::phantom(mob) else {
            return;
        };

        if rand::random_range(0..175) == 0 {
            self.height = CIRCLE_HEIGHT_MIN + rand::random::<f32>() * CIRCLE_HEIGHT_RANGE;
        }
        if rand::random_range(0..125) == 0 {
            self.distance += 1.0;
            if self.distance > CIRCLE_DISTANCE_MAX {
                self.distance = CIRCLE_DISTANCE_MIN;
                self.clockwise = -self.clockwise;
            }
        }
        if rand::random_range(0..225) == 0 {
            self.angle = rand::random::<f32>() * 2.0 * f32::consts::PI;
            self.select_next(phantom);
        }
        if mob.position().distance_squared(phantom.move_target_point()) < 1.0 {
            self.select_next(phantom);
        }

        if let Some(world) = mob.level() {
            let position = mob.position();
            if phantom.move_target_point().y < position.y
                && !world
                    .get_block_state(mob.block_position().offset(0, -1, 0))
                    .is_air()
            {
                self.height = self.height.max(1.0);
                self.select_next(phantom);
            }
            if phantom.move_target_point().y > position.y
                && !world
                    .get_block_state(mob.block_position().offset(0, 1, 0))
                    .is_air()
            {
                self.height = self.height.min(-1.0);
                self.select_next(phantom);
            }
        }
    }
}

/// Vanilla `Phantom.PhantomSweepAttackGoal`.
struct PhantomSweepAttackGoal {
    cat_search_tick: i32,
    scared_of_cat: bool,
}

impl PhantomSweepAttackGoal {
    const fn new() -> Self {
        Self {
            cat_search_tick: 0,
            scared_of_cat: false,
        }
    }

    fn phantom(mob: &dyn PathfinderMob) -> Option<&PhantomEntity> {
        mob.as_entity_event_source().downcast_ref::<PhantomEntity>()
    }
}

impl Goal for PhantomSweepAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(phantom) = Self::phantom(mob) else {
            return false;
        };
        phantom.target().is_some() && phantom.attack_phase() == AttackPhase::Swoop
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };
        if !LivingEntity::is_alive(target_living) {
            return false;
        }
        if target_living.is_spectator() {
            return false;
        }
        if !self.can_use(mob) {
            return false;
        }

        if self.cat_search_tick < mob.base().tick_count() {
            self.cat_search_tick = mob.base().tick_count() + 20;
            let mut found_cat = false;
            if let Some(world) = mob.level() {
                let search_box = mob.bounding_box().inflate_xyz(16.0, 16.0, 16.0);
                found_cat = world
                    .nearest_entity_in_aabb_matching(&search_box, mob.position(), |entity| {
                        entity.entity_type() == &vanilla_entities::CAT
                    })
                    .is_some();
            }
            self.scared_of_cat = found_cat;
        }

        !self.scared_of_cat
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        mob.set_target(None);
        if let Some(phantom) = Self::phantom(mob) {
            phantom.set_attack_phase(AttackPhase::Circle);
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(phantom) = Self::phantom(mob) else {
            return;
        };
        let Some(target) = mob.target() else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };

        let target_position =
            DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
        phantom.set_move_target_point(target_position);

        if mob.bounding_box().intersects(target.bounding_box()) {
            Mob::do_hurt_target(phantom, entity, &world, &target);
            phantom.set_attack_phase(AttackPhase::Circle);
        }
    }
}

impl Entity for PhantomEntity {
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
        self.tick_flying();
        self.tick_living_entity(&entity);
    }

    /// Vanilla `Phantom.getDefaultDimensions` scales by `1 + 0.15 * size`.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        self.entity_type
            .dimensions
            .scale(1.0 + 0.15 * self.phantom_size() as f32)
            .scale(scale)
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
        SoundSource::Hostile
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    /// Vanilla `Phantom.onClimbable` is `false`.
    fn on_climbable(&self) -> bool {
        false
    }

    /// Vanilla `Phantom.checkFallDamage` does nothing.
    fn cause_fall_damage(
        self: Arc<Self>,
        _fall_distance: f64,
        _damage_modifier: f32,
        _source: &DamageSource,
    ) -> bool {
        false
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        if let Some(anchor) = self.anchor_point() {
            nbt.insert("anchor_pos_x", anchor.x());
            nbt.insert("anchor_pos_y", anchor.y());
            nbt.insert("anchor_pos_z", anchor.z());
        }
        nbt.insert("size", self.phantom_size());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let (Some(x), Some(y), Some(z)) = (
            nbt.int("anchor_pos_x"),
            nbt.int("anchor_pos_y"),
            nbt.int("anchor_pos_z"),
        ) {
            self.set_anchor_point(Some(BlockPos::new(x, y, z)));
        }
        self.set_phantom_size(nbt.int("size").unwrap_or(0));
    }
}

impl LivingEntity for PhantomEntity {
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
        Some(&sound_events::ENTITY_PHANTOM_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PHANTOM_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for PhantomEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PHANTOM_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Phantom.finalizeSpawn` anchors the phantom above its spawn position.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_anchor_point(Some(self.block_position().offset(0, 5, 0)));
        self.set_phantom_size(0);
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }
}

impl Monster for PhantomEntity {}

impl PathfinderMob for PhantomEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::{AttackPhase, PhantomEntity};
    use crate::entity::{Entity, LivingEntity};

    #[test]
    fn phantom_size_scales_attack_damage_and_nbt() {
        init_vanilla_registry();
        let phantom = PhantomEntity::new(&vanilla_entities::PHANTOM, 1, DVec3::ZERO, Weak::new());

        assert_eq!(phantom.phantom_size(), 0);
        assert_eq!(phantom.attack_phase(), AttackPhase::Circle);

        phantom.set_phantom_size(4);
        assert_eq!(phantom.phantom_size(), 4);
        assert_eq!(
            phantom
                .attributes()
                .lock()
                .get_value(steel_registry::vanilla_attributes::ATTACK_DAMAGE),
            Some(10.0)
        );

        let mut nbt = simdnbt::owned::NbtCompound::new();
        phantom.save_additional(&mut nbt);
        assert_eq!(nbt.int("size"), Some(4));
        assert_eq!(phantom.entity_type(), &vanilla_entities::PHANTOM);
    }
}
