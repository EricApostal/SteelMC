//! Vanilla `Enderman` (`net.minecraft.world.entity.monster.Enderman`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::EndermanEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_block_tags, vanilla_blocks, vanilla_entities,
    vanilla_game_events, vanilla_game_rules, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::UpdateFlags;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier,
};

use crate::behavior::BLOCK_BEHAVIORS;
use crate::behavior::BlockLootContext;
use crate::entity::ai::goal::{
    FloatGoal, Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, RandomLookAroundGoal, ResetUniversalAngerTargetGoal,
    WaterAvoidingRandomStrollGoal,
};
use crate::entity::ai::path::PathType;
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::neutral_mob::{NeutralMob, PersistentAnger};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource, EntityPose, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};

const DEFAULT_STEP_HEIGHT: f32 = 1.0;
const XP_REWARD: i32 = 5;
/// Vanilla `Enderman.SPEED_MODIFIER_ATTACKING_ID`.
const SPEED_MODIFIER_ATTACKING_ID: Identifier = Identifier::vanilla_static("attacking");
/// Vanilla `Enderman.SPEED_MODIFIER_ATTACKING` amount.
const SPEED_MODIFIER_ATTACKING_AMOUNT: f64 = 0.15;
/// Vanilla `Enderman.MIN_DEAGGRESSION_TIME`.
const MIN_DEAGGRESSION_TIME: i32 = 600;
/// Vanilla `Enderman.PERSISTENT_ANGER_TIME` bounds (`rangeOfSeconds(20, 39)`).
const PERSISTENT_ANGER_TIME_MIN: i64 = 20 * 20;
const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;
/// Vanilla `Enderman.repeatedlyTryToTeleport` attempt count.
const TELEPORT_ATTEMPTS: i32 = 64;
/// Vanilla `Enderman.customServerAiStep` daylight teleport roll.
const DAYLIGHT_TELEPORT_CHANCE_SCALE: f32 = 30.0;
/// Vanilla `Enderman.teleportTowards` distance.
const TELEPORT_TOWARDS_DISTANCE: f64 = 16.0;

/// Vanilla `Enderman`.
#[entity_behavior(class = "Enderman")]
pub struct EndermanEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<EndermanEntityData>,
    persistent_anger: PersistentAnger,
    /// Vanilla `Enderman.targetChangeTime`.
    target_change_time: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `EndermanEntity`.
unsafe impl DowncastType for EndermanEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/enderman");
}

impl EndermanEntity {
    /// Creates a new enderman entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved enderman entity.
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
        let mut entity_data = EndermanEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.carry_state.set(None);
        entity_data.creepy.set(false);
        entity_data.stared_at.set(false);

        // Vanilla `Enderman` constructor: water is impassable.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, -1.0);

        {
            // Vanilla `Enderman.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, EndermanFreezeWhenLookedAtGoal::new());
            goal_selector.add_goal(2, MeleeAttackGoal::new(1.0, false));
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());
            goal_selector.add_goal(10, EndermanLeaveBlockGoal::new());
            goal_selector.add_goal(11, EndermanTakeBlockGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, EndermanLookForPlayerGoal::new());
            target_selector.add_goal(2, HurtByTargetGoal::new());
            target_selector.add_goal(
                3,
                NearestAttackableTargetGoal::new_with_interval(
                    10,
                    true,
                    false,
                    |target, _world| target.entity_type() == &vanilla_entities::ENDERMITE,
                ),
            );
            target_selector.add_goal(4, ResetUniversalAngerTargetGoal::new(false));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            persistent_anger: PersistentAnger::new(),
            target_change_time: SyncMutex::new(0),
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

    /// Vanilla `Enderman.getCarriedBlock`.
    #[must_use]
    pub fn carried_block(&self) -> Option<BlockStateId> {
        *self.entity_data.lock().carry_state.get()
    }

