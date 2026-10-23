//! Vanilla `Creaking` (`net.minecraft.world.entity.monster.creaking.Creaking`).
//!
//! Vanilla is brain-driven (`CreakingAi`, owned by the brain slice) and bound to a
//! `CreakingHeartBlockEntity`. The heart link is real here: `is_protector` is checked every tick
//! and a creaking that loses its heart dies, exactly like vanilla `Creaking.tick`.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::CreakingEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_blocks, vanilla_particle_types};
use steel_utils::Downcast as _;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::block_entity::entities::CreakingHeartBlockEntity;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type;
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, RemovalReason, SharedEntity,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::World;

use super::brain;

const DEFAULT_STEP_HEIGHT: f32 = 1.0625;
/// Vanilla `Creaking` has no XP reward.
const XP_REWARD: i32 = 0;
/// Vanilla `Creaking.ATTACK_ANIMATION_DURATION`.
const ATTACK_ANIMATION_DURATION: i32 = 15;
/// Vanilla `Creaking.INVULNERABILITY_ANIMATION_DURATION`.
const INVULNERABILITY_ANIMATION_DURATION: i32 = 8;
/// Vanilla `Creaking.TWITCH_DEATH_DURATION`.
const TWITCH_DEATH_DURATION: i32 = 45;
/// Vanilla `Creaking.ACTIVATION_RANGE_SQ`.
const ACTIVATION_RANGE_SQR: f64 = 144.0;
/// Vanilla `Creaking` gaze tolerance for activation.
const GAZE_TOLERANCE: f64 = 0.5;

