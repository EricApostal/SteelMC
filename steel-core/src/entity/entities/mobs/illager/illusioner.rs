//! Vanilla `Illusioner` (vanilla `net.minecraft.world.entity.monster.illager.Illusioner`).
//!
//! Vanilla splits the spell state between `SpellcasterIllager` (tick count, current spell,
//! arm pose) and `Illusioner` (mirror offsets, bow, sounds). Steel keeps both on this one
//! entity and reuses the shared goals from [`crate::entity::ai::goal::illager_spells`].
//!
//! Deliberate differences from vanilla:
//! - Mirror duplicates are server-side offsets only; vanilla's four client-side duplicates
//!   are spawned from `Illusioner.getIllusionOffsets` in the client entity and need a
//!   dedicated sync packet, which does not exist yet.
//! - No client particle emission: `ParticleTypes.CLOUD` on a mirror move and
//!   `ParticleTypes.ENTITY_EFFECT` while casting are client-only render effects.
//! - The `Illusioner.aiStep` hurt branch keys off `LivingEntity.hurtTime` server-side (the
//!   client reads the same field locally), so it runs from the shared hurt bookkeeping.
//! - `Attributes` come from the entity type's generated `default_attributes` table
//!   (`movement_speed` 0.5, `follow_range` 18.0, `max_health` 32.0), matching vanilla
//!   `Illusioner.createAttributes`.