    /// Vanilla `Enderman.setCarriedBlock`.
    pub fn set_carried_block(&self, state: Option<BlockStateId>) {
        self.entity_data.lock().carry_state.set(state);
    }

    /// Vanilla `Enderman.isCreepy`.
    #[must_use]
    pub fn is_creepy(&self) -> bool {
        *self.entity_data.lock().creepy.get()
    }

    /// Vanilla `Enderman.hasBeenStaredAt`.
    #[must_use]
    pub fn has_been_stared_at(&self) -> bool {
        *self.entity_data.lock().stared_at.get()
    }

    /// Vanilla `Enderman.setBeingStaredAt`.
    pub fn set_being_stared_at(&self) {
        self.entity_data.lock().stared_at.set(true);
    }

    /// Vanilla `Enderman.playStareSound` (server side: the `ENTITY_ENDERMAN_STARE` sound).
    pub fn play_stare_sound(&self) {
        if let Some(world) = self.level() {
            world.play_sound_at(
                &sound_events::ENTITY_ENDERMAN_STARE,
                SoundSource::Hostile,
                DVec3::new(self.position().x, self.get_eye_y(), self.position().z),
                2.5,
                1.0,
                None,
            );
        }
    }

    /// Vanilla `Enderman.teleport()`.
    pub fn teleport(&self) -> bool {
        if self.is_passenger() || !LivingEntity::is_alive(self) {
            return false;
        }
        let position = self.position();
        let xx = position.x + (rand::random::<f64>() - 0.5) * 64.0;
        let yy = position.y + f64::from(rand::random_range(0..64) - 32);
        let zz = position.z + (rand::random::<f64>() - 0.5) * 64.0;
        self.teleport_to_position(xx, yy, zz)
    }

    /// Vanilla `Enderman.teleportTowards`.
    fn teleport_towards(&self, target: &SharedEntity) -> bool {
        let position = self.position();
        let target_position = target.position();
        let direction = DVec3::new(
            position.x - target_position.x,
            (position.y + 0.5) - target.get_eye_y(),
            position.z - target_position.z,
        );
        let Some(direction) = direction.try_normalize() else {
            return false;
        };

        let xx = position.x + (rand::random::<f64>() - 0.5) * 8.0
            - direction.x * TELEPORT_TOWARDS_DISTANCE;
        let yy = position.y + f64::from(rand::random_range(0..16) - 8)
            - direction.y * TELEPORT_TOWARDS_DISTANCE;
        let zz = position.z + (rand::random::<f64>() - 0.5) * 8.0
            - direction.z * TELEPORT_TOWARDS_DISTANCE;
        self.teleport_to_position(xx, yy, zz)
    }

    /// Vanilla `Enderman.teleport(x, y, z)`.
    pub fn teleport_to_position(&self, x: f64, y: f64, z: f64) -> bool {
        if self.is_passenger() {
            return false;
        }
        let Some(world) = self.level() else {
            return false;
        };

        let old_pos = self.position();
        // Vanilla `canRandomlyTeleportTo` rejects water below the destination; `random_teleport`
        // owns the block-space search.
        let destination_below = BlockPos::containing(x, y, z).offset(0, -1, 0);
        if world
            .get_block_state(destination_below)
            .get_fluid_state()
            .is_water()
        {
            return false;
        }

        let result = self.random_teleport(&world, x, y, z, true);
        if result {
            self.game_event(&vanilla_game_events::TELEPORT);
            if !self.is_silent() {
                let old_block_pos = BlockPos::containing(old_pos.x, old_pos.y, old_pos.z);
                world.level_event(2018, old_block_pos, 0, None);
                world.play_sound_at(
                    &sound_events::ENTITY_ENDERMAN_TELEPORT,
                    SoundSource::Hostile,
                    old_pos,
                    1.0,
                    1.0,
                    None,
                );
                self.play_sound(&sound_events::ENTITY_ENDERMAN_TELEPORT, 1.0, 1.0);
            }
        }
        result
    }

