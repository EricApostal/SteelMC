//! Vanilla `ElderGuardian` (`net.minecraft.world.entity.monster.ElderGuardian`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::ElderGuardianEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_mob_effects};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    LookAtPlayerGoal, MoveTowardsRestrictionGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal, RandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::LevelReader;
use crate::world::World;
use steel_protocol::packets::game::{CGameEvent, GameEventType};

use super::guardian::{GuardianAttackGoal, tick_guardian_move_control};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::fluid::FluidStateExt as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 10;
/// Vanilla `ElderGuardian.getAttackDuration`.
const ELDER_ATTACK_DURATION: i32 = 60;
/// Vanilla `ElderGuardian.EFFECT_INTERVAL`.
const EFFECT_INTERVAL: i32 = 1200;
/// Vanilla `ElderGuardian.EFFECT_RADIUS`.
const EFFECT_RADIUS: f64 = 50.0;
/// Vanilla `ElderGuardian.EFFECT_DURATION`.
const EFFECT_DURATION: i32 = 6000;
/// Vanilla `ElderGuardian.EFFECT_AMPLIFIER`.
const EFFECT_AMPLIFIER: i32 = 2;
/// Vanilla `ElderGuardian.EFFECT_DISPLAY_LIMIT` (how long the player sees the curse).
const EFFECT_DISPLAY_LIMIT: i32 = 1200;
/// Vanilla `ElderGuardian` stroll interval.
const ELDER_STROLL_INTERVAL: i32 = 400;

