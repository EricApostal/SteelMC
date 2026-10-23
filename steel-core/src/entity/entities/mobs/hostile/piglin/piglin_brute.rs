//! Vanilla `PiglinBrute` (`net.minecraft.world.entity.monster.piglin.PiglinBrute`).
//!
//! Vanilla `PiglinBrute` extends `AbstractPiglin` and is brain-driven through
//! [`brain`] (`PiglinBruteAi`). It always spawns with a golden axe, never hunts, only
//! picks up golden axes and remembers its spawn position as `HOME`.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PiglinBruteEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::behavior::piglin::ai;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::path::PathType;
use crate::entity::conversion::ConversionParams;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::World;

use super::brain;

/// Vanilla `PiglinBrute` XP reward.
const XP_REWARD: i32 = 20;
/// Vanilla `AbstractPiglin.CONVERSION_TIME`.
const CONVERSION_TIME: i32 = 300;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `PiglinBrute`.
#[entity_behavior(class = "PiglinBrute")]
pub struct PiglinBruteEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<PiglinBruteEntityData>,
    brain: SyncMutex<Brain>,
    /// Vanilla `AbstractPiglin.timeInOverworld`.
    time_in_overworld: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PiglinBruteEntity`.
unsafe impl DowncastType for PiglinBruteEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/piglin_brute");
}

impl PiglinBruteEntity {
    /// Creates a new piglin brute entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved piglin brute entity.
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

        // Vanilla `AbstractPiglin` constructor.
        mob_base.set_can_pick_up_loot(true);
        mob_base.navigation().lock().set_can_open_doors(true);
        {
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(PathType::FireInNeighbor, 16.0);
            malus.set(PathType::Fire, -1.0);
        }

        let mut entity_data = PiglinBruteEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(brain::make_piglin_brute_brain()),
            time_in_overworld: SyncMutex::new(0),
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

    /// Vanilla `AbstractPiglin.isImmuneToZombification`.
    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        *self.entity_data.lock().immune_to_zombification.get()
    }

    /// Vanilla `AbstractPiglin.setImmuneToZombification`.
    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.entity_data.lock().immune_to_zombification.set(immune);
    }

    /// Vanilla `AbstractPiglin.isConverting`.
    #[must_use]
    pub fn is_converting(&self) -> bool {
        !self.is_immune_to_zombification()
            && !self.is_no_ai()
            && self
                .level()
                .is_some_and(|world| world.dimension_type.piglins_zombify)
    }

    /// Vanilla `PiglinBrute.playAngrySound`.
    pub fn play_angry_sound(&self) {
        self.make_sound(Some(&sound_events::ENTITY_PIGLIN_BRUTE_ANGRY));
    }

    /// Vanilla `PiglinBrute.playConvertedSound`.
    fn play_converted_sound(&self) {
        self.make_sound(Some(
            &sound_events::ENTITY_PIGLIN_BRUTE_CONVERTED_TO_ZOMBIFIED,
        ));
    }

    /// Vanilla `AbstractPiglin.finishConversion`.
    fn finish_conversion(&self) {
        let converted = self.convert_to(
            &vanilla_entities::ZOMBIFIED_PIGLIN,
            ConversionParams::single(true, true),
            Some(&|zombified| {
                zombified.add_mob_effect(MobEffectInstance::with_duration(
                    &steel_registry::vanilla_mob_effects::NAUSEA,
                    200,
                    0,
                ));
            }),
        );
        if converted.is_some() {
            self.play_converted_sound();
        }
    }
}

impl Entity for PiglinBruteEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        let _ = pose;
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
        self.play_sound(&sound_events::ENTITY_PIGLIN_BRUTE_STEP, 0.15, 1.0);
    }

    /// Vanilla `PiglinBrute.hurtServer`: retaliate against non-piglins.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = LivingEntity::hurt_server(self, world, source, amount);
        if was_hurt
            && let Some(attacker) = source.causing_entity()
            && attacker.as_living_entity().is_some()
            && attacker.downcast_ref::<PiglinBruteEntity>().is_none()
            && attacker.downcast_ref::<super::PiglinEntity>().is_none()
        {
            ai::maybe_retaliate(world, self.as_entity_event_source(), attacker);
        }
        was_hurt
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert(
            "IsImmuneToZombification",
            i8::from(self.is_immune_to_zombification()),
        );
        nbt.insert("TimeInOverworld", *self.time_in_overworld.lock());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_can_pick_up_loot(nbt.byte("CanPickUpLoot").is_none_or(|value| value != 0));
        self.set_immune_to_zombification(
            nbt.byte("IsImmuneToZombification")
                .is_some_and(|value| value != 0),
        );
        *self.time_in_overworld.lock() = nbt.int("TimeInOverworld").unwrap_or(0);
    }
}

impl LivingEntity for PiglinBruteEntity {
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
        Some(&sound_events::ENTITY_PIGLIN_BRUTE_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PIGLIN_BRUTE_DEATH)
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

impl Mob for PiglinBruteEntity {
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
        Some(&sound_events::ENTITY_PIGLIN_BRUTE_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `PiglinBrute.customServerAiStep`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        let update = {
            let mut brain = self.brain.lock();
            brain::tick_piglin_brute(&mut brain, &world, entity);
            brain::update_piglin_brute_activity(&mut brain)
        };

        if update.activity_changed
            && self
                .brain
                .lock()
                .get_active_non_core_activity()
                .is_some_and(|activity| activity == Activity::Fight)
        {
            self.play_angry_sound();
        }

        brain::maybe_play_brute_activity_sound(entity);
        self.set_aggressive(update.aggressive);

        if self.is_converting() {
            let mut time = self.time_in_overworld.lock();
            *time += 1;
            if *time > CONVERSION_TIME {
                drop(time);
                self.finish_conversion();
            }
        } else {
            *self.time_in_overworld.lock() = 0;
        }
    }

    /// Vanilla `PiglinBrute.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        {
            let mut brain = self.brain.lock();
            brain::init_brute_memories(&mut brain, world, self.as_entity_event_source());
        }

        self.living_base.equipment().lock().set(
            EquipmentSlot::MainHand,
            ItemStack::new(&vanilla_items::GOLDEN_AXE),
        );
        self.refresh_equipment_attribute_modifiers(EquipmentSlot::MainHand);

        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    /// Vanilla `PiglinBrute.wantsToPickUp`: golden axes only.
    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        item_stack.is(&vanilla_items::GOLDEN_AXE) && Mob::wants_to_pick_up(self, item_stack)
    }

    /// Vanilla `PiglinBrute.getBaseExperienceReward`.
    fn base_experience_reward_mob(&self) -> i32 {
        XP_REWARD
    }

    fn mob_can_be_leashed(&self) -> bool {
        true
    }
}