    /// Vanilla `Enderman.repeatedlyTryToTeleport`.
    fn repeatedly_try_to_teleport(&self) {
        if self.is_passenger() {
            return;
        }
        for _ in 0..TELEPORT_ATTEMPTS {
            if self.teleport() {
                return;
            }
        }
    }

    /// Vanilla `Enderman.isBeingStaredBy`: a player looking at the enderman's head, without a
    /// carved-pumpkin disguise.
    fn is_being_stared_by(&self, player: &SharedEntity) -> bool {
        if player.as_player().is_none() {
            return false;
        }
        // Vanilla `PLAYER_NOT_WEARING_DISGUISE_ITEM`: a carved pumpkin head hides the player.
        let mut disguised = false;
        if let Some(living) = player.as_living_entity() {
            living.with_equipment_slot(EquipmentSlot::Head, &mut |stack| {
                disguised = stack.is(&vanilla_items::CARVED_PUMPKIN);
            });
        }
        if disguised {
            return false;
        }
        self.is_looking_at_me(player, 0.025)
    }

    /// Vanilla `LivingEntity.isLookingAtMe` reduced to the enderman stare check.
    fn is_looking_at_me(&self, player: &SharedEntity, tolerance: f64) -> bool {
        let look = player.calculate_view_vector(player.rotation().1, player.rotation().0);
        let to_enderman = DVec3::new(
            self.position().x - player.position().x,
            self.get_eye_y() - player.get_eye_y(),
            self.position().z - player.position().z,
        );
        if to_enderman.length_squared() < 1.0e-4 {
            return true;
        }
        let Some(to_enderman) = to_enderman.try_normalize() else {
            return false;
        };
        if look.dot(to_enderman) < 1.0 - tolerance {
            return false;
        }
        self.has_line_of_sight_cached(player.as_ref())
    }

    /// Vanilla `Enderman.customServerAiStep` daylight teleport.
    fn custom_step_enderman(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if !world.is_bright_outside() {
            return;
        }

        let target_change_time = *self.target_change_time.lock();
        let position = self.position();
        let eye_pos = BlockPos::new(
            steel_math::fast_floor(position.x),
            steel_math::fast_floor(self.get_eye_y()),
            steel_math::fast_floor(position.z),
        );
        let brightness = world.light_level_dependent_magic_value(eye_pos);
        if brightness <= 0.5 {
            return;
        }
        if !world.can_see_sky(self.block_position()) {
            return;
        }
        if rand::random::<f32>() * DAYLIGHT_TELEPORT_CHANCE_SCALE < (brightness - 0.4) * 2.0
            && self.base.tick_count() >= target_change_time + MIN_DEAGGRESSION_TIME
        {
            self.set_target(None);
            self.teleport();
        }
    }

    /// Vanilla `Enderman.dropCustomDeathLoot`: the carried block drops with a diamond-axe tool.
    fn drop_carried_block(&self) {
        let Some(carried) = self.carried_block() else {
            return;
        };
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla forges a diamond axe so the block's loot table yields axe-harvested drops.
        let tool = ItemStack::new(&vanilla_items::DIAMOND_AXE);
        let context = BlockLootContext::new(&world, self.block_position())
            .with_entity(Some(self.as_entity_event_source()))
            .with_tool(&tool);
        let block = carried.get_block();
        let drops = BLOCK_BEHAVIORS
            .get_behavior(block)
            .get_drops(carried, &context)
            .unwrap_or_default();

        let position = self.position();
        for drop in drops {
            let item = ItemEntity::with_item(
                &vanilla_entities::ITEM,
                next_entity_id(),
                position,
                drop,
                Arc::downgrade(&world),
            );
            let _ = world.try_add_entity(Arc::new(item));
        }
    }
}

impl NeutralMob for EndermanEntity {
    fn persistent_anger(&self) -> &PersistentAnger {
        &self.persistent_anger
    }

