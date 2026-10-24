//! Vanilla `WitherSkeleton`
//! (`net.minecraft.world.entity.monster.skeleton.WitherSkeleton`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::enchantment::table::EnchantmentRandom;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::WitherSkeletonEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{sound_events, vanilla_attributes, vanilla_items, vanilla_mob_effects};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

use std::ptr;

use crate::entity::ai::goal::RangedAttackMob;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::skeleton::abstract_skeleton::{
    AbstractSkeleton, DEFAULT_STEP_HEIGHT, SkeletonArrow, skeleton_bases,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::World;

/// Vanilla `WitherSkeleton.finalizeSpawn` attack damage.
const WITHER_SKELETON_ATTACK_DAMAGE: f64 = 4.0;
/// Vanilla `WitherSkeleton.doHurtTarget` wither duration in ticks.
const WITHER_EFFECT_DURATION: i32 = 200;
/// Vanilla `WitherSkeleton.getArrow`: `igniteForSeconds(100.0F)`.
const WITHER_ARROW_IGNITE_SECONDS: f32 = 100.0;
/// Vanilla `WitherSkeleton` constructor: `setPathfindingMalus(PathType.LAVA, 8.0F)`.
const LAVA_PATHFINDING_MALUS: f32 = 8.0;

/// Vanilla `WitherSkeleton`.
#[entity_behavior(class = "WitherSkeleton")]
pub struct WitherSkeletonEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<WitherSkeletonEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `WitherSkeletonEntity`.
unsafe impl DowncastType for WitherSkeletonEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/wither_skeleton");
}

impl WitherSkeletonEntity {
    /// Creates a new wither skeleton at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a wither skeleton from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let (living_base, mob_base) = skeleton_bases(entity_type);
        let mut entity_data = WitherSkeletonEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
        };
        // Vanilla `WitherSkeleton` runs `super(...)` before the lava pathfinding malus.
        entity.setup_goals();
        entity.set_pathfinding_malus(PathType::Lava, LAVA_PATHFINDING_MALUS);
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

impl Entity for WitherSkeletonEntity {
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

impl LivingEntity for WitherSkeletonEntity {
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
        Some(&sound_events::ENTITY_WITHER_SKELETON_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WITHER_SKELETON_DEATH)
    }

    fn can_be_affected(&self, effect: &MobEffectInstance) -> bool {
        // Vanilla `WitherSkeleton.canBeAffected` rejects wither.
        if ptr::eq(effect.effect(), vanilla_mob_effects::WITHER) {
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

impl Mob for WitherSkeletonEntity {
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
        Some(&sound_events::ENTITY_WITHER_SKELETON_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        // Vanilla `WitherSkeleton.canHoldItem` rejects disliked weapons; vanilla's
        // `super.canHoldItem` is `Mob`'s unconditional `true`.
        !item_stack
            .item()
            .has_tag(&ItemTag::WITHER_SKELETON_DISLIKED_WEAPONS)
    }

    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        AbstractSkeleton::wants_to_pick_up(self, item_stack)
    }

    fn get_preferred_weapon_type(&self) -> Option<Identifier> {
        // Vanilla `WitherSkeleton.getPreferredWeaponType` returns `null`.
        None
    }

    fn after_hurt_target(&self, _world: &World, target: &SharedEntity, was_hurt: bool) {
        if !was_hurt {
            return;
        }

        if let Some(target_living) = target.as_living_entity() {
            // Vanilla `WitherSkeleton.doHurtTarget` applies wither after a landed hit.
            target_living.add_mob_effect(MobEffectInstance::with_duration(
                vanilla_mob_effects::WITHER,
                WITHER_EFFECT_DURATION,
                0,
            ));
        }
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = AbstractSkeleton::finalize_spawn(self, world, spawn_reason, group_data);

        // Vanilla `WitherSkeleton.finalizeSpawn` tail.
        self.attributes().lock().set_base_value(
            vanilla_attributes::ATTACK_DAMAGE,
            WITHER_SKELETON_ATTACK_DAMAGE,
        );
        self.reassess_weapon_goal();
        group_data
    }
}

impl Monster for WitherSkeletonEntity {}

impl PathfinderMob for WitherSkeletonEntity {}

impl AbstractSkeleton for WitherSkeletonEntity {
    fn get_step_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_WITHER_SKELETON_STEP
    }

    fn populate_default_equipment_slots(
        &self,
        _random: &mut impl Random,
        _difficulty_modifier: f32,
    ) {
        // Vanilla `WitherSkeleton.populateDefaultEquipmentSlots` replaces the bow with a
        // stone sword.
        self.living_base().equipment().lock().set(
            EquipmentSlot::MainHand,
            ItemStack::new(&vanilla_items::STONE_SWORD),
        );
    }

    fn populate_default_equipment_enchantments(
        &self,
        _world: &World,
        _random: &mut (impl Random + EnchantmentRandom),
        _difficulty_modifier: f32,
    ) {
        // Vanilla `WitherSkeleton.populateDefaultEquipmentEnchantments` is empty.
    }

    fn get_arrow(
        &self,
        _projectile: &ItemStack,
        power: f32,
        _firing_weapon: Option<&ItemStack>,
    ) -> SkeletonArrow {
        // Vanilla `WitherSkeleton.getArrow` ignites the released arrow.
        let arrow = SkeletonArrow::base(power);
        SkeletonArrow {
            ignite_seconds: Some(WITHER_ARROW_IGNITE_SECONDS),
            ..arrow
        }
    }

    // Vanilla `WitherSkeleton.registerGoals` adds an `AbstractPiglin` target at priority 3
    // before the shared goals. Steel has no piglin entity classes yet, so that target is
    // omitted and lands with the piglin family (the same documented-omission style as
    // `zombie.rs`).
}

impl RangedAttackMob for WitherSkeletonEntity {
    fn perform_ranged_attack(&self, target: &SharedEntity, power: f32) -> bool {
        AbstractSkeleton::perform_ranged_attack(self, target, power)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        init_vanilla_registry, vanilla_attributes, vanilla_entities, vanilla_items,
    };

    use super::WitherSkeletonEntity;
    use crate::entity::{LivingEntity, Mob};

    #[test]
    fn wither_skeleton_spawn_equipment_and_immunities() {
        init_vanilla_registry();

        let wither_skeleton = WitherSkeletonEntity::new(
            &vanilla_entities::WITHER_SKELETON,
            1,
            DVec3::ZERO,
            Weak::new(),
        );

        assert!(Mob::get_preferred_weapon_type(&wither_skeleton).is_none());
        assert!(
            wither_skeleton.can_hold_item(&ItemStack::new(&vanilla_items::STONE_SWORD)),
            "a stone sword is not a disliked weapon"
        );
        assert!(
            !wither_skeleton.can_hold_item(&ItemStack::new(&vanilla_items::BOW)),
            "vanilla `WitherSkeleton.canHoldItem` rejects bows"
        );
        assert!(
            (wither_skeleton
                .attributes()
                .lock()
                .required_value(vanilla_attributes::ATTACK_DAMAGE)
                - 2.0)
                .abs()
                < f64::EPSILON,
            "the constructor keeps the 2.0 json default; `finalize_spawn` raises it to 4.0"
        );
    }
}
