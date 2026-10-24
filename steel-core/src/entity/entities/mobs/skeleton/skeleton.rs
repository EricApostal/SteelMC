//! Vanilla `Skeleton` (`net.minecraft.world.entity.monster.skeleton.Skeleton`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SkeletonEntityData;
use steel_registry::{level_events, sound_events, vanilla_attributes, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

use crate::entity::ai::goal::RangedAttackMob;
use crate::entity::conversion::ConversionParams;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::skeleton::abstract_skeleton::{
    AbstractSkeleton, DEFAULT_STEP_HEIGHT, skeleton_bases,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

/// Vanilla `Skeleton` freezing tracker: powder-snow ticks before the conversion starts.
const FREEZING_AFFLICTION_TIME: i32 = 140;
/// Vanilla `Skeleton` freezing tracker: conversion duration in ticks.
const STRAY_CONVERSION_TIME: i32 = 300;

/// Powder-snow conversion counters (vanilla `ConversionTracker`).
#[derive(Debug, Clone, Copy, Default)]
struct FreezingConversion {
    affliction_time: i32,
    conversion_time: i32,
}

/// Vanilla `Skeleton`.
#[entity_behavior(class = "Skeleton")]
pub struct SkeletonEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<SkeletonEntityData>,
    freezing: SyncMutex<FreezingConversion>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SkeletonEntity`.
unsafe impl DowncastType for SkeletonEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/skeleton");
}

impl SkeletonEntity {
    /// Creates a new skeleton at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a skeleton from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let (living_base, mob_base) = skeleton_bases(entity_type);
        let mut entity_data = SkeletonEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            freezing: SyncMutex::new(FreezingConversion::default()),
        };
        entity.setup_goals();
        entity
    }

    /// Vanilla `ConversionTracker.isConverting` for the stray conversion.
    #[must_use]
    pub fn is_converting(&self) -> bool {
        *self.entity_data.lock().stray_conversion.get()
    }

    /// Vanilla `Skeleton.startFreezeConversion` (test hook).
    pub fn start_freeze_conversion(&self, time: i32) {
        self.start_conversion(time);
    }

    fn start_conversion(&self, time: i32) {
        self.freezing.lock().conversion_time = time;
        self.entity_data.lock().stray_conversion.set(true);
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

    /// Vanilla `ConversionTracker.tick` for the stray conversion.
    fn tick_freezing_conversion(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if !LivingEntity::is_alive(self) || self.is_no_ai() {
            return;
        }

        if self.is_in_powder_snow() {
            if self.is_converting() {
                let finished = {
                    let mut freezing = self.freezing.lock();
                    freezing.conversion_time -= 1;
                    freezing.conversion_time < 0
                };
                if finished {
                    self.convert_to_stray(&world);
                }
            } else {
                let start = {
                    let mut freezing = self.freezing.lock();
                    freezing.affliction_time += 1;
                    freezing.affliction_time >= FREEZING_AFFLICTION_TIME
                };
                if start {
                    self.start_conversion(STRAY_CONVERSION_TIME);
                }
            }
        } else {
            self.freezing.lock().affliction_time = -1;
            self.entity_data.lock().stray_conversion.set(false);
        }
    }

    fn convert_to_stray(&self, world: &Arc<World>) {
        let converted = self.convert_to(
            &vanilla_entities::STRAY,
            ConversionParams::single(true, true),
            Some(&move |mob: &dyn Mob| {
                if !mob.is_silent() {
                    world.level_event(
                        level_events::SOUND_SKELETON_TO_STRAY,
                        mob.block_position(),
                        0,
                        None,
                    );
                }
            }),
        );
        if converted.is_none() {
            log::warn!("skeleton could not convert to stray");
        }
    }
}

impl Entity for SkeletonEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        // Vanilla `Skeleton.tick` advances the freezing tracker before `super.tick()`.
        self.tick_freezing_conversion();
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

        let freezing = *self.freezing.lock();
        nbt.insert(
            "StrayConversionTime",
            if self.is_converting() {
                freezing.conversion_time
            } else {
                -1
            },
        );
        nbt.insert(
            "FreezingTime",
            if self.is_in_powder_snow() {
                freezing.affliction_time
            } else {
                -1
            },
        );
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);

        self.freezing.lock().affliction_time = nbt.int("FreezingTime").unwrap_or(0);
        let conversion_time = nbt.int("StrayConversionTime").unwrap_or(-1);
        if conversion_time == -1 {
            self.entity_data.lock().stray_conversion.set(false);
        } else {
            self.start_conversion(conversion_time);
        }

        // Vanilla `AbstractSkeleton.readAdditionalSaveData` re-evaluates the weapon goal.
        self.reassess_weapon_goal();
    }

    fn can_freeze(&self) -> bool {
        // Vanilla `Skeleton.canFreeze`.
        false
    }
}

impl LivingEntity for SkeletonEntity {
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
        Some(&sound_events::ENTITY_SKELETON_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SKELETON_DEATH)
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

impl Mob for SkeletonEntity {
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
        Some(&sound_events::ENTITY_SKELETON_AMBIENT)
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

impl Monster for SkeletonEntity {}

impl PathfinderMob for SkeletonEntity {}

impl AbstractSkeleton for SkeletonEntity {
    fn get_step_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_SKELETON_STEP
    }

    fn is_shaking(&self) -> bool {
        // Vanilla `Skeleton.isShaking` reports the freezing tracker, not full freeze.
        self.is_converting()
    }
}

impl RangedAttackMob for SkeletonEntity {
    fn perform_ranged_attack(&self, target: &SharedEntity, power: f32) -> bool {
        AbstractSkeleton::perform_ranged_attack(self, target, power)
    }
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::SkeletonEntity;
    use crate::entity::Entity;

    #[test]
    fn skeleton_freezing_conversion_tracks_and_disables_freezing() {
        init_vanilla_registry();

        let skeleton =
            SkeletonEntity::new(&vanilla_entities::SKELETON, 1, DVec3::ZERO, Weak::new());
        assert!(
            !skeleton.can_freeze(),
            "vanilla `Skeleton.canFreeze` is false"
        );
        assert!(!skeleton.is_converting());

        skeleton.start_freeze_conversion(2);
        assert!(skeleton.is_converting());

        let mut nbt = NbtCompound::new();
        skeleton.save_additional(&mut nbt);
        assert_eq!(nbt.int("StrayConversionTime"), Some(2));
    }
}