    fn start_persistent_anger_timer(&self) {
        self.set_time_to_remain_angry(
            PERSISTENT_ANGER_TIME_MIN
                + rand::random_range(0..(PERSISTENT_ANGER_TIME_MAX - PERSISTENT_ANGER_TIME_MIN)),
        );
    }
}

/// Vanilla `Enderman.EndermanFreezeWhenLookedAt`.
struct EndermanFreezeWhenLookedAtGoal {
    target: Option<SharedEntity>,
}

impl EndermanFreezeWhenLookedAtGoal {
    const fn new() -> Self {
        Self { target: None }
    }

    fn enderman(mob: &dyn PathfinderMob) -> Option<&EndermanEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<EndermanEntity>()
    }
}

impl Goal for EndermanFreezeWhenLookedAtGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };
        let Some(target) = mob.target() else {
            return false;
        };
        if !target
            .as_living_entity()
            .is_some_and(LivingEntity::is_alive)
        {
            return false;
        }
        if mob.position().distance_squared(target.position()) > 256.0 {
            return false;
        }
        if !enderman.is_being_stared_by(&target) {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };
        self.target
            .as_ref()
            .is_some_and(|target| enderman.is_being_stared_by(target))
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.target = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if let Some(target) = &self.target {
            mob.mob_base().controls().lock().look_control.set_look_at(
                target.position(),
                10.0,
                mob.max_head_x_rot(),
            );
        }
    }
}

/// Vanilla `Enderman.EndermanLookForPlayerGoal`: players provoke an enderman by staring.
struct EndermanLookForPlayerGoal {
    inner: NearestAttackableTargetGoal,
    pending_target: Option<SharedEntity>,
    aggro_time: i32,
    teleport_time: i32,
}

impl EndermanLookForPlayerGoal {
    fn new() -> Self {
        Self {
            inner: NearestAttackableTargetGoal::new_for_players_with_interval(
                10,
                false,
                false,
                |_, _| true,
            ),
            pending_target: None,
            aggro_time: 0,
            teleport_time: 0,
        }
    }

    fn enderman(mob: &dyn PathfinderMob) -> Option<&EndermanEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<EndermanEntity>()
    }
}

impl Goal for EndermanLookForPlayerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };
        if !self.inner.can_use(mob) {
            return false;
        }
        let Some(target) = mob.target() else {
            return false;
        };
        let provoked = enderman.is_being_stared_by(&target)
            || target
                .as_living_entity()
                .is_some_and(|living| NeutralMob::is_angry_at(enderman, living));
        if !provoked {
            mob.set_target(None);
            return false;
        }
        self.pending_target = Some(target);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(enderman) = Self::enderman(mob) else {
            return;
        };
        self.aggro_time = 5;
        self.teleport_time = 0;
        if let Some(pending) = self.pending_target.clone() {
            mob.set_target(Some(&pending));
        }
        enderman.set_being_stared_at();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
        self.pending_target = None;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(enderman) = Self::enderman(mob) else {
            return;
        };

        let Some(target) = mob.target() else {
            return;
        };

        if self.aggro_time > 0 {
            self.aggro_time -= 1;
            if self.aggro_time == 0 {
                self.teleport_time = 5;
            }
        } else if self.teleport_time > 0 {
            self.teleport_time -= 1;
            enderman.teleport_towards(&target);
        }

        mob.mob_base().controls().lock().look_control.set_look_at(
            target.position(),
            10.0,
            mob.max_head_x_rot(),
        );
        let _ = entity;
    }
}

/// Vanilla `Enderman.EndermanTakeBlockGoal`.
struct EndermanTakeBlockGoal;

impl EndermanTakeBlockGoal {
    const fn new() -> Self {
        Self
    }

    fn enderman(mob: &dyn PathfinderMob) -> Option<&EndermanEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<EndermanEntity>()
    }
}