impl Monster for PiglinBruteEntity {}

impl PathfinderMob for PiglinBruteEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};

    use super::PiglinBruteEntity;
    use crate::entity::{Entity, Mob};
    use steel_registry::item_stack::ItemStack;

    #[test]
    fn piglin_brute_only_picks_up_golden_axes() {
        init_vanilla_registry();
        let brute =
            PiglinBruteEntity::new(&vanilla_entities::PIGLIN_BRUTE, 1, DVec3::ZERO, Weak::new());

        assert!(brute.wants_to_pick_up(&ItemStack::new(&vanilla_items::GOLDEN_AXE)));
        assert!(!brute.wants_to_pick_up(&ItemStack::new(&vanilla_items::IRON_AXE)));
        assert!(!brute.wants_to_pick_up(&ItemStack::new(&vanilla_items::GOLD_INGOT)));
        assert_eq!(brute.entity_type(), &vanilla_entities::PIGLIN_BRUTE);
    }

    #[test]
    fn piglin_brute_zombification_state_round_trips() {
        init_vanilla_registry();
        let brute =
            PiglinBruteEntity::new(&vanilla_entities::PIGLIN_BRUTE, 1, DVec3::ZERO, Weak::new());

        brute.set_immune_to_zombification(true);
        let mut nbt = simdnbt::owned::NbtCompound::new();
        brute.save_additional(&mut nbt);
        assert_eq!(nbt.byte("IsImmuneToZombification"), Some(1));
    }
}