/// Vanilla `Creaking`.
#[entity_behavior(class = "Creaking")]
pub struct CreakingEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<CreakingEntityData>,
    brain: SyncMutex<Brain>,
    /// Vanilla `Creaking.attackAnimationRemainingTicks`.
    attack_animation_remaining_ticks: SyncMutex<i32>,
    /// Vanilla `Creaking.invulnerabilityAnimationRemainingTicks`.
    invulnerability_animation_remaining_ticks: SyncMutex<i32>,
    /// Vanilla `Creaking.deathTime` for the tear-down animation.
    tear_down_ticks: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CreakingEntity`.
unsafe impl DowncastType for CreakingEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/creaking");
}

impl CreakingEntity {
    /// Creates a new creaking entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved creaking entity.
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
        // Vanilla `Creaking` sets `navigation.setCanFloat(true)`.
        mob_base.navigation().lock().set_can_float(true);
        let mut entity_data = CreakingEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.can_move.set(true);
        entity_data.is_active.set(false);
        entity_data.is_tearing_down.set(false);
        entity_data.home_pos.set(None);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(brain::make_brain()),
            attack_animation_remaining_ticks: SyncMutex::new(0),
            invulnerability_animation_remaining_ticks: SyncMutex::new(0),
            tear_down_ticks: SyncMutex::new(0),
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

    /// Vanilla `Creaking.canMove`.
    #[must_use]
    pub fn can_move(&self) -> bool {
        *self.entity_data.lock().can_move.get()
    }

    /// Vanilla `Creaking.isActive`.
    #[must_use]
    pub fn is_active(&self) -> bool {
        *self.entity_data.lock().is_active.get()
    }

    /// Vanilla `Creaking.setIsActive`.
    pub fn set_active(&self, active: bool) {
        self.entity_data.lock().is_active.set(active);
    }

    /// Vanilla `Creaking.isTearingDown`.
    #[must_use]
    pub fn is_tearing_down(&self) -> bool {
        *self.entity_data.lock().is_tearing_down.get()
    }

    /// Vanilla `Creaking.setTearingDown`.
    pub fn set_tearing_down(&self) {
        self.entity_data.lock().is_tearing_down.set(true);
    }

    /// Vanilla `Creaking.getHomePos`.
    #[must_use]
    pub fn home_pos(&self) -> Option<BlockPos> {
        *self.entity_data.lock().home_pos.get()
    }

    /// Vanilla `Creaking.setHomePos`.
    pub fn set_home_pos(&self, pos: BlockPos) {
        self.entity_data.lock().home_pos.set(Some(pos));
    }

    /// Vanilla `Creaking.isHeartBound`.
    #[must_use]
    pub fn is_heart_bound(&self) -> bool {
        self.home_pos().is_some()
    }

    /// Vanilla `Creaking.setTransient`: heart binding plus the creaking's pathfinding maluses.
    pub fn set_transient(&self, pos: BlockPos) {
        self.set_home_pos(pos);
        let mut malus = self.mob_base.pathfinding_malus().lock();
        malus.set(PathType::Damaging, 8.0);
        malus.set(PathType::PowderSnow, 8.0);
        malus.set(PathType::Lava, 8.0);
        malus.set(PathType::Fire, 0.0);
    }

    /// Vanilla `Creaking.activate`.
    pub fn activate(&self, player: &SharedEntity) {
        {
            let mut brain = self.brain.lock();
            brain.set_memory(&module_type::ATTACK_TARGET, Some(player.clone()));
        }
        self.game_event(&steel_registry::vanilla_game_events::ENTITY_ACTION);
        self.play_sound(&sound_events::ENTITY_CREAKING_ACTIVATE, 1.0, 1.0);
        self.set_active(true);
    }

    /// Vanilla `Creaking.deactivate`.
    pub fn deactivate(&self) {
        {
            let mut brain = self.brain.lock();
            brain.erase_memory(&module_type::ATTACK_TARGET);
        }
        self.game_event(&steel_registry::vanilla_game_events::ENTITY_ACTION);
        self.play_sound(&sound_events::ENTITY_CREAKING_DEACTIVATE, 1.0, 1.0);
        self.set_active(false);
    }

    /// Vanilla `Creaking.checkCanMove`.
    fn check_can_move(&self) -> bool {
        let players = {
            let brain = self.brain.lock();
            brain
                .get_memory(&module_type::NEAREST_PLAYERS)
                .unwrap_or_default()
        };
        let active = self.is_active();

        if players.is_empty() {
            if active {
                self.deactivate();
            }
            return true;
        }

        let mut has_potential_target = false;
        for player in &players {
            let Some(player_living) = player.as_living_entity() else {
                continue;
            };
            if !Mob::can_attack(self, player_living) || self.is_allied_to(player_living) {
                continue;
            }
            has_potential_target = true;

            let mut disguised = false;
            player_living.with_equipment_slot(EquipmentSlot::Head, &mut |stack| {
                disguised = stack.is(&steel_registry::vanilla_items::CARVED_PUMPKIN);
            });
            if (!active || !disguised) && self.is_looking_at_me(player, GAZE_TOLERANCE) {
                if active {
                    return false;
                }
                if self.position().distance_squared(player.position()) < ACTIVATION_RANGE_SQR {
                    self.activate(player);
                    return false;
                }
            }
        }

        if !has_potential_target && active {
            self.deactivate();
        }
        true
    }

    /// Vanilla `LivingEntity.isLookingAtMe` for the creaking's activation gaze.
    fn is_looking_at_me(&self, player: &SharedEntity, tolerance: f64) -> bool {
        let look = player.calculate_view_vector(player.rotation().1, player.rotation().0);
        let to_creaking = DVec3::new(
            self.position().x - player.position().x,
            self.get_eye_y() - player.get_eye_y(),
            self.position().z - player.position().z,
        );
        if to_creaking.length_squared() < 1.0e-4 {
            return true;
        }
        let Some(to_creaking) = to_creaking.try_normalize() else {
            return false;
        };
        look.dot(to_creaking) >= 1.0 - tolerance && self.has_line_of_sight_cached(player.as_ref())
    }

    /// Vanilla `Creaking.tick` heart-protection check.
    fn check_heart_protection(&self) {
        let Some(home_pos) = self.home_pos() else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla `CreakingHeartBlockEntity.isProtector(this)`: the heart's linked UUID matches.
        let protected = world
            .get_block_entity(home_pos)
            .is_some_and(|block_entity| {
                block_entity
                    .downcast_ref::<CreakingHeartBlockEntity>()
                    .is_some_and(|heart| heart.creaking() == Some(self.uuid()))
            });

        if !protected {
            self.set_health(0.0);
        }
    }

    /// Vanilla `Creaking.tearDown`: pale-oak and heart crumble particles, death sound, removal.
    fn tear_down(&self) {
        if let Some(world) = self.level() {
            let aabb = self.bounding_box();
            let center = aabb.center();
            let x_spread = aabb.width() * 0.3;
            let y_spread = aabb.height() * 0.3;
            let z_spread = aabb.depth() * 0.3;

            world.send_particles(
                ParticleData::new(
                    &vanilla_particle_types::BLOCK_CRUMBLE,
                    steel_registry::particle_type::BlockParticleOption::new(
                        vanilla_blocks::PALE_OAK_WOOD.default_state(),
                    ),
                ),
                center,
                100,
                DVec3::new(x_spread, y_spread, z_spread),
                0.0,
            );
            world.send_particles(
                ParticleData::new(
                    &vanilla_particle_types::BLOCK_CRUMBLE,
                    steel_registry::particle_type::BlockParticleOption::new(
                        vanilla_blocks::CREAKING_HEART.default_state(),
                    ),
                ),
                center,
                10,
                DVec3::new(x_spread, y_spread, z_spread),
                0.0,
            );
        }

        self.play_sound(self.death_sound_ref(), 1.0, 1.0);
        self.set_removed(RemovalReason::Discarded);
    }

    fn death_sound_ref(&self) -> SoundEventRef {
        &sound_events::ENTITY_CREAKING_DEATH
    }
}

impl Entity for CreakingEntity {
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

        self.check_heart_protection();

        // Vanilla `Creaking.aiStep`: freeze/unfreeze transitions.
        let can_move = self.can_move();
        let now_can_move = self.check_can_move();
        if now_can_move != can_move {
            self.game_event(&steel_registry::vanilla_game_events::ENTITY_ACTION);
            if now_can_move {
                self.play_sound(&sound_events::ENTITY_CREAKING_UNFREEZE, 1.0, 1.0);
            } else {
                self.mob_base().navigation().lock().stop();
                self.play_sound(&sound_events::ENTITY_CREAKING_FREEZE, 1.0, 1.0);
            }
        }
        self.entity_data.lock().can_move.set(now_can_move);

        {
            let mut invulnerability = self.invulnerability_animation_remaining_ticks.lock();
            if *invulnerability > 0 {
                *invulnerability -= 1;
            }
        }
        {
            let mut attack = self.attack_animation_remaining_ticks.lock();
            if *attack > 0 {
                *attack -= 1;
            }
        }

        self.tick_living_entity(&entity);

        // Vanilla `Creaking.tickDeath` tear-down.
        if self.is_heart_bound()
            && self.is_tearing_down()
            && LivingEntity::is_dead_or_dying(self.as_ref())
        {
            let mut ticks = self.tear_down_ticks.lock();
            *ticks += 1;
            if *ticks > TWITCH_DEATH_DURATION && !self.is_removed() {
                drop(ticks);
                self.tear_down();
            }
        }
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_CREAKING_STEP, 0.15, 1.0);
    }

    /// Vanilla `Creaking.isPushable` requires `canMove`.
    fn is_pushable(&self) -> bool {
        self.can_move() && Entity::is_pushable(self)
    }

    /// Vanilla `Creaking.fireImmune` is true while heart-bound.
    fn fire_immune(&self) -> bool {
        self.is_heart_bound() || Entity::fire_immune(self)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        if let Some(home) = self.home_pos() {
            nbt.insert("home_pos_x", home.x());
            nbt.insert("home_pos_y", home.y());
            nbt.insert("home_pos_z", home.z());
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let (Some(x), Some(y), Some(z)) = (
            nbt.int("home_pos_x"),
            nbt.int("home_pos_y"),
            nbt.int("home_pos_z"),
        ) {
            self.set_transient(BlockPos::new(x, y, z));
        }
    }
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let Some(home_pos) = self.home_pos() else {
            return LivingEntity::hurt_server(self, world, source, amount);
        };
        if source.bypasses_invulnerability() {
            return LivingEntity::hurt_server(self, world, source, amount);
        }
        if LivingEntity::is_invulnerable_to(self, world, source)
            || *self.invulnerability_animation_remaining_ticks.lock() > 0
            || LivingEntity::is_dead_or_dying(self)
        {
            return false;
        }

        let responsible_player = source.causing_entity().cloned();
        let direct_is_living = source
            .direct_entity()
            .is_some_and(|direct| direct.as_living_entity().is_some());
        let direct_is_projectile = source.direct_entity().is_some_and(|direct| {
            direct.as_projectile().is_some()
                || direct.entity_type() == &steel_registry::vanilla_entities::ARROW
        });
        if !direct_is_living && !direct_is_projectile && responsible_player.is_none() {
            return false;
        }

        *self.invulnerability_animation_remaining_ticks.lock() = INVULNERABILITY_ANIMATION_DURATION;
        self.game_event(&steel_registry::vanilla_game_events::ENTITY_ACTION);

        if let Some(heart_entity) = world.get_block_entity(home_pos)
            && let Some(heart) = heart_entity.downcast_ref::<CreakingHeartBlockEntity>()
            && heart.creaking() == Some(self.uuid())
        {
            // Vanilla `CreakingHeartBlockEntity.creakingHurt`.
            if responsible_player.is_some() {
                world.play_block_sound(
                    &sound_events::BLOCK_CREAKING_HEART_HURT,
                    home_pos,
                    1.0,
                    1.0,
                    None,
                );
            }
            if let Some(sound) = self.hurt_sound(source) {
                self.play_sound(sound, self.sound_volume(), 1.0);
            }
        }

        true
    }
}

impl LivingEntity for CreakingEntity {
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
        Some(&sound_events::ENTITY_CREAKING_DEATH)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CREAKING_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        false
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

impl Mob for CreakingEntity {
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
        Some(&sound_events::ENTITY_CREAKING_AMBIENT)
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

    /// Vanilla `Creaking.customServerAiStep`: tick the brain, then update its activity.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
        brain::update_activity(&mut brain, self.can_move());
    }
    /// Vanilla `Creaking.doHurtTarget` plays the attack animation on the shared melee path.
    fn after_hurt_target(&self, _world: &World, target: &SharedEntity, was_hurt: bool) {
        if !was_hurt || target.as_living_entity().is_none() {
            return;
        }
        *self.attack_animation_remaining_ticks.lock() = ATTACK_ANIMATION_DURATION;
        self.broadcast_entity_event(steel_utils::entity_events::EntityStatus::StartAttacking);
    }
}

impl Monster for CreakingEntity {}

impl PathfinderMob for CreakingEntity {
    /// Vanilla `Creaking.getWalkTargetValue` returns 0 everywhere.
    fn get_walk_target_value(&self, _pos: BlockPos) -> f32 {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::CreakingEntity;
    use crate::entity::Entity;

    #[test]
    fn creaking_transient_binding_sets_home_and_flags() {
        init_vanilla_registry();
        let creaking =
            CreakingEntity::new(&vanilla_entities::CREAKING, 1, DVec3::ZERO, Weak::new());

        assert!(!creaking.is_heart_bound());
        assert!(creaking.can_move());
        assert!(!creaking.is_active());

        creaking.set_transient(steel_utils::BlockPos::new(1, 64, 2));
        assert!(creaking.is_heart_bound());
        assert_eq!(
            creaking.home_pos(),
            Some(steel_utils::BlockPos::new(1, 64, 2))
        );

        let mut nbt = simdnbt::owned::NbtCompound::new();
        creaking.save_additional(&mut nbt);
        assert_eq!(nbt.int("home_pos_y"), Some(64));
        assert_eq!(creaking.entity_type(), &vanilla_entities::CREAKING);
    }
}
