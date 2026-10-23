//! Vanilla `Spider` (`net.minecraft.world.entity.monster.spider.Spider`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SpiderEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_mob_effects};
use steel_utils::locks::SyncMutex;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    AvoidEntityGoal, FloatGoal, Goal, GoalControls, HurtByTargetGoal, LeapAtTargetGoal,
    LookAtPlayerGoal, MeleeAttackGoal, NearestAttackableTargetGoal, RandomLookAroundGoal,
    WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::mob::LIGHT_MAGIC_VALUE_BRIGHTNESS_GATE;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Spider.DATA_FLAGS_ID` climbing bit.
const FLAG_CLIMBING: i8 = 1;
/// Vanilla `Spider.SPIDER_SPECIAL_EFFECT_CHANCE`.
const SPIDER_SPECIAL_EFFECT_CHANCE: f32 = 0.1;
/// Vanilla `Spider.finalizeSpawn` skeleton-jockey chance (`random.nextInt(100) == 0`).
const SKELETON_JOCKEY_BOUND: i32 = 100;
/// Vanilla `SpiderAttackGoal` daylight drop chance (`random.nextInt(100) == 0`).
const DAYLIGHT_TARGET_DROP_BOUND: i32 = 100;

/// Vanilla `Spider`.
#[entity_behavior(class = "Spider")]
pub struct SpiderEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<SpiderEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SpiderEntity`.
unsafe impl DowncastType for SpiderEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/spider");
}

impl SpiderEntity {
    /// Creates a new spider at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a spider from persisted base entity state.
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
        let mut entity_data = SpiderEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.flags.set(0);

        {
            // Vanilla `Spider.registerGoals` order and priorities.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, FloatGoal::new(&mob_base));
            goal_selector.add_goal(
                2,
                AvoidEntityGoal::with_selector(6.0, 1.0, 1.2, |target, _world| {
                    target.entity_type() == &vanilla_entities::ARMADILLO
                }),
            );
            goal_selector.add_goal(3, LeapAtTargetGoal::new(0.4));
            goal_selector.add_goal(4, SpiderAttackGoal::new());
            goal_selector.add_goal(5, WaterAvoidingRandomStrollGoal::new(0.8));
            goal_selector.add_goal(6, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(6, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
            target_selector.add_goal(2, SpiderTargetGoal::new_player());
            target_selector.add_goal(3, SpiderTargetGoal::new_iron_golem());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
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

    /// Vanilla `Spider.isClimbing`.
    #[must_use]
    pub fn is_climbing(&self) -> bool {
        self.entity_data.lock().flags.get() & FLAG_CLIMBING != 0
    }

    /// Vanilla `Spider.setClimbing`.
    pub fn set_climbing(&self, climbing: bool) {
        let mut entity_data = self.entity_data.lock();
        let flags = *entity_data.flags.get();
        entity_data.flags.set(if climbing {
            flags | FLAG_CLIMBING
        } else {
            flags & !FLAG_CLIMBING
        });
    }

    /// Vanilla `LivingEntity.getLightLevelDependentMagicValue` at the spider's eye position.
    fn light_level_dependent_magic_value(&self) -> f32 {
        let Some(world) = self.level() else {
            return 0.0;
        };
        let eye_pos = BlockPos::new(
            steel_math::fast_floor(self.position().x),
            steel_math::fast_floor(self.get_eye_y()),
            steel_math::fast_floor(self.position().z),
        );
        world.light_level_dependent_magic_value(eye_pos)
    }

    /// Vanilla `Spider.finalizeSpawn`: skeleton jockey plus the hard-difficulty random effect.
    ///
    /// Vanilla propagates the effect through `SpiderEffectsGroupData`; Steel's
    /// [`SpawnGroupData`] only models ageable mobs, so the effect is applied directly to this
    /// spider (identical result for a single spawn) and the group data stays `None`.
    fn finalize_spawn_spider(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla calls `super.finalizeSpawn` before the jockey and effect rolls.
        let group_data = self.finalize_spawn_mob_base(world, spawn_reason, group_data);

        if rand::random_range(0..SKELETON_JOCKEY_BOUND) == 0
            && let Some(skeleton) = crate::entity::ENTITIES.create(
                &vanilla_entities::SKELETON,
                crate::entity::next_entity_id(),
                self.position(),
                Arc::downgrade(world),
            )
        {
            if let Some(skeleton_mob) = skeleton.as_mob() {
                skeleton_mob.snap_to(self.position(), self.rotation().0, 0.0);
                skeleton_mob.finalize_spawn(world, spawn_reason, None);
                self.start_riding(&skeleton);
            }
            if let Err(error) = world.try_add_entity(Arc::clone(&skeleton)) {
                log::warn!("could not spawn the spider's skeleton jockey: {error}");
            }
        }

        if world.difficulty() == Difficulty::Hard
            && rand::random::<f32>() < SPIDER_SPECIAL_EFFECT_CHANCE
        {
            let effect = match rand::random_range(0..5) {
                0 | 1 => &vanilla_mob_effects::SPEED,
                2 => &vanilla_mob_effects::STRENGTH,
                3 => &vanilla_mob_effects::REGENERATION,
                _ => &vanilla_mob_effects::INVISIBILITY,
            };
            self.add_mob_effect(MobEffectInstance::new(effect, 0));
        }

        group_data
    }
}

/// Vanilla `Spider.SpiderAttackGoal extends MeleeAttackGoal`.
///
/// Vanilla subclasses `MeleeAttackGoal`; Steel's `MeleeAttackGoal` is a sealed concrete goal, so
/// this wraps it and adds the two overrides (no vehicle, daylight target drop).
pub(crate) struct SpiderAttackGoal {
    inner: MeleeAttackGoal,
}

impl SpiderAttackGoal {
    /// Vanilla `new SpiderAttackGoal(this)` → `super(mob, 1.0, true)`.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            inner: MeleeAttackGoal::new(1.0, true),
        }
    }

    fn is_daylight(mob: &dyn PathfinderMob) -> bool {
        SpiderEntity::spider_light_at_eye(mob) >= LIGHT_MAGIC_VALUE_BRIGHTNESS_GATE
    }
}

impl Goal for SpiderAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        !mob.is_vehicle() && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if Self::is_daylight(mob) && rand::random_range(0..DAYLIGHT_TARGET_DROP_BOUND) == 0 {
            mob.set_target(None);
            return false;
        }
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `Spider.SpiderTargetGoal<T> extends NearestAttackableTargetGoal<T>`: spiders only
/// start hunting in darkness (`getLightLevelDependentMagicValue() < 0.5`).
pub(crate) struct SpiderTargetGoal {
    inner: NearestAttackableTargetGoal,
}

impl SpiderTargetGoal {
    pub(super) fn new_player() -> Self {
        Self {
            inner: NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
        }
    }