impl Goal for EndermanTakeBlockGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };
        if enderman.carried_block().is_some() {
            return false;
        }
        let Some(world) = mob.level() else {
            return false;
        };
        if !world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING) {
            return false;
        }
        // Vanilla `reducedTickDelay(20)`.
        rand::random_range(0..10) == 0
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(enderman) = Self::enderman(mob) else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };
        let position = mob.position();
        let x = steel_math::fast_floor(position.x - 2.0 + rand::random::<f64>() * 4.0);
        let y = steel_math::fast_floor(position.y + rand::random::<f64>() * 3.0);
        let z = steel_math::fast_floor(position.z - 2.0 + rand::random::<f64>() * 4.0);
        let pos = BlockPos::new(x, y, z);
        let state = world.get_block_state(pos);
        let block = state.get_block();
        if !block.has_tag(&vanilla_block_tags::BlockTag::ENDERMAN_HOLDABLE) {
            return;
        }
        enderman.set_carried_block(Some(state));
        world.set_block(
            pos,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        enderman.game_event(&vanilla_game_events::ENTITY_ACTION);
    }
}

/// Vanilla `Enderman.EndermanLeaveBlockGoal`.
struct EndermanLeaveBlockGoal;

impl EndermanLeaveBlockGoal {
    const fn new() -> Self {
        Self
    }

    fn enderman(mob: &dyn PathfinderMob) -> Option<&EndermanEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<EndermanEntity>()
    }
}

impl Goal for EndermanLeaveBlockGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };
        if enderman.carried_block().is_none() {
            return false;
        }
        let Some(world) = mob.level() else {
            return false;
        };
        if !world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING) {
            return false;
        }
        // Vanilla `reducedTickDelay(2000)`.
        rand::random_range(0..1000) == 0
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(enderman) = Self::enderman(mob) else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };
        let Some(carried) = enderman.carried_block() else {
            return;
        };
        let position = mob.position();
        let x = steel_math::fast_floor(position.x - 1.0 + rand::random::<f64>() * 2.0);
        let y = steel_math::fast_floor(position.y + rand::random::<f64>() * 2.0);
        let z = steel_math::fast_floor(position.z - 1.0 + rand::random::<f64>() * 2.0);
        let pos = BlockPos::new(x, y, z);

        let target_state = world.get_block_state(pos);
        let below = pos.offset(0, -1, 0);
        let below_state = world.get_block_state(below);

        if !target_state.is_air() || below_state.is_air() {
            return;
        }
        if below_state.get_block() == &vanilla_blocks::BEDROCK {
            return;
        }
        if !BLOCK_BEHAVIORS
            .get_behavior(carried.get_block())
            .can_survive(carried, world.as_ref(), pos)
        {
            return;
        }

        world.set_block(pos, carried, UpdateFlags::UPDATE_ALL);
        enderman.game_event(&vanilla_game_events::BLOCK_PLACE);
        enderman.set_carried_block(None);
    }
}

impl Entity for EndermanEntity {
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_ENDERMAN_AMBIENT, 0.15, 1.0);
    }

    /// Vanilla `Enderman.hurtServer`: projectiles make it teleport instead of taking damage.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if LivingEntity::is_invulnerable_to(self, world, source) {
            return false;
        }
        if source.is(&steel_utils::Identifier::vanilla_static("is_projectile"))
            && !self.is_passenger()
        {
            self.repeatedly_try_to_teleport();
            return false;
        }

        let hurt = LivingEntity::hurt_server(self, world, source, amount);
        if source.causing_entity().is_none() && rand::random_range(0..10) != 0 {
            self.teleport();
        }
        hurt
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        if let Some(carried) = self.carried_block() {
            nbt.insert("carriedBlockState", i32::from(carried.0));
        }
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(state_id) = nbt.int("carriedBlockState") {
            let state_id = u16::try_from(state_id).ok().map(BlockStateId);
            if let Some(state) = state_id
                && !state.is_air()
            {
                self.set_carried_block(Some(state));
            }
        }
        self.read_persistent_anger_save_data(&nbt);
    }
}

