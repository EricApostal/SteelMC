//! Vanilla `Shulker` (`net.minecraft.world.entity.monster.Shulker`).
//!
//! Vanilla is a `BlockAttachedEntity` that stays glued to the block face it spawned on; Steel's
//! `BlockAttachedEntity` trait exists but is reserved for the block-attached slice, so this
//! implementation keeps the same attach-face/peek/teleport state and re-derives the attachment
//! each tick (documented deviation: it does not share the `BlockAttachedEntity` base yet).

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
use steel_registry::vanilla_entity_data::ShulkerEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::ShulkerBulletEntity;
use crate::entity::projectile::spawn_projectile;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity, next_entity_id,
};
use crate::physics::MoveResult;
use crate::world::World;
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Shulker.DEFAULT_ATTACH_FACE`.
const DEFAULT_ATTACH_FACE: Direction = Direction::Down;
/// Vanilla `Shulker.DEFAULT_COLOR` (16 = no dye).
const DEFAULT_COLOR: i8 = 16;
/// Vanilla `Shulker` peek bounds.
const MAX_PEEK_HEIGHT: i8 = 30;
/// Vanilla `Shulker.ShulkerAttackGoal` attack interval and range.
const ATTACK_INTERVAL: i32 = 100;
const ATTACK_RANGE: f64 = 16.0;
/// Vanilla `Shulker.teleportSomewhere` attempts and offsets.
const TELEPORT_ATTEMPTS: i32 = 5;
const TELEPORT_OFFSET: i32 = 8;
/// Vanilla `LevelEvent.SHULKER_TELEPORT`.
const SHULKER_TELEPORT_LEVEL_EVENT: i32 = 2016;

/// Vanilla `Shulker`.
#[entity_behavior(class = "Shulker")]
pub struct ShulkerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<ShulkerEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ShulkerEntity`.
unsafe impl DowncastType for ShulkerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/shulker");
}

impl ShulkerEntity {
    /// Creates a new shulker entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved shulker entity.
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
        let mut entity_data = ShulkerEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.attach_face.set(DEFAULT_ATTACH_FACE);
        entity_data.peek.set(0);
        entity_data.color.set(DEFAULT_COLOR);

