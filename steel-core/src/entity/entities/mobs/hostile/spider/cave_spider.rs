//! Vanilla `CaveSpider` (`net.minecraft.world.entity.monster.spider.CaveSpider`).

use std::ptr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::CaveSpiderEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_mob_effects};
use steel_utils::locks::SyncMutex;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    AvoidEntityGoal, FloatGoal, HurtByTargetGoal, LeapAtTargetGoal, LookAtPlayerGoal,
    RandomLookAroundGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

use super::spider::{SpiderAttackGoal, SpiderTargetGoal};

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const NORMAL_POISON_SECONDS: i32 = 7;
const HARD_POISON_SECONDS: i32 = 15;
const TICKS_PER_SECOND: i32 = 20;
const XP_REWARD: i32 = 5;

/// Vanilla `CaveSpider`.
#[entity_behavior(class = "CaveSpider")]
pub struct CaveSpiderEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<CaveSpiderEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CaveSpiderEntity`.
unsafe impl DowncastType for CaveSpiderEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/cave_spider");
}

impl CaveSpiderEntity {
    /// Creates a new cave spider at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a cave spider from persisted base entity state.
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
        let mut entity_data = CaveSpiderEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.spider_mut().flags.set(0);

        {
            // Vanilla `CaveSpider` inherits `Spider.registerGoals` unchanged.
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

    /// Vanilla `Spider.isClimbing` on the shared spider flag layer.
    #[must_use]
    pub fn is_climbing(&self) -> bool {
        self.entity_data.lock().spider().flags.get() & 1 != 0
    }

    /// Vanilla `Spider.setClimbing` on the shared spider flag layer.
    pub fn set_climbing(&self, climbing: bool) {
        let mut entity_data = self.entity_data.lock();
        let flags = *entity_data.spider().flags.get();
        entity_data
            .spider_mut()
            .flags
            .set(if climbing { flags | 1 } else { flags & !1 });
    }
}

impl Entity for CaveSpiderEntity {
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

impl LivingEntity for CaveSpiderEntity {
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

    /// Vanilla `CaveSpider` reuses the spider hurt/death sounds.
    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SPIDER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SPIDER_DEATH)
    }

    /// Vanilla `Spider.canBeAffected` rejects poison; inherited unchanged.
    fn can_be_affected(&self, effect: &MobEffectInstance) -> bool {
        if ptr::eq(effect.effect(), &raw const *vanilla_mob_effects::POISON) {
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

impl Mob for CaveSpiderEntity {
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

    /// Vanilla `CaveSpider.doHurtTarget` poison rider on the shared melee path.
    fn after_hurt_target(&self, world: &World, target: &SharedEntity, was_hurt: bool) {
        if !was_hurt {
            return;
        }
        let Some(target_living) = target.as_living_entity() else {
            return;
        };
        let poison_seconds = match world.difficulty() {
            Difficulty::Normal => NORMAL_POISON_SECONDS,
            Difficulty::Hard => HARD_POISON_SECONDS,
            _ => 0,
        };
        if poison_seconds > 0 {
            target_living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::POISON,
                poison_seconds * TICKS_PER_SECOND,
                0,
            ));
        }
    }

    /// Vanilla `CaveSpider.finalizeSpawn` returns the group data unchanged and
    /// never reaches `Mob.finalizeSpawn`, so the cave spider gets no
    /// random-spawn follow-range or left-handed roll.
    fn finalize_spawn(
        &self,
        _world: &Arc<World>,
        _spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        group_data
    }
}

impl Monster for CaveSpiderEntity {}

impl PathfinderMob for CaveSpiderEntity {
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        Monster::get_walk_target_value_monster(self, pos)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;
    use steel_registry::vanilla_mob_effects;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::CaveSpiderEntity;
    use crate::entity::Entity;

    #[test]
    fn cave_spider_starts_grounded_with_poison_immunity() {
        use crate::entity::{LivingEntity, MobEffectInstance};

        init_vanilla_registry();
        let spider =
            CaveSpiderEntity::new(&vanilla_entities::CAVE_SPIDER, 1, DVec3::ZERO, Weak::new());

        assert!(!spider.is_climbing());
        spider.set_climbing(true);
        assert!(spider.is_climbing());
        assert!(spider.on_climbable());

        let poison = MobEffectInstance::new(vanilla_mob_effects::POISON, 0);
        assert!(!spider.can_be_affected(&poison));
        assert_eq!(spider.entity_type(), &vanilla_entities::CAVE_SPIDER);
    }
}