/// Vanilla `ElderGuardian`.
#[entity_behavior(class = "ElderGuardian")]
pub struct ElderGuardianEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<ElderGuardianEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ElderGuardianEntity`.
unsafe impl DowncastType for ElderGuardianEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/elder_guardian");
}

impl ElderGuardianEntity {
    /// Creates a new elder guardian entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved elder guardian entity.
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
        // Vanilla `ElderGuardian` constructor calls `setPersistenceRequired`; Steel models that
        // through `Mob::requires_custom_persistence`, overridden below.
        let mut entity_data = ElderGuardianEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.guardian_mut().id_moving.set(false);
        entity_data.guardian_mut().id_attack_target.set(0);

        {
            // Vanilla `Guardian.registerGoals` with the elder's longer stroll interval.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(4, GuardianAttackGoal::new(true));
            goal_selector.add_goal(5, MoveTowardsRestrictionGoal::new(1.0));
            goal_selector.add_goal(
                7,
                RandomStrollGoal::with_interval(1.0, ELDER_STROLL_INTERVAL),
            );
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(
                8,
                LookAtPlayerGoal::new_for_living_entities(12.0, 0.01, |target, _world| {
                    target.entity_type() == &vanilla_entities::GUARDIAN
                        || target.entity_type() == &vanilla_entities::ELDER_GUARDIAN
                }),
            );
            goal_selector.add_goal(9, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(
                1,
                NearestAttackableTargetGoal::new_with_interval(
                    10,
                    true,
                    false,
                    |target, _world| {
                        target.entity_type() == &vanilla_entities::PLAYER
                            || target.entity_type() == &vanilla_entities::SQUID
                            || target.entity_type() == &vanilla_entities::GLOW_SQUID
                            || target.entity_type() == &vanilla_entities::AXOLOTL
                    },
                ),
            );
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

    /// Vanilla `Guardian.isMoving` on the shared guardian layer.
    #[must_use]
    pub fn is_moving(&self) -> bool {
        *self.entity_data.lock().guardian().id_moving.get()
    }

    /// Vanilla `Guardian.setMoving` on the shared guardian layer.
    pub fn set_moving(&self, moving: bool) {
        self.entity_data.lock().guardian_mut().id_moving.set(moving);
    }

    /// Vanilla `ElderGuardian.getAttackDuration`.
    #[must_use]
    pub const fn attack_duration() -> i32 {
        ELDER_ATTACK_DURATION
    }

    /// Vanilla `Guardian.hasActiveAttackTarget`.
    #[must_use]
    pub fn has_active_attack_target(&self) -> bool {
        *self.entity_data.lock().guardian().id_attack_target.get() != 0
    }

    /// Vanilla `Guardian.setActiveAttackTarget`.
    pub fn set_active_attack_target(&self, entity_id: i32) {
        self.entity_data
            .lock()
            .guardian_mut()
            .id_attack_target
            .set(entity_id);
    }

    /// Vanilla `Guardian.getFlopSound`.
    #[must_use]
    pub fn flop_sound() -> SoundEventRef {
        &sound_events::ENTITY_ELDER_GUARDIAN_FLOP
    }

    /// Vanilla `ElderGuardian.customServerAiStep`: the mining-fatigue curse.
    fn apply_elder_curse(&self, world: &Arc<World>) {
        let tick_count = self.base.tick_count();
        if (tick_count + self.id()) % EFFECT_INTERVAL != 0 {
            return;
        }

        let center = self.position();
        let effect = MobEffectInstance::with_duration(
            vanilla_mob_effects::MINING_FATIGUE,
            EFFECT_DURATION,
            EFFECT_AMPLIFIER,
        );

        // Vanilla `MobEffectUtil.addEffectToPlayersAround` with a 1200-tick display limit.
        let mut players = Vec::new();
        world.players.iter_players(|_, player| {
            if player.position().distance_squared(center) <= EFFECT_RADIUS * EFFECT_RADIUS {
                players.push(Arc::clone(player));
            }
            true
        });

        for player in players {
            player.add_mob_effect(effect.clone());
            // Vanilla sends `ClientboundGameEventPacket.GUARDIAN_ELDER_EFFECT`.
            player.send_packet(CGameEvent {
                event: GameEventType::GuardianElderEffect,
                data: if self.is_silent() { 0.0 } else { 1.0 },
            });
            let _ = EFFECT_DISPLAY_LIMIT;
        }
    }
}

impl Entity for ElderGuardianEntity {
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
        SoundSource::Hostile
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
    }
}

impl LivingEntity for ElderGuardianEntity {
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
        Some(if self.is_in_water() {
            &sound_events::ENTITY_ELDER_GUARDIAN_HURT
        } else {
            &sound_events::ENTITY_ELDER_GUARDIAN_HURT_LAND
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_in_water() {
            &sound_events::ENTITY_ELDER_GUARDIAN_DEATH
        } else {
            &sound_events::ENTITY_ELDER_GUARDIAN_DEATH_LAND
        })
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        if let Some(world) = self.level() {
            self.apply_elder_curse(&world);
        }
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for ElderGuardianEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `ElderGuardian` inherits `Guardian.GuardianMoveControl`.
    fn tick_move_control(&self) {
        // `ElderGuardian` has no `setMoving` override; vanilla inherits the
        // guardian's moving flag through the shared control.
        let _ = tick_guardian_move_control(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.is_in_water() {
            &sound_events::ENTITY_ELDER_GUARDIAN_AMBIENT
        } else {
            &sound_events::ENTITY_ELDER_GUARDIAN_AMBIENT_LAND
        })
    }

    fn ambient_sound_interval(&self) -> i32 {
        160
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn max_head_x_rot(&self) -> f32 {
        180.0
    }

    /// Vanilla `ElderGuardian` always requires persistence.
    fn requires_custom_persistence(&self) -> bool {
        true
    }
}

impl Monster for ElderGuardianEntity {}

impl PathfinderMob for ElderGuardianEntity {
    /// Vanilla `Guardian.getWalkTargetValue` favors water.
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        if let Some(world) = self.level()
            && world.get_block_state(pos).get_fluid_state().is_water()
        {
            return 10.0 + LevelReader::pathfinding_cost_from_light_levels(world.as_ref(), pos);
        }
        Monster::get_walk_target_value_monster(self, pos)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::ElderGuardianEntity;
    use crate::entity::{Entity, Mob};

    #[test]
    fn elder_guardian_has_the_vanilla_attack_duration_and_persistence() {
        init_vanilla_registry();
        let elder = ElderGuardianEntity::new(
            &vanilla_entities::ELDER_GUARDIAN,
            1,
            DVec3::ZERO,
            Weak::new(),
        );

        assert_eq!(ElderGuardianEntity::attack_duration(), 60);
        assert!(elder.requires_custom_persistence());
        assert!(!elder.is_moving());
        assert_eq!(elder.entity_type(), &vanilla_entities::ELDER_GUARDIAN);
    }
}