        {
            // Vanilla `Shulker.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, LookAtPlayerGoal::new_with_probability(8.0, 0.02));
            goal_selector.add_goal(4, ShulkerAttackGoal::new());
            goal_selector.add_goal(7, ShulkerPeekGoal::new());
            goal_selector.add_goal(8, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players_with_interval(
                    10,
                    true,
                    false,
                    |_, _| true,
                ),
            );
            // Vanilla `ShulkerDefenseAttackGoal` defends against any living attacker in range.
            target_selector.add_goal(
                3,
                NearestAttackableTargetGoal::new_with_interval(10, true, false, |_, _| true),
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

    /// Vanilla `Shulker.getAttachFace`.
    #[must_use]
    pub fn attach_face(&self) -> Direction {
        *self.entity_data.lock().attach_face.get()
    }

    /// Vanilla `Shulker.setAttachFace`.
    pub fn set_attach_face(&self, face: Direction) {
        self.entity_data.lock().attach_face.set(face);
    }

    /// Vanilla `Shulker.getRawPeekAmount`.
    #[must_use]
    pub fn peek_amount(&self) -> i8 {
        *self.entity_data.lock().peek.get()
    }

    /// Vanilla `Shulker.setRawPeekAmount`.
    pub fn set_peek_amount(&self, amount: i8) {
        self.entity_data
            .lock()
            .peek
            .set(amount.clamp(0, MAX_PEEK_HEIGHT));
    }

    /// Vanilla `Shulker.getColor` (dye id, 16 = none).
    #[must_use]
    pub fn color(&self) -> i8 {
        *self.entity_data.lock().color.get()
    }

    /// Vanilla `Shulker.setColor`.
    pub fn set_color(&self, color: i8) {
        self.entity_data.lock().color.set(color);
    }

    /// Vanilla `Shulker.isClosed`.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.peek_amount() == 0
    }

    /// Vanilla `Shulker.shootBullet`: fire a homing shulker bullet at the target.
    fn shoot_bullet(&self, target: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        let direction = self.attach_face().offset();
        let spawn = self.position()
            + DVec3::new(
                f64::from(direction.0) * 0.5,
                f64::from(direction.1) * 0.5,
                f64::from(direction.2) * 0.5,
            );
        let bullet = ShulkerBulletEntity::new(
            &vanilla_entities::SHULKER_BULLET,
            next_entity_id(),
            spawn,
            Arc::downgrade(&world),
        );
        bullet.set_target_entity(Some(target));
        let mut item_stack = ItemStack::empty();
        let _ = spawn_projectile(&world, bullet, &mut item_stack, |_| {});
        self.play_sound(
            &sound_events::ENTITY_SHULKER_SHOOT,
            2.0,
            (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0,
        );
    }

    /// Vanilla `Shulker.teleportSomewhere`.
    pub fn teleport_somewhere(&self) -> bool {
        if !LivingEntity::is_alive(self) {
            return false;
        }
        let Some(world) = self.level() else {
            return false;
        };

        let old_pos = self.block_position();
        for _ in 0..TELEPORT_ATTEMPTS {
            let target = old_pos.offset(
                rand::random_range(-TELEPORT_OFFSET..=TELEPORT_OFFSET),
                rand::random_range(-TELEPORT_OFFSET..=TELEPORT_OFFSET),
                rand::random_range(-TELEPORT_OFFSET..=TELEPORT_OFFSET),
            );
            if target.y() <= world.get_min_y() {
                continue;
            }
            if !world.get_block_state(target).is_air() {
                continue;
            }

            let Some(attachment) = self.find_attachable_surface(&target) else {
                continue;
            };

            self.set_attach_face(attachment);
            self.play_sound(&sound_events::ENTITY_SHULKER_TELEPORT, 1.0, 1.0);
            let destination = DVec3::new(
                f64::from(target.x()) + 0.5,
                f64::from(target.y()),
                f64::from(target.z()) + 0.5,
            );
            if self.teleport_to(destination).is_ok() {
                self.game_event(&steel_registry::vanilla_game_events::TELEPORT);
                world.level_event(SHULKER_TELEPORT_LEVEL_EVENT, old_pos, 0, None);
                self.set_peek_amount(0);
                self.set_target(None);
                return true;
            }
        }
        false
    }

    /// Vanilla `Shulker.findAttachableSurface`: the first face whose neighbor is solid.
    fn find_attachable_surface(&self, pos: &BlockPos) -> Option<Direction> {
        let world = self.level()?;
        for face in [
            Direction::Down,
            Direction::Up,
            Direction::North,
            Direction::South,
            Direction::West,
            Direction::East,
        ] {
            let offset = face.offset();
            let neighbor = pos.offset(offset.0, offset.1, offset.2);
            let state = world.get_block_state(neighbor);
            if !state.is_air() {
                return Some(face);
            }
        }
        None
    }
}

/// Vanilla `Shulker.ShulkerAttackGoal`.
struct ShulkerAttackGoal {
    attack_time: i32,
}

impl ShulkerAttackGoal {
    const fn new() -> Self {
        Self { attack_time: 0 }
    }

    fn shulker(mob: &dyn PathfinderMob) -> Option<&ShulkerEntity> {
        mob.as_entity_event_source().downcast_ref::<ShulkerEntity>()
    }
}

impl Goal for ShulkerAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target().is_some_and(|target| {
            target
                .as_living_entity()
                .is_some_and(LivingEntity::is_alive)
        })
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.attack_time = 0;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(shulker) = Self::shulker(mob) else {
            return;
        };
        let Some(target) = mob.target() else {
            return;
        };

        if mob.position().distance_squared(target.position()) >= ATTACK_RANGE * ATTACK_RANGE
            || !mob.has_line_of_sight_cached(target.as_ref())
        {
            return;
        }

        self.attack_time += 1;
        if self.attack_time < ATTACK_INTERVAL {
            return;
        }

        self.attack_time = 0;
        shulker.shoot_bullet(&target);
    }
}

