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
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_blocks, vanilla_damage_type_tags, vanilla_entities,
    vanilla_game_events,
};
use steel_utils::Identifier;
use steel_utils::locks::SyncMutex;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey, WorldAabb};

use crate::entity::ai::goal::{
    Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::ShulkerBulletEntity;
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::projectile::spawn_projectile;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource, EntityMoveError, EntityMovementEmission,
    EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, Monster,
    PathfinderMob, SharedEntity, SpawnGroupData, next_entity_id, physics_state_for_move,
    record_movement_for_block_effects, should_apply_resolved_movement,
};
use crate::physics::{MoveResult, MoverType, WorldCollisionProvider, move_entity as resolve_entity_movement};
use crate::world::{LevelReader as _, World};
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Shulker.DEFAULT_ATTACH_FACE`.
const DEFAULT_ATTACH_FACE: Direction = Direction::Down;
/// Vanilla `Shulker.DEFAULT_COLOR` (16 = no dye).
const DEFAULT_COLOR: i8 = 16;
/// Vanilla `Shulker.COVERED_ARMOR_MODIFIER_ID` (`"covered"`).
const COVERED_ARMOR_MODIFIER_ID: Identifier = Identifier::vanilla_static("covered");
/// Vanilla `Shulker.COVERED_ARMOR_MODIFIER` (+20 armor while closed, `ADD_VALUE`).
const COVERED_ARMOR_MODIFIER: AttributeModifier = AttributeModifier {
    id: COVERED_ARMOR_MODIFIER_ID,
    amount: 20.0,
    operation: AttributeModifierOperation::AddValue,
};
/// Vanilla `Shulker.MAX_SCALE`.
const MAX_SCALE: f32 = 1.0;
/// Vanilla `Shulker.PEEK_PER_TICK`.
const PEEK_PER_TICK: f32 = 0.05;
/// Vanilla `Shulker.DEFAULT_PEEK`.
const DEFAULT_PEEK: i8 = 0;
/// Vanilla `Shulker.ShulkerAttackGoal` cooldown range and distance.
const ATTACK_COOLDOWN_BASE: i32 = 20;
const ATTACK_COOLDOWN_RANGE: i32 = 10;
const ATTACK_COOLDOWN_STEP: i32 = 20;
const ATTACK_RANGE_SQR: f64 = 400.0;
/// Vanilla `Shulker.teleportSomewhere` attempts and offsets.
const TELEPORT_ATTEMPTS: i32 = 5;
const TELEPORT_OFFSET: i32 = 8;
/// Vanilla `LevelEvent.SHULKER_TELEPORT`.
const SHULKER_TELEPORT_LEVEL_EVENT: i32 = 2016;

/// Vanilla `new AABB(BlockPos)`: the full one-block cube at `pos`.
fn block_aabb(pos: BlockPos) -> WorldAabb {
    let min_x = f64::from(pos.x());
    let min_y = f64::from(pos.y());
    let min_z = f64::from(pos.z());
    WorldAabb::new(min_x, min_y, min_z, min_x + 1.0, min_y + 1.0, min_z + 1.0)
}

/// Vanilla `BlockUtil.packDifferenceInPosition`, used as the payload of the
/// `SHULKER_TELEPORT` level event.
const fn pack_difference_in_position(
    from: BlockPos,
    to: BlockPos,
    x_radius: i32,
    y_radius: i32,
    z_radius: i32,
) -> i32 {
    let x_diff = to.x() - from.x();
    let y_diff = to.y() - from.y();
    let z_diff = to.z() - from.z();
    ((x_diff + x_radius) & 0xFF) << 16 | ((y_diff + y_radius) & 0xFF) << 8 | ((z_diff + z_radius) & 0xFF)
}

/// Vanilla `Shulker.currentPeekAmount`/`currentPeekAmountO`: the interpolated lid
/// amount that drives the shell bounding box and the opening push.
#[derive(Debug, Clone, Copy)]
struct PeekAmount {
    current: f32,
    old: f32,
}

/// Vanilla `Shulker`.
#[entity_behavior(class = "Shulker")]
pub struct ShulkerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<ShulkerEntityData>,
    /// Vanilla `currentPeekAmount`/`currentPeekAmountO` (not synced data).
    peek_amount: SyncMutex<PeekAmount>,
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
        entity_data.peek.set(DEFAULT_PEEK);
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
            // Vanilla `Shulker.ShulkerNearestAttackGoal`: players, interval 10, must-see.
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players_with_interval(
                    10,
                    true,
                    false,
                    |_, _| true,
                ),
            );
            // Vanilla `ShulkerDefenseAttackGoal`: living entities that are `Enemy`
            // (`instanceof Enemy`). Vanilla also gates this goal on the mob being on a
            // scoreboard team; Steel has no team storage, so the goal is only selector-
            // accurate. Its attach-axis search box (vanilla `getTargetSearchArea`) is not
            // expressible with Steel's follow-range target search.
            target_selector.add_goal(
                3,
                NearestAttackableTargetGoal::new_with_interval(10, true, false, |target, _| {
                    target.as_enemy().is_some()
                }),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            peek_amount: SyncMutex::new(PeekAmount {
                current: 0.0,
                old: 0.0,
            }),
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

    /// Vanilla `Shulker.getRawPeekAmount` (`DATA_PEEK_ID`, 0..=100 in practice).
    #[must_use]
    pub fn peek_amount(&self) -> i8 {
        *self.entity_data.lock().peek.get()
    }

    /// Vanilla `Shulker.setRawPeekAmount`: swaps the covered-armor modifier,
    /// plays the open/close sound and fires the container game event, then
    /// writes `DATA_PEEK_ID`. The raw amount is not clamped (the attack goal
    /// uses 100); vanilla only ever clamps the *interpolated* amount.
    pub fn set_peek_amount(&self, amount: i8) {
        self.attributes()
            .lock()
            .remove_modifier(vanilla_attributes::ARMOR, &COVERED_ARMOR_MODIFIER_ID);
        if amount == 0 {
            self.attributes().lock().set_modifier(
                vanilla_attributes::ARMOR,
                COVERED_ARMOR_MODIFIER,
                true,
            );
            self.play_sound(&sound_events::ENTITY_SHULKER_CLOSE, 1.0, 1.0);
            self.game_event(&vanilla_game_events::CONTAINER_CLOSE);
        } else {
            self.play_sound(&sound_events::ENTITY_SHULKER_OPEN, 1.0, 1.0);
            self.game_event(&vanilla_game_events::CONTAINER_OPEN);
        }
        self.entity_data.lock().peek.set(amount);
    }

    /// Vanilla `Shulker.getClientPeekAmount(float partialTick)`.
    #[must_use]
    pub fn client_peek_amount(&self, partial_tick: f32) -> f32 {
        let peek = *self.peek_amount.lock();
        peek.old + (peek.current - peek.old) * partial_tick
    }

    /// Vanilla `Shulker.getPhysicalPeek`: the physical lid offset for an
    /// interpolated peek amount.
    fn physical_peek(peek: f32) -> f32 {
        0.5 - 0.5 * ((0.5 + peek) * std::f32::consts::PI).sin()
    }

    /// Vanilla `Shulker.updatePeekAmount`, returning whether it changed.
    fn update_peek_amount(&self) -> bool {
        let target = f32::from(self.peek_amount()) * 0.01;
        let mut peek = self.peek_amount.lock();
        peek.old = peek.current;
        if peek.current == target {
            return false;
        }
        peek.current = if peek.current > target {
            (peek.current - PEEK_PER_TICK).clamp(target, 1.0)
        } else {
            (peek.current + PEEK_PER_TICK).clamp(0.0, target)
        };
        true
    }

    /// Vanilla `Shulker.onPeekAmountChange`: refresh the shell box and push
    /// non-shulker entities out of the opening shell.
    fn on_peek_amount_change(&self) {
        // Vanilla `reapplyPosition()` runs the overridden `setPos` for the
        // current position; Steel's position setter is `try_set_position`.
        let position = self.position();
        let _ = self.try_set_position(position);
        self.refresh_dimensions();

        let progress = Self::physical_peek(self.peek_amount.lock().current);
        let old_progress = Self::physical_peek(self.peek_amount.lock().old);
        let direction = self.attach_face().opposite();
        let scale = f64::from(LivingEntity::get_scale(self));
        let delta = f64::from(progress - old_progress) * scale;
        if delta <= 0.0 {
            return;
        }

        let Some(world) = self.level() else {
            return;
        };

        let area = self.progress_delta_aabb(direction, old_progress, progress, position);
        let (step_x, step_y, step_z) = direction.offset();
        let push = DVec3::new(
            delta * f64::from(step_x),
            delta * f64::from(step_y),
            delta * f64::from(step_z),
        );

        for entity in world.get_entities_in_aabb_matching(&area, |entity| {
            !entity.is_spectator()
                && entity.entity_type() != &vanilla_entities::SHULKER
                && !entity.no_physics()
        }) {
            let _ = entity.move_entity(MoverType::Shulker, push);
        }
    }

    /// Vanilla `Shulker.getProgressAabb`.
    fn progress_aabb(&self, direction: Direction, progress: f32, pos: DVec3) -> WorldAabb {
        self.progress_delta_aabb(direction, progress, -1.0, pos)
    }

    /// Vanilla `Shulker.getProgressDeltaAabb`.
    fn progress_delta_aabb(
        &self,
        direction: Direction,
        progress: f32,
        delta: f32,
        pos: DVec3,
    ) -> WorldAabb {
        let scale = f64::from(LivingEntity::get_scale(self));
        let half = scale * 0.5;
        let base = WorldAabb::new(-half, 0.0, -half, half, scale, half);
        let max = f64::from(progress.max(delta));
        let min = f64::from(progress.min(delta));
        let (step_x, step_y, step_z) = direction.offset();
        let step = DVec3::new(f64::from(step_x), f64::from(step_y), f64::from(step_z));
        base.expand_towards(step * (max * scale))
            .contract(-step * ((1.0 + min) * scale))
            .translate(pos)
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

    /// Vanilla `Shulker.ShulkerAttackGoal.tick` bullet spawn: a `ShulkerBullet`
    /// owned by this shulker, targeting `target`, whose first move direction
    /// avoids the shulker's attach axis.
    fn shoot_bullet(&self, shulker: &SharedEntity, target: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };
        let bullet = ShulkerBulletEntity::with_target(
            &vanilla_entities::SHULKER_BULLET,
            next_entity_id(),
            shulker,
            target,
            Some(self.attach_face().get_axis()),
            Arc::downgrade(&world),
        );
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
        if self.is_no_ai() || !LivingEntity::is_alive(self) {
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
            if !world.is_block_within_world_border(target) {
                continue;
            }
            if !world.no_collision(block_aabb(target).deflate(1.0e-6)) {
                continue;
            }

            let Some(attachment) = self.find_attachable_surface(&target) else {
                continue;
            };

            let neighbor = target.offset(
                attachment.offset().0,
                attachment.offset().1,
                attachment.offset().2,
            );
            if world
                .get_block_state(neighbor)
                .get_block()
                .has_tag(&BlockTag::SHULKER_DOES_NOT_TELEPORT_TO)
            {
                continue;
            }

            self.stop_riding();
            self.set_attach_face(attachment);
            self.play_sound(&sound_events::ENTITY_SHULKER_TELEPORT, 1.0, 1.0);
            let destination = DVec3::new(
                f64::from(target.x()) + 0.5,
                f64::from(target.y()),
                f64::from(target.z()) + 0.5,
            );
            if let Err(error) = self.teleport_to(destination) {
                log::debug!("failed to teleport shulker {}: {error}", self.id());
            }
            self.game_event(&vanilla_game_events::TELEPORT);
            let packed = pack_difference_in_position(
                old_pos,
                self.block_position(),
                TELEPORT_OFFSET,
                TELEPORT_OFFSET,
                TELEPORT_OFFSET,
            );
            world.level_event(SHULKER_TELEPORT_LEVEL_EVENT, old_pos, packed, None);
            // Vanilla writes `DATA_PEEK_ID` directly here, bypassing the
            // `setRawPeekAmount` sound/armor side effects.
            self.entity_data.lock().peek.set(0);
            self.set_target(None);
            return true;
        }
        false
    }

    /// Vanilla `Shulker.findNewAttachment`.
    fn find_new_attachment(&self) {
        let pos = self.block_position();
        match self.find_attachable_surface(&pos) {
            Some(face) => self.set_attach_face(face),
            None => {
                self.teleport_somewhere();
            }
        }
    }

    /// Vanilla `Shulker.findAttachableSurface`: the first face the shulker can
    /// stay attached through.
    fn find_attachable_surface(&self, pos: &BlockPos) -> Option<Direction> {
        for face in [
            Direction::Down,
            Direction::Up,
            Direction::North,
            Direction::South,
            Direction::West,
            Direction::East,
        ] {
            if self.can_stay_at(*pos, face) {
                return Some(face);
            }
        }
        None
    }

    /// Vanilla `Shulker.canStayAt`.
    fn can_stay_at(&self, pos: BlockPos, face: Direction) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        if self.is_position_blocked(&world, pos) {
            return false;
        }

        let opposite = face.opposite();
        let (step_x, step_y, step_z) = face.offset();
        let neighbor = pos.offset(step_x, step_y, step_z);
        let state = world.get_block_state(neighbor);
        if !world.is_face_sturdy(state, neighbor, opposite) {
            return false;
        }

        let center = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let area = self.progress_aabb(opposite, 1.0, center).deflate(1.0e-6);
        world.no_collision(area)
    }

    /// Vanilla `Shulker.isPositionBlocked`.
    fn is_position_blocked(&self, world: &Arc<World>, pos: BlockPos) -> bool {
        let state = world.get_block_state(pos);
        if state.is_air() {
            return false;
        }

        let moving_piston_here = state.get_block() == &vanilla_blocks::MOVING_PISTON
            && pos == self.block_position();
        !moving_piston_here
    }

    /// Vanilla `Shulker.hitByShulkerBullet`: an open shell hit by a shulker
    /// bullet teleports away and can leave a duplicate behind when fewer than
    /// six shells share the area.
    fn hit_by_shulker_bullet(&self) {
        let position = self.position();
        let bounds = self.bounding_box();

        if self.is_closed() || !self.teleport_somewhere() {
            return;
        }

        let Some(world) = self.level() else {
            return;
        };

        let nearby = world
            .get_entities_in_aabb_matching(&bounds.inflate(8.0), |entity| {
                entity.entity_type() == &vanilla_entities::SHULKER && entity.is_alive()
            })
            .len();
        if rand::random::<f32>() < (nearby as f32 - 1.0) / 5.0 {
            return;
        }

        let duplicate = Self::new(
            &vanilla_entities::SHULKER,
            next_entity_id(),
            position,
            Arc::downgrade(&world),
        );
        duplicate.set_color(self.color());
        duplicate.set_rotation(self.rotation());
        duplicate.set_old_position_to_current();
        duplicate.base().set_old_rotation_to_current();
        if let Err(error) = world.try_add_entity(Arc::new(duplicate)) {
            log::debug!("failed to add shulker duplicate: {error}");
        }
    }
}

/// Vanilla `Goal.reducedTickDelay` (`(ticks + 1) / 2`). The goal module's
/// helper is `pub(super)`, so entity implementations mirror it locally.
const fn reduced_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
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
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let has_target = mob.target().is_some_and(|target| {
            target
                .as_living_entity()
                .is_some_and(LivingEntity::is_alive)
        });
        has_target
            && mob
                .level()
                .is_some_and(|world| world.difficulty() != Difficulty::Peaceful)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.attack_time = ATTACK_COOLDOWN_BASE;
        if let Some(shulker) = Self::shulker(mob) {
            shulker.set_peek_amount(100);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(shulker) = Self::shulker(mob) {
            shulker.set_peek_amount(0);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(shulker) = Self::shulker(mob) else {
            return;
        };

        if mob
            .level()
            .is_some_and(|world| world.difficulty() == Difficulty::Peaceful)
        {
            return;
        }

        self.attack_time -= 1;
        let Some(target) = mob.target() else {
            return;
        };

        let target_position = target.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            DVec3::new(target_position.x, target.get_eye_y(), target_position.z),
            180.0,
            180.0,
        );

        let distance_sqr = mob.position().distance_squared(target_position);
        if distance_sqr < ATTACK_RANGE_SQR && self.attack_time <= 0 {
            self.attack_time = ATTACK_COOLDOWN_BASE
                + rand::random_range(0..ATTACK_COOLDOWN_RANGE) * ATTACK_COOLDOWN_STEP / 2;
            shulker.shoot_bullet(entity, &target);
        } else if distance_sqr >= ATTACK_RANGE_SQR {
            mob.set_target(None);
        }
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
        mob.target().is_none()
            && rand::random_range(0..reduced_tick_delay(40)) == 0
            && shulker.can_stay_at(mob.as_entity_event_source().block_position(), shulker.attach_face())
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::shulker(mob).is_some() && mob.target().is_none() && self.peek_time > 0
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.peek_time = reduced_tick_delay(20 * (1 + rand::random_range(0..3)));
        if let Some(shulker) = Self::shulker(mob) {
            shulker.set_peek_amount(30);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if mob.target().is_none()
            && let Some(shulker) = Self::shulker(mob)
        {
            shulker.set_peek_amount(0);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, _mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        self.peek_time -= 1;
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

    /// Vanilla `Shulker.tick`: re-derive the attachment face when the shell can
    /// no longer stay where it is, then advance the lid animation.
    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        if let Some(living) = entity.as_living_entity() {
            living.tick_living_entity(&entity);
        }

        if !self.is_passenger() && !self.can_stay_at(self.block_position(), self.attach_face()) {
            self.find_new_attachment();
        }

        if self.update_peek_amount() {
            self.on_peek_amount_change();
        }
    }

    /// Vanilla `Shulker.getDeltaMovement`: a shulker has no velocity at all.
    fn velocity(&self) -> DVec3 {
        DVec3::ZERO
    }

    /// Vanilla `Shulker.setDeltaMovement`: velocity changes are dropped, which
    /// is what makes the shell immune to knockback, explosion impulses, fluid
    /// currents and gravity.
    fn set_velocity(&self, _velocity: DVec3) {}

    /// Vanilla `Shulker.move`: a shulker-box lid teleports the shell instead of
    /// pushing it; every other mover defers to `Entity.move`.
    ///
    /// A shulker's delta movement is pinned to zero (see [`Self::velocity`] and
    /// [`Self::set_velocity`]), so its self-driven travel always carries a zero
    /// delta and the plain `Entity.move` body would be a no-op; the only mover
    /// that hands a shulker an explicit delta is a piston. That path is replayed
    /// through the same base helpers (`limitPistonMovement`, collision
    /// resolution, resolved-movement commit). The base body's velocity
    /// bookkeeping is inert here because every `set_velocity` call is dropped.
    fn move_entity(self: Arc<Self>, mover_type: MoverType, delta: DVec3) -> Option<MoveResult> {
        if mover_type == MoverType::ShulkerBox {
            self.teleport_somewhere();
            return None;
        }

        if mover_type != MoverType::Piston {
            return None;
        }

        let world = self.level()?;
        let requested = self.base().limit_piston_movement(delta, world.game_time());
        if requested == DVec3::ZERO {
            return None;
        }
        let movement = self.base().consume_stuck_speed_multiplier(requested, false);

        let entity = EntityEventSource::as_entity_event_source(&*self);
        let physics_state = physics_state_for_move(entity);
        let start_position = physics_state.position();
        let collision_world = WorldCollisionProvider::for_entity(&world, entity);
        let result = resolve_entity_movement(&physics_state, movement, mover_type, &collision_world);

        record_movement_for_block_effects(
            entity,
            start_position,
            result.final_position,
            movement,
            result.actual_movement,
        );

        if should_apply_resolved_movement(movement, result.actual_movement) {
            self.reset_fall_distance_on_resetting_clip(&world, result.actual_movement);
            if let Err(error) = self.try_set_position(result.final_position) {
                log::debug!(
                    "Rejected resolved piston movement for entity {}: {error}",
                    self.id()
                );
                self.remove_latest_movement_recording();
                return None;
            }
        }

        Some(result)
    }

    /// Vanilla `Shulker.push`: the shell pushes nobody.
    fn push_entity(&self, _entity: &dyn Entity) {}

    /// Vanilla `Shulker.setPos`: a position change through teleports resets the
    /// synced peek amount and flags a sync.
    fn try_set_position(&self, pos: DVec3) -> Result<(), EntityMoveError> {
        let old_pos = self.block_position();
        let result = self.try_set_position_base(pos);
        if self.tick_count() != 0 && self.block_position() != old_pos {
            self.entity_data.lock().peek.set(DEFAULT_PEEK);
            self.mark_velocity_sync();
        }
        result
    }

    /// Vanilla `Shulker.getMovementEmission`: a shulker emits no movement.
    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::None
    }

    /// Vanilla `Shulker.makeBoundingBox`: the shell box grows along the face it
    /// opens towards.
    fn make_bounding_box_at(&self, position: DVec3) -> WorldAabb {
        let peek = Self::physical_peek(self.peek_amount.lock().current);
        let direction = self.attach_face().opposite();
        self.progress_aabb(direction, peek, position)
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        let dimensions = if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        };

        // Vanilla `Shulker.getDefaultDimensions`: a downward-facing shulker
        // grows taller as its lid opens.
        let peek = self.peek_amount.lock().current;
        if self.attach_face() == Direction::Down && peek > 0.0 {
            let height_factor = 1.0 + peek;
            return EntityDimensions::new(
                dimensions.width,
                dimensions.height * height_factor,
                dimensions.eye_height * height_factor,
            );
        }

        dimensions
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
        nbt.insert("Peek", self.peek_amount());
        nbt.insert("Color", self.color());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(face) = nbt.byte("AttachFace") {
            self.set_attach_face(Direction::from_3d_data_value(i32::from(face)));
        }
        self.set_peek_amount(nbt.byte("Peek").unwrap_or(0));
        self.set_color(nbt.byte("Color").unwrap_or(DEFAULT_COLOR));
    }
    /// Vanilla `Shulker.hurtServer`: arrows are ignored while the shell is
    /// closed, a hurt shulker below half health sometimes teleports away, and a
    /// shulker bullet that connects makes the shell retaliate.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_closed()
            && source
                .direct_entity()
                .is_some_and(|entity| entity.is_abstract_arrow())
        {
            return false;
        }

        let was_hurt = LivingEntity::hurt_server(self, world, source, amount);
        if !was_hurt {
            return false;
        }

        if self.get_health() < self.get_max_health() * 0.5 && rand::random_range(0..4) == 0 {
            self.teleport_somewhere();
        } else if source.is(&vanilla_damage_type_tags::DamageTypeTag::IS_PROJECTILE)
            && source.direct_entity().is_some_and(|entity| {
                entity.entity_type() == &vanilla_entities::SHULKER_BULLET
            })
        {
            self.hit_by_shulker_bullet();
        }

        true
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

    /// Vanilla `Shulker.getMaxHeadXRot`.
    fn max_head_x_rot(&self) -> f32 {
        180.0
    }

    /// Vanilla `Shulker.getMaxHeadYRot`.
    fn max_head_y_rot(&self) -> f32 {
        180.0
    }

    /// Vanilla `Shulker.playAmbientSound`: a closed shell stays silent.
    fn play_ambient_sound(&self) {
        if self.is_closed() {
            return;
        }
        self.make_sound(self.ambient_sound());
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

    /// Vanilla `Shulker.finalizeSpawn`: face yaw zero and refresh the
    /// previous-position snapshot before `Mob`'s spawn rolls.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.set_rotation((0.0, self.rotation().1));
        self.set_y_head_rot(0.0);
        self.set_old_position_to_current();
        self.base().set_old_rotation_to_current();
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }
}

impl Monster for ShulkerEntity {}

impl PathfinderMob for ShulkerEntity {}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
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

        let mut nbt = NbtCompound::new();
        shulker.save_additional(&mut nbt);
        assert_eq!(nbt.byte("Peek"), Some(20));
        assert_eq!(nbt.byte("Color"), Some(4));
        assert_eq!(shulker.entity_type(), &vanilla_entities::SHULKER);
    }
}