    pub(super) fn new_iron_golem() -> Self {
        Self {
            inner: NearestAttackableTargetGoal::new(true, |target, _world| {
                target.entity_type() == &vanilla_entities::IRON_GOLEM
            }),
        }
    }
}

impl Goal for SpiderTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if SpiderEntity::spider_light_at_eye(mob) >= LIGHT_MAGIC_VALUE_BRIGHTNESS_GATE {
            return false;
        }
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

impl Entity for SpiderEntity {
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
        self.tick_living_entity(&entity);
        // Vanilla `Spider.tick` mirrors horizontal collision into the climbing flag.
        self.set_climbing(self.horizontal_collision());
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
        Monster::sound_source_monster(self)
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_SPIDER_STEP, 0.15, 1.0);
    }

    /// Vanilla `Spider.onClimbable` returns the synced climbing flag.
    fn on_climbable(&self) -> bool {
        self.is_climbing()
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
    }
}

impl LivingEntity for SpiderEntity {
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
        Some(&sound_events::ENTITY_SPIDER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SPIDER_DEATH)
    }

    /// Vanilla `Spider.canBeAffected` rejects poison.
    fn can_be_affected(&self, effect: &MobEffectInstance) -> bool {
        if std::ptr::eq(effect.effect(), &*vanilla_mob_effects::POISON) {
            return false;
        }
        LivingEntity::default_can_be_affected(self, effect)
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

impl Mob for SpiderEntity {
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
        Some(&sound_events::ENTITY_SPIDER_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.finalize_spawn_spider(world, spawn_reason, group_data)
    }
}

impl Monster for SpiderEntity {}

impl PathfinderMob for SpiderEntity {}

impl SpiderEntity {
    /// Vanilla `getLightLevelDependentMagicValue` for the spider behind `mob`, if any.
    fn spider_light_at_eye(mob: &dyn PathfinderMob) -> f32 {
        mob.as_entity_event_source()
            .downcast_ref::<SpiderEntity>()
            .map_or(0.0, SpiderEntity::light_level_dependent_magic_value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::SpiderEntity;

    #[test]
    fn spider_climbing_flag_round_trips_through_synced_data() {
        init_vanilla_registry();
        let spider = SpiderEntity::new(&vanilla_entities::SPIDER, 1, DVec3::ZERO, Weak::new());

        assert!(!spider.is_climbing());
        spider.set_climbing(true);
        assert!(spider.is_climbing());
        assert_eq!(spider.entity_data.lock().flags.get() & 1, 1);

        spider.set_climbing(false);
        assert!(!spider.is_climbing());
        assert_eq!(spider.entity_data.lock().flags.get() & 1, 0);
    }
}