/// Vanilla `Shulker.ShulkerPeekGoal`.
struct ShulkerPeekGoal {
    peek_time: i32,
}

impl ShulkerPeekGoal {
    const fn new() -> Self {
        Self { peek_time: 0 }
    }

    fn shulker(mob: &dyn PathfinderMob) -> Option<&ShulkerEntity> {
        mob.as_entity_event_source().downcast_ref::<ShulkerEntity>()
    }
}

impl Goal for ShulkerPeekGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(shulker) = Self::shulker(mob) else {
            return false;
        };
        if shulker.peek_amount() != 0 || rand::random_range(0..100) != 0 {
            return false;
        }
        self.peek_time = 0;
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(shulker) = Self::shulker(mob) else {
            return false;
        };
        self.peek_time <= 60 && shulker.peek_amount() != 0
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(shulker) = Self::shulker(mob) {
            shulker.set_peek_amount(0);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(shulker) = Self::shulker(mob) else {
            return;
        };
        self.peek_time += 1;
        let peek = if self.peek_time > 60 {
            i32::from(MAX_PEEK_HEIGHT) - (self.peek_time - 60) * 3
        } else {
            self.peek_time * 3
        };
        shulker.set_peek_amount(peek.clamp(0, i32::from(MAX_PEEK_HEIGHT)) as i8);
    }
}

impl Entity for ShulkerEntity {
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
        // Vanilla persists `Direction.LEGACY_ID_CODEC`; Steel uses the 3D data value.
        nbt.insert("AttachFace", self.attach_face().get_3d_data_value() as i8);
        nbt.insert("Peek", self.peek_amount() as i8);
        nbt.insert("Color", self.color() as i8);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(face) = nbt.byte("AttachFace") {
            self.set_attach_face(Direction::from_3d_data_value(i32::from(face)));
        }
        self.set_peek_amount(nbt.byte("Peek").unwrap_or(0));
        self.set_color(nbt.byte("Color").unwrap_or(DEFAULT_COLOR));
    }
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = LivingEntity::hurt_server(self, world, source, amount);
        if was_hurt && self.is_closed() && rand::random::<f32>() < 0.2 {
            self.teleport_somewhere();
        }
        was_hurt
    }
}

impl LivingEntity for ShulkerEntity {
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

    /// Vanilla `Shulker.getHurtSound` depends on the shell being closed.
    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(if self.is_closed() {
            &sound_events::ENTITY_SHULKER_HURT_CLOSED
        } else {
            &sound_events::ENTITY_SHULKER_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SHULKER_DEATH)
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

impl Mob for ShulkerEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Shulker.getAmbientSound`.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SHULKER_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Shulker.requiresCustomPersistence` keeps shulkers forever.
    fn requires_custom_persistence(&self) -> bool {
        true
    }
}

impl Monster for ShulkerEntity {}

impl PathfinderMob for ShulkerEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use steel_utils::Direction;

    use super::{DEFAULT_COLOR, ShulkerEntity};
    use crate::entity::Entity;

    #[test]
    fn shulker_attachment_and_peek_round_trip() {
        init_vanilla_registry();
        let shulker = ShulkerEntity::new(&vanilla_entities::SHULKER, 1, DVec3::ZERO, Weak::new());

        assert_eq!(shulker.attach_face(), Direction::Down);
        assert_eq!(shulker.color(), DEFAULT_COLOR);
        assert!(shulker.is_closed());

        shulker.set_attach_face(Direction::Up);
        shulker.set_peek_amount(20);
        shulker.set_color(4);
        assert!(!shulker.is_closed());

        let mut nbt = simdnbt::owned::NbtCompound::new();
        shulker.save_additional(&mut nbt);
        assert_eq!(nbt.byte("Peek"), Some(20));
        assert_eq!(nbt.byte("Color"), Some(4));
        assert_eq!(shulker.entity_type(), &vanilla_entities::SHULKER);
    }
}