use std::iter;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::IllusionerEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use super::abstract_illager::{IllagerArmPose, IllagerSpell, RaiderKeys};
use crate::entity::ai::goal::{
    AvoidEntityGoal, FloatGoal, HurtByTargetGoal, IllusionerBlindnessSpell, IllusionerMirrorSpell,
    LookAtPlayerGoal, NearestAttackableTargetGoal, RandomStrollGoal, RangedBowAttackGoal,
    SpellcasterCastingSpellGoal, SpellcasterUseSpellGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::raid::RaidSnapshot;
use crate::entity::raid::raider::{Raider, RaiderBase};
use crate::entity::{
    ENTITIES, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Illusioner.NUM_ILLUSIONS`.
const NUM_ILLUSIONS: usize = 4;
/// Vanilla `Illusioner.ILLUSION_TRANSITION_TICKS`.
const ILLUSION_TRANSITION_TICKS: i32 = 3;
/// Vanilla `Illusioner.ILLUSION_SPREAD`: offsets span `[-3.0, 3.0]` on both horizontal axes.
const ILLUSION_SPREAD: f64 = 3.0;
/// Vanilla `Illusioner.aiStep` mirror refresh period (`tickCount % 1200`).
const ILLUSION_REROLL_INTERVAL: i32 = 1200;

/// Vanilla illusioner entity.
#[entity_behavior(class = "Illusioner")]
pub struct IllusionerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    raider_base: RaiderBase,
    entity_data: SyncMutex<IllusionerEntityData>,
    /// Vanilla `SpellcasterIllager.spellCastingTickCount`.
    spell_casting_tick_count: SyncMutex<i32>,
    /// Vanilla `Illusioner.clientSideIllusionOffsets[1]`, kept server-side.
    mirror_offsets: SyncMutex<[DVec3; NUM_ILLUSIONS]>,
    /// Vanilla `Illusioner.clientSideIllusionTicks`, kept server-side.
    illusion_ticks: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `IllusionerEntity`.
unsafe impl DowncastType for IllusionerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/illusioner");
}

impl IllusionerEntity {
    /// Creates a new illusioner entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved illusioner entity.
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
        let mut entity_data = IllusionerEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `Illusioner.registerGoals` (on top of `Raider.registerGoals`).
            let mut goals = mob_base.goal_selector().lock();
            goals.add_goal(0, FloatGoal::new(&mob_base));
            goals.add_goal(1, SpellcasterCastingSpellGoal);
            // Vanilla goal 3 avoids `Creaking`; Steel only registers the avoidance once a
            // creaking factory exists (same guard as vindicator).
            if ENTITIES.has_factory(&vanilla_entities::CREAKING) {
                goals.add_goal(
                    3,
                    AvoidEntityGoal::with_selector(8.0, 1.0, 1.2, |target, _| {
                        target.entity_type() == &vanilla_entities::CREAKING
                    }),
                );
            }
            goals.add_goal(4, SpellcasterUseSpellGoal::new(IllusionerMirrorSpell));
            goals.add_goal(
                5,
                SpellcasterUseSpellGoal::new(IllusionerBlindnessSpell::default()),
            );
            goals.add_goal(6, RangedBowAttackGoal::new(0.5, 20, 15.0));
            goals.add_goal(8, RandomStrollGoal::new(0.6));
            goals.add_goal(9, LookAtPlayerGoal::new(3.0));
            goals.add_goal(
                10,
                LookAtPlayerGoal::new_for_living_entities(8.0, 1.0, |_, _| true),
            );

            let mut targets = mob_base.target_selector().lock();
            targets.add_goal(
                1,
                HurtByTargetGoal::new()
                    .with_ignored_damage_types(RaiderKeys::ALL)
                    .set_alert_others(iter::empty()),
            );
            targets.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true)
                    .with_unseen_memory_ticks(300),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new(
                    false,
                    super::abstract_illager::is_target_villager,
                )
                .with_unseen_memory_ticks(300),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new(
                    false,
                    super::abstract_illager::is_target_iron_golem,
                )
                .with_unseen_memory_ticks(300),
            );
        }

        super::raider_goals::register_raider_goals(&mob_base);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            raider_base: RaiderBase::new(),
            entity_data: SyncMutex::new(entity_data),
            spell_casting_tick_count: SyncMutex::new(0),
            mirror_offsets: SyncMutex::new([DVec3::ZERO; NUM_ILLUSIONS]),
            illusion_ticks: SyncMutex::new(0),
        }
    }

    /// Vanilla `SpellcasterIllager.isCastingSpell` server-side branch.
    #[must_use]
    pub fn is_casting_spell(&self) -> bool {
        self.spell_casting_tick_count() > 0
    }

    /// Vanilla `SpellcasterIllager.spellCastingTickCount`.
    #[must_use]
    pub fn spell_casting_tick_count(&self) -> i32 {
        *self.spell_casting_tick_count.lock()
    }

    /// Sets vanilla `SpellcasterIllager.spellCastingTickCount`.
    pub fn set_spell_casting_tick_count(&self, ticks: i32) {
        *self.spell_casting_tick_count.lock() = ticks;
    }

    /// Vanilla `SpellcasterIllager.setIsCastingSpell`: records the current spell and syncs
    /// its id through `DATA_SPELL_CASTING_ID`.
    pub fn set_is_casting_spell(&self, spell: IllagerSpell) {
        self.entity_data
            .lock()
            .spellcaster_illager_mut()
            .spell_casting
            .set(spell.id() as i8);
    }

    /// Vanilla `Illusioner.getCastingSoundEvent`.
    #[must_use]
    #[expect(
        clippy::unused_self,
        reason = "vanilla instance method reached through the `Spellcaster` UFCS shim"
    )]
    pub fn casting_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_ILLUSIONER_CAST_SPELL
    }

    /// Vanilla `Illusioner.aiStep` mirror reroll: four offsets inside the 3.0 spread.
    pub fn reroll_illusion_offsets(&self) {
        let mut offsets = self.mirror_offsets.lock();
        for offset in offsets.iter_mut() {
            *offset = DVec3::new(
                f64::from(rand::random_range(-6_i32..=6)) * 0.5,
                f64::from((rand::random_range(0_i32..6) - 4).max(0)),
                f64::from(rand::random_range(-6_i32..=6)) * 0.5,
            );
        }
        debug_assert!(
            offsets.iter().all(
                |offset| offset.x.abs() <= ILLUSION_SPREAD && offset.z.abs() <= ILLUSION_SPREAD
            )
        );
    }

    /// The current mirror offsets, one per vanilla illusion duplicate.
    #[must_use]
    pub fn illusion_offsets(&self) -> [DVec3; NUM_ILLUSIONS] {
        *self.mirror_offsets.lock()
    }

    /// Vanilla `Illusioner.getArmPose` through the `SpellcasterIllager` base branch.
    #[must_use]
    pub fn arm_pose(&self) -> IllagerArmPose {
        if self.is_casting_spell() {
            IllagerArmPose::Spellcasting
        } else if <Self as Raider>::is_celebrating(self) {
            IllagerArmPose::Celebrating
        } else {
            IllagerArmPose::Crossed
        }
    }

    /// Vanilla `Illusioner.aiStep` client-side mirror refresh, kept server-side.
    ///
    /// The two refresh branches are vanilla's: a hurt flash starting
    /// (`hurtTime == hurtDuration - 1`) collapses the duplicates onto the
    /// illusioner, while `hurtTime == 1` (the flash ending) or the 1200-tick
    /// periodic refresh rerolls them.
    fn tick_mirror_offsets(&self) {
        if !self.is_invisible() {
            return;
        }

        {
            let mut ticks = self.illusion_ticks.lock();
            *ticks = (*ticks - 1).max(0);
        }

        let hurt_time = self.living_base.hurt_time();
        if hurt_time == 1 || self.tick_count() % ILLUSION_REROLL_INTERVAL == 0 {
            *self.illusion_ticks.lock() = ILLUSION_TRANSITION_TICKS;
            self.reroll_illusion_offsets();
        } else if hurt_time == self.living_base.hurt_duration() - 1 {
            *self.illusion_ticks.lock() = ILLUSION_TRANSITION_TICKS;
            *self.mirror_offsets.lock() = [DVec3::ZERO; NUM_ILLUSIONS];
        }
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }
        let display = self.living_base.mob_effect_display_state();
        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.raider_mut().mob_mut().living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }
        self.entity_data.set_base_invisible_flag(display.invisible);
    }
}

impl Entity for IllusionerEntity {
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

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.raider_base.save(nbt);
        // Vanilla `SpellcasterIllager.addAdditionalSaveData`.
        nbt.insert("SpellTicks", self.spell_casting_tick_count());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(raid_id) = self.raider_base.load(&nbt) {
            self.rejoin_saved_raid(raid_id);
        }
        // Vanilla `SpellcasterIllager.readAdditionalSaveData`.
        *self.spell_casting_tick_count.lock() = nbt.int("SpellTicks").unwrap_or(0);
    }
}

impl LivingEntity for IllusionerEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self
            .entity_data
            .lock()
            .raider()
            .mob()
            .living_entity()
            .health
            .get()
    }

    fn set_health(&self, health: f32) {
        let clamped = health.clamp(0.0, self.get_max_health());
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ILLUSIONER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ILLUSIONER_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Raider::raider_ai_step(self, entity);
        let result = Monster::monster_ai_step(self, entity);
        self.tick_mirror_offsets();
        result
    }

    fn on_death(&self, source: &DamageSource) {
        Raider::raider_on_death(self, source);
    }
}

impl Mob for IllusionerEntity {
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
        Some(&sound_events::ENTITY_ILLUSIONER_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().raider().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .mob_flags
            .set(flags);
    }

    /// Vanilla `AbstractIllager.canAttack`: never attack a baby villager.
    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        // Vanilla's `super.canAttack` is `Mob.canAttack`, whose default cannot be reached from
        // an override; its body (ghast veto + `LivingEntity.canAttack`) is inlined here.
        super::abstract_illager::can_attack_villager(target, &world)
            && target.entity_type() != &vanilla_entities::GHAST
            && LivingEntity::can_attack(self, target)
    }

    /// Vanilla `SpellcasterIllager.customServerAiStep`.
    fn custom_server_ai_step(&self) {
        let mut ticks = self.spell_casting_tick_count.lock();
        if *ticks > 0 {
            *ticks -= 1;
        }
    }

    /// Vanilla `Illusioner.finalizeSpawn`: equip a bow in the main hand.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.living_base
            .equipment()
            .lock()
            .set(EquipmentSlot::MainHand, ItemStack::new(&vanilla_items::BOW));
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn requires_custom_persistence(&self) -> bool {
        Raider::raider_requires_custom_persistence(self)
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        Raider::raider_remove_when_far_away(self, dist_sqr)
    }
}

impl Monster for IllusionerEntity {}

impl PathfinderMob for IllusionerEntity {}

impl Raider for IllusionerEntity {
    fn raider_base(&self) -> &RaiderBase {
        &self.raider_base
    }

    fn is_celebrating(&self) -> bool {
        *self.entity_data.lock().raider().is_celebrating.get()
    }

    fn set_celebrating(&self, celebrating: bool) {
        self.entity_data
            .lock()
            .raider_mut()
            .is_celebrating
            .set(celebrating);
    }

    fn apply_raid_buffs(
        &self,
        _world: &Arc<World>,
        _raid: &RaidSnapshot,
        _wave: i32,
        _is_captain: bool,
    ) {
        // Vanilla `Illusioner.applyRaidBuffs` is an empty override: the illusioner gains no
        // equipment or enchantments from raid waves.
    }

    fn celebrate_sound(&self) -> SoundEventRef {
        // Vanilla `Illusioner.getCelebrateSound` returns `SoundEvents.ILLUSIONER_AMBIENT`;
        // there is no dedicated `ENTITY_ILLUSIONER_CELEBRATE` sound.
        &sound_events::ENTITY_ILLUSIONER_AMBIENT
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_mob_effects};

    use super::{IllusionerEntity, NUM_ILLUSIONS};
    use crate::entity::{Entity, Mob, init_entities};
    use crate::test_support::fresh_test_world;

    #[test]
    fn illusioner_can_attack_keeps_the_illager_veto_without_recursing() {
        init_vanilla_registry();
        init_entities();
        let world = fresh_test_world("illusioner_can_attack");
        let illusioner = IllusionerEntity::new(
            &vanilla_entities::ILLUSIONER,
            1,
            DVec3::ZERO,
            Arc::downgrade(&world.world),
        );
        let target = IllusionerEntity::new(
            &vanilla_entities::ILLUSIONER,
            2,
            DVec3::ZERO,
            Arc::downgrade(&world.world),
        );

        // Vanilla `AbstractIllager.canAttack` = baby-villager veto && `Mob.canAttack`. Before
        // the fix the `Mob::can_attack(self, ..)` fallback re-entered this override and
        // stack-overflowed whenever the entity had a level.
        assert!(illusioner.can_attack(&target));
    }

    #[test]
    fn illusioner_hurt_flash_collapses_the_mirror_offsets() {
        init_vanilla_registry();
        init_entities();
        let world = fresh_test_world("illusioner_hurt_mirror");
        let illusioner = IllusionerEntity::new(
            &vanilla_entities::ILLUSIONER,
            1,
            DVec3::ZERO,
            Arc::downgrade(&world.world),
        );
        illusioner
            .living_base
            .set_mob_effect(vanilla_mob_effects::INVISIBILITY, 0);
        illusioner.update_dirty_mob_effect_entity_data();
        assert!(illusioner.is_invisible());

        // Tick 0 is always a 1200-tick refresh tick; move off it so only the
        // hurt branches can run.
        illusioner.base().advance_tick_count();
        *illusioner.mirror_offsets.lock() = [DVec3::splat(1.5); NUM_ILLUSIONS];

        // Vanilla `LivingEntity.hurtServer` starts the flash at `hurtTime = 10`
        // and the tick decrement runs before `aiStep`, so the first `aiStep`
        // after a hit sees `hurtTime == hurtDuration - 1` and collapses the
        // duplicates onto the illusioner.
        assert!(
            illusioner
                .living_base
                .apply_damage_cooldown(1.0, false)
                .is_some()
        );
        illusioner.living_base.decrement_hurt_time();
        illusioner.tick_mirror_offsets();

        assert_eq!(illusioner.illusion_offsets(), [DVec3::ZERO; NUM_ILLUSIONS]);
    }
}