impl LivingEntity for EndermanEntity {
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
        Some(&sound_events::ENTITY_ENDERMAN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ENDERMAN_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        self.custom_step_enderman();
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        self.update_persistent_anger(true);
        Monster::monster_ai_step(self, entity)
    }

    fn on_death(&self, _source: &DamageSource) {
        self.drop_carried_block();
    }
}

impl Mob for EndermanEntity {
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
        Some(if self.is_creepy() {
            &sound_events::ENTITY_ENDERMAN_SCREAM
        } else {
            &sound_events::ENTITY_ENDERMAN_AMBIENT
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `Enderman.setTarget`: the attacking modifier and the creepy/stared flags follow
    /// the target.
    fn set_target(&self, target: Option<&SharedEntity>) -> bool {
        // Vanilla `Enderman.setTarget` calls `super.setTarget`; the `Mob` default body is
        // `mob_base().set_target(..)` with `is_valid_target`, so call that directly — a
        // `Mob::set_target(self, ..)` UFCS call would re-enter this override.
        let changed = self
            .mob_base()
            .set_target(target, |target| Mob::is_valid_target(self, target));

        {
            let mut entity_data = self.entity_data.lock();
            if target.is_none() {
                *self.target_change_time.lock() = 0;
                entity_data.creepy.set(false);
                entity_data.stared_at.set(false);
            } else {
                entity_data.creepy.set(true);
            }
        }

        let mut attributes = self.attributes().lock();
        if target.is_none() {
            attributes.remove_modifier(
                vanilla_attributes::MOVEMENT_SPEED,
                &SPEED_MODIFIER_ATTACKING_ID,
            );
        } else if !attributes.has_modifier(
            vanilla_attributes::MOVEMENT_SPEED,
            &SPEED_MODIFIER_ATTACKING_ID,
        ) {
            attributes.add_modifier(
                vanilla_attributes::MOVEMENT_SPEED,
                AttributeModifier {
                    id: SPEED_MODIFIER_ATTACKING_ID,
                    amount: SPEED_MODIFIER_ATTACKING_AMOUNT,
                    operation: AttributeModifierOperation::AddValue,
                },
                false,
            );
        }

        changed
    }

    /// Vanilla `Enderman.requiresCustomPersistence` keeps a block-carrying enderman.
    fn requires_custom_persistence(&self) -> bool {
        self.carried_block().is_some()
    }

    /// Vanilla `Enderman.getAmbientSoundInterval` is the shared mob default.
    fn max_head_x_rot(&self) -> f32 {
        40.0
    }
}

impl Monster for EndermanEntity {}

impl PathfinderMob for EndermanEntity {
    /// Vanilla `Enderman.getWalkTargetValue` returns 0 for every position.
    fn get_walk_target_value(&self, _pos: BlockPos) -> f32 {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use std::sync::Weak;
    use steel_registry::vanilla_blocks;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::EndermanEntity;
    use crate::entity::{Entity, Mob};

    #[test]
    fn enderman_carried_block_and_creepy_flags_round_trip() {
        init_vanilla_registry();
        let enderman =
            EndermanEntity::new(&vanilla_entities::ENDERMAN, 1, DVec3::ZERO, Weak::new());

        assert!(enderman.carried_block().is_none());
        assert!(!enderman.is_creepy());

        let stone = vanilla_blocks::STONE.default_state();
        enderman.set_carried_block(Some(stone));
        assert_eq!(enderman.carried_block(), Some(stone));
        assert!(enderman.requires_custom_persistence());

        let mut nbt = NbtCompound::new();
        enderman.save_additional(&mut nbt);
        assert!(nbt.int("carriedBlockState").is_some());
        assert_eq!(enderman.entity_type(), &vanilla_entities::ENDERMAN);
    }
}
