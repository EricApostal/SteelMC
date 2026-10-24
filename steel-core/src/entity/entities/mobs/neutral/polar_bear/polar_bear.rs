//! Vanilla `PolarBear` (`net.minecraft.world.entity.animal.polarbear.PolarBear`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PolarBearEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    FloatGoal, FollowParentGoal, Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal,
    MeleeAttackGoal, NearestAttackableTargetGoal, PanicGoal, RandomLookAroundGoal,
    RandomStrollGoal, ResetUniversalAngerTargetGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::neutral_mob::{NeutralMob, PersistentAnger};
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 1;
/// Vanilla `PolarBear.PERSISTENT_ANGER_TIME` (`rangeOfSeconds(20, 39)`).
const PERSISTENT_ANGER_TIME_MIN: i64 = 20 * 20;
const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;
/// Vanilla `PolarBear.standAnimationTicks` warning cooldown.
const WARNING_SOUND_COOLDOWN: i32 = 40;
/// Vanilla `PolarBearAttackPlayersGoal` attack range and standing gates.
const STANDING_DISTANCE_MARGIN: f64 = 3.0;

/// Vanilla `PolarBear`.
#[entity_behavior(class = "PolarBear")]
pub struct PolarBearEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<PolarBearEntityData>,
    persistent_anger: PersistentAnger,
    /// Vanilla `PolarBear.playingWarningSound` cooldown.
    warning_sound_ticks: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PolarBearEntity`.
unsafe impl DowncastType for PolarBearEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/polar_bear");
}

impl PolarBearEntity {
    /// Creates a new polar bear entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved polar bear entity.
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
        let mut entity_data = PolarBearEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.standing.set(false);

        {
            // Vanilla `PolarBear.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, PolarBearMeleeAttackGoal::new());
            goal_selector.add_goal(1, PanicGoal::new(2.0));
            goal_selector.add_goal(4, FollowParentGoal::new(1.25));
            goal_selector.add_goal(5, RandomStrollGoal::new(1.0));
            goal_selector.add_goal(6, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(7, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players_with_interval(
                    20,
                    true,
                    true,
                    |_, _| true,
                ),
            );
            // Vanilla targets foxes when adult.
            target_selector.add_goal(
                4,
                NearestAttackableTargetGoal::new_with_interval(10, true, true, |target, _world| {
                    target.entity_type() == &vanilla_entities::FOX
                }),
            );
            target_selector.add_goal(5, ResetUniversalAngerTargetGoal::new(false));
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
            warning_sound_ticks: SyncMutex::new(0),
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

    /// Vanilla `PolarBear.isStanding`.
    #[must_use]
    pub fn is_standing(&self) -> bool {
        *self.entity_data.lock().standing.get()
    }

    /// Vanilla `PolarBear.setStanding`.
    pub fn set_standing(&self, standing: bool) {
        self.entity_data.lock().standing.set(standing);
    }

    /// Vanilla `PolarBear.getStandingAnimationScale`.
    #[must_use]
    pub fn standing_animation_scale(&self) -> f32 {
        if self.is_standing() { 1.0 } else { 0.0 }
    }

    /// Vanilla `PolarBear.playWarningSound`.
    pub fn play_warning_sound(&self) {
        let mut ticks = self.warning_sound_ticks.lock();
        if *ticks <= 0 {
            *ticks = WARNING_SOUND_COOLDOWN;
            drop(ticks);
            self.play_sound(&sound_events::ENTITY_POLAR_BEAR_WARNING, 1.0, 1.0);
        }
    }
}

/// Vanilla `PolarBear.PolarBearMeleeAttackGoal`.
struct PolarBearMeleeAttackGoal {
    inner: MeleeAttackGoal,
}

impl PolarBearMeleeAttackGoal {
    const fn new() -> Self {
        Self {
            inner: MeleeAttackGoal::new(1.25, true),
        }
    }

    fn bear(mob: &dyn PathfinderMob) -> Option<&PolarBearEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<PolarBearEntity>()
    }
}

impl Goal for PolarBearMeleeAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(bear) = Self::bear(mob) {
            bear.set_standing(false);
        }
        self.inner.stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.inner.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        // Vanilla `PolarBearMeleeAttackGoal.checkAndPerformAttack`: stand up shortly before
        // striking, and stand down when in range or out of range.
        if let Some(bear) = Self::bear(mob)
            && let Some(target) = mob.target()
        {
            let target_width = target.bounding_box().width();
            let strike_distance = (target_width + STANDING_DISTANCE_MARGIN).powi(2);
            if mob.position().distance_squared(target.position()) < strike_distance {
                bear.set_standing(true);
                bear.play_warning_sound();
            } else {
                bear.set_standing(false);
            }
        }
        self.inner.tick(mob, entity);
    }
}

impl NeutralMob for PolarBearEntity {
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

impl Entity for PolarBearEntity {
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
        {
            let mut ticks = self.warning_sound_ticks.lock();
            if *ticks > 0 {
                *ticks -= 1;
            }
        }
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
            &sound_events::ENTITY_POLAR_BEAR_STEP,
            &sound_events::ENTITY_POLAR_BEAR_STEP,
        )
    }

    fn swim_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_POLAR_BEAR_AMBIENT
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_POLAR_BEAR_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.read_persistent_anger_save_data(&nbt);
    }
}

impl LivingEntity for PolarBearEntity {
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
        Some(&sound_events::ENTITY_POLAR_BEAR_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_POLAR_BEAR_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.update_persistent_anger(true);
        self.mob_ai_step(entity)
    }
}

impl Mob for PolarBearEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `PolarBear.getAmbientSound`: babies have their own call.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_POLAR_BEAR_AMBIENT_BABY
        } else {
            &sound_events::ENTITY_POLAR_BEAR_AMBIENT
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `PolarBear.finalizeSpawn`: the group data forces single-baby-free spawns.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    /// Vanilla `PolarBear.checkPolarBearSpawnRules` reduced to the standard animal rules (the
    /// alternate-block biome branch needs a biome-tag lookup this static rule cannot reach).
    fn check_mob_spawn_rules(
        entity_type: EntityTypeRef,
        level: &dyn LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        let _ = entity_type;
        <Self as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
    }
}

impl AgeableMob for PolarBearEntity {
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

impl Animal for PolarBearEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `PolarBear.isFood` always returns `false` (polar bears cannot be bred).
    fn is_food(&self, _item_stack: &ItemStack) -> bool {
        false
    }
}

impl PathfinderMob for PolarBearEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::PolarBearEntity;
    use crate::entity::Entity;

    #[test]
    fn polar_bear_standing_flag_round_trips() {
        init_vanilla_registry();
        let bear = PolarBearEntity::new(&vanilla_entities::POLAR_BEAR, 1, DVec3::ZERO, Weak::new());

        assert!(!bear.is_standing());
        bear.set_standing(true);
        assert!(bear.is_standing());
        assert_eq!(bear.standing_animation_scale(), 1.0);
        assert_eq!(bear.entity_type(), &vanilla_entities::POLAR_BEAR);
    }
}
