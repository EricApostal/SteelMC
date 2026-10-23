//! Vanilla `Stray` (`net.minecraft.world.entity.monster.skeleton.Stray`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::StrayEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_blocks, vanilla_mob_effects};
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

use crate::entity::ai::goal::RangedAttackMob;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::skeleton::abstract_skeleton::{
    AbstractSkeleton, DEFAULT_STEP_HEIGHT, SkeletonArrow, skeleton_bases,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

/// Vanilla `Stray.getArrow`: slowness effect duration in ticks.
const STRAY_SLOWNESS_DURATION: i32 = 600;

/// Vanilla `Stray`.
#[entity_behavior(class = "Stray")]
pub struct StrayEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<StrayEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `StrayEntity`.
unsafe impl DowncastType for StrayEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/stray");
}

impl StrayEntity {
    /// Creates a new stray at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a stray from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    /// Vanilla `Stray.checkStraySpawnRules`: strays spawn in the open, and powder snow above
    /// the candidate is not counted as cover.
    ///
    /// Steel's `SpawnPlacements` table (`world/spawn/placement.rs`) has no `stray` row yet,
    /// so natural spawning does not consult this predicate; binding the row is outside this
    /// slice.
    #[must_use]
    pub fn check_stray_spawn_rules(
        entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        random: &mut RandomSource,
    ) -> bool {
        let mut check_sky_pos = pos;
        while level.get_block_state(check_sky_pos).get_block() == &vanilla_blocks::POWDER_SNOW {
            check_sky_pos = check_sky_pos.above();
        }

        <Self as Monster>::check_monster_spawn_rules(entity_type, level, spawn_reason, pos, random)
            && (spawn_reason.is_spawner() || level.can_see_sky(check_sky_pos.below()))
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let (living_base, mob_base) = skeleton_bases(entity_type);
        let mut entity_data = StrayEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
        };
        entity.setup_goals();
        entity
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
}

impl Entity for StrayEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
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

    fn play_step_sound(&self, pos: BlockPos, block_state: BlockStateId) {
        AbstractSkeleton::play_step_sound(self, pos, block_state);
    }

    fn ride_tick(self: Arc<Self>) {
        AbstractSkeleton::ride_tick(self);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);

        // Vanilla `AbstractSkeleton.readAdditionalSaveData` re-evaluates the weapon goal.
        self.reassess_weapon_goal();
    }
}

impl LivingEntity for StrayEntity {
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

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_STRAY_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_STRAY_DEATH)
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

impl Mob for StrayEntity {
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
        Some(&sound_events::ENTITY_STRAY_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        AbstractSkeleton::wants_to_pick_up(self, item_stack)
    }

    fn get_preferred_weapon_type(&self) -> Option<Identifier> {
        AbstractSkeleton::get_preferred_weapon_type(self)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        AbstractSkeleton::finalize_spawn(self, world, spawn_reason, group_data)
    }
}

impl Monster for StrayEntity {}

impl PathfinderMob for StrayEntity {}

impl AbstractSkeleton for StrayEntity {
    fn get_step_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_STRAY_STEP
    }

    fn get_arrow(
        &self,
        _projectile: &ItemStack,
        power: f32,
        _firing_weapon: Option<&ItemStack>,
    ) -> SkeletonArrow {
        // Vanilla `Stray.getArrow` adds slowness when the released arrow is a plain `Arrow`.
        let arrow = SkeletonArrow::base(power);
        SkeletonArrow {
            effect: Some(MobEffectInstance::with_duration(
                &vanilla_mob_effects::SLOWNESS,
                STRAY_SLOWNESS_DURATION,
                0,
            )),
            ..arrow
        }
    }
}

impl RangedAttackMob for StrayEntity {
    fn perform_ranged_attack(&self, target: &SharedEntity, power: f32) -> bool {
        AbstractSkeleton::perform_ranged_attack(self, target, power)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_mob_effects};

    use steel_registry::item_stack::ItemStack;

    use super::StrayEntity;
    use crate::entity::entities::mobs::skeleton::abstract_skeleton::AbstractSkeleton;
    use crate::entity::{Entity, SharedEntity};

    #[test]
    fn stray_implements_the_bow_capability_and_arrow_effect() {
        init_vanilla_registry();

        let stray = StrayEntity::new(&vanilla_entities::STRAY, 1, DVec3::ZERO, Weak::new());
        let arrow = stray.get_arrow(&ItemStack::empty(), 1.0, None);
        let Some(effect) = arrow.effect.as_ref() else {
            panic!("vanilla `Stray.getArrow` adds an effect");
        };
        assert_eq!(effect.effect().key, vanilla_mob_effects::SLOWNESS.key);
        assert_eq!(effect.duration(), 600);

        let stray_entity: SharedEntity = std::sync::Arc::new(stray);
        assert!(stray_entity.as_ranged_attack_mob().is_some());
    }
}
