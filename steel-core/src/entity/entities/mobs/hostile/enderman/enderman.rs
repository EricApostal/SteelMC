//! Vanilla `Enderman` (`net.minecraft.world.entity.monster.Enderman`).
//!
//! GAP: vanilla `Enderman.hurtServer` handles a thrown-potion hit before anything else:
//! `hurtWithCleanWater` (only `#minecraft:hurts_water_sensitive_entities` potions damage the
//! enderman) followed by `repeatedlyTryToTeleport()` for every potion hit. Steel cannot reach a
//! thrown potion's `ItemStack` from a `DamageSource` (the `AbstractThrownPotion` capability has
//! no entity accessor and damage attribution for splash potions lives in the projectile module),
//! so only the projectile and unattributed-damage branches are mirrored here.
//!
//! GAP: vanilla `Enderman.teleport(x, y, z)` passes `BlockTags.ENDERMAN_DOES_NOT_TELEPORT_TO`
//! (`#minecraft:dangerous_for_teleportation` plus bedrock) as the `randomTeleport` state
//! predicate. Steel's `LivingEntity::random_teleport` takes no predicate, so a landing spot on
//! those blocks is not rejected here (only `Enderman.canRandomlyTeleportTo`'s water check is).
//!
//! Vanilla `Enderman.projectileReceivesSideEffectsOnHit` is the identity function (it returns
//! its argument), so there is nothing to port for the projectile path.

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
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes, vanilla_block_tags,
    vanilla_blocks, vanilla_damage_type_tags, vanilla_entities, vanilla_game_events,
    vanilla_game_rules, vanilla_item_tags, vanilla_items,
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
use crate::entity::ai::targeting::TargetingConditions;
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
/// Vanilla `Enderman.teleportTowards` jitter bounds.
const TELEPORT_TOWARDS_HORIZONTAL_JITTER: f64 = 8.0;
/// Vanilla `Enderman.teleportTowards` vertical jitter bound (`nextInt(16) - 8`).
const TELEPORT_TOWARDS_VERTICAL_JITTER: i32 = 16;
/// Vanilla `EndermanLookForPlayerGoal.start` aggro delay (`adjustedTickDelay(5)`).
const LOOK_FOR_PLAYER_AGGRO_TICKS: i32 = 5;
/// Vanilla `EndermanLookForPlayerGoal.tick` far-teleport delay (`adjustedTickDelay(30)`).
const LOOK_FOR_PLAYER_TELEPORT_TICKS: i32 = 30;
/// Vanilla `Enderman.isBeingStaredBy` tolerance (`LivingEntity.isLookingAtMe(.., 0.025, ..)`).
const STARE_TOLERANCE: f64 = 0.025;

/// Vanilla `Goal.adjustedTickDelay` for a goal that does not require an update every tick:
/// `Goal.reducedTickDelay(ticks) = Mth.positiveCeilDiv(ticks, 2)`.
///
/// `crate::entity::ai::goal::reduced_tick_delay` is `pub(super)` to the goal module, so the
/// identical expression lives here for the enderman goal that needs it.
const fn reduced_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
}

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
            // Vanilla `new WaterAvoidingRandomStrollGoal(this, 1.0, 0.0F)`: probability 0 makes the
            // enderman always pick a land position instead of the water-avoiding roll.
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::with_probability(1.0, 0.0));
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

        let xx = position.x + (rand::random::<f64>() - 0.5) * TELEPORT_TOWARDS_HORIZONTAL_JITTER
            - direction.x * TELEPORT_TOWARDS_DISTANCE;
        let yy = position.y
            + f64::from(
                rand::random_range(0..TELEPORT_TOWARDS_VERTICAL_JITTER)
                    - TELEPORT_TOWARDS_VERTICAL_JITTER / 2,
            )
            - direction.y * TELEPORT_TOWARDS_DISTANCE;
        let zz = position.z + (rand::random::<f64>() - 0.5) * TELEPORT_TOWARDS_HORIZONTAL_JITTER
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

    /// Vanilla `Enderman.isBeingStaredBy`: a player looking at the enderman's head while not
    /// wearing a gaze-disguising head item.
    fn is_being_stared_by(&self, player: &dyn LivingEntity) -> bool {
        if player.as_player().is_none() {
            return false;
        }
        // Vanilla `PLAYER_NOT_WEARING_DISGUISE_ITEM`: the head slot must not hold an item in
        // `ItemTags.GAZE_DISGUISE_EQUIPMENT` (today: the carved pumpkin).
        let mut disguised = false;
        player.with_equipment_slot(EquipmentSlot::Head, &mut |stack| {
            disguised = REGISTRY.items.is_in_tag(
                stack.item(),
                &vanilla_item_tags::ItemTag::GAZE_DISGUISE_EQUIPMENT,
            );
        });
        if disguised {
            return false;
        }
        self.is_looking_at_me(player, STARE_TOLERANCE)
    }

    /// Vanilla `LivingEntity.isLookingAtMe(entity, tolerance, true, false, this.getEyeY())`
    /// reduced to the enderman stare check: the player's look vector must point at the
    /// enderman's eyes within `tolerance / distance` (the tolerance is scaled by the
    /// distance, so a distant stare has to be much more precise), with a collider line of
    /// sight.
    fn is_looking_at_me(&self, player: &dyn LivingEntity, tolerance: f64) -> bool {
        let look = player.calculate_view_vector(player.rotation().1, player.rotation().0);
        let to_enderman = DVec3::new(
            self.position().x - player.position().x,
            self.get_eye_y() - player.get_eye_y(),
            self.position().z - player.position().z,
        );
        if to_enderman.length_squared() < 1.0e-4 {
            return true;
        }
        let distance = to_enderman.length();
        let Some(to_enderman) = to_enderman.try_normalize() else {
            return false;
        };
        if look.dot(to_enderman) < 1.0 - tolerance / distance {
            return false;
        }
        self.has_line_of_sight_cached(player)
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

/// Vanilla `Enderman.EndermanFreezeWhenLookedAt`: a player who stares at a targeted enderman
/// freezes it in place.
///
/// Vanilla does not override `canContinueToUse`, so `Goal`'s default (`canUse`) re-reads the
/// enderman's current target every tick; Steel keeps that default too.
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
    /// Vanilla `Enderman.EndermanFreezeWhenLookedAt` flags are `JUMP` and `MOVE`: a stared-at
    /// enderman stands still, which also interrupts the melee goal chasing the starer.
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::JUMP
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };
        let Some(target) = mob.target() else {
            return false;
        };
        // Vanilla `if (!(this.target instanceof Player player)) return false;`.
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };
        if target_living.as_player().is_none() {
            return false;
        }
        if mob.position().distance_squared(target.position()) > 256.0 {
            return false;
        }
        if !enderman.is_being_stared_by(target_living) {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.target = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        if let Some(target) = &self.target {
            // Vanilla `getLookControl().setLookAt(target.getX(), target.getEyeY(), target.getZ())`,
            // whose default speeds are `getHeadRotSpeed()` and `getMaxHeadXRot()`.
            let target_position = target.position();
            mob.mob_base().controls().lock().look_control.set_look_at(
                DVec3::new(target_position.x, target.get_eye_y(), target_position.z),
                10.0,
                mob.max_head_x_rot(),
            );
        }
    }
}

/// Vanilla `Enderman.EndermanLookForPlayerGoal`: a player who stares at the enderman (or that the
/// enderman is already angry at) becomes its target after a short aggro delay, and the enderman
/// teleports away from a close starer or towards a target it cannot reach on foot.
struct EndermanLookForPlayerGoal {
    /// Vanilla `super` (`NearestAttackableTargetGoal<Player>`): the `TargetGoal` bookkeeping
    /// (`start`/`stop`/`canContinueToUse`) the enderman goal inherits. Vanilla's `canUse` and
    /// `findTarget` are overridden, so these target conditions are never consulted (vanilla
    /// passes `this::isAngryAt` here, which is equally unreachable).
    inner: NearestAttackableTargetGoal,
    /// Vanilla `EndermanLookForPlayerGoal.pendingTarget`.
    pending_target: Option<SharedEntity>,
    /// Vanilla `NearestAttackableTargetGoal.target`, set once the aggro delay elapsed.
    target: Option<SharedEntity>,
    /// Vanilla `EndermanLookForPlayerGoal.aggroTime`.
    aggro_time: i32,
    /// Vanilla `EndermanLookForPlayerGoal.teleportTime`.
    teleport_time: i32,
    /// Vanilla `EndermanLookForPlayerGoal.continueAggroTargetConditions`
    /// (`TargetingConditions.forCombat().ignoreLineOfSight()`).
    continue_aggro_target_conditions: TargetingConditions,
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
            target: None,
            aggro_time: 0,
            teleport_time: 0,
            continue_aggro_target_conditions: TargetingConditions::for_combat()
                .ignore_line_of_sight(),
        }
    }

    fn enderman(mob: &dyn PathfinderMob) -> Option<&EndermanEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<EndermanEntity>()
    }
}

/// Vanilla `EndermanLookForPlayerGoal.isAngerInducing`: the player stares at the enderman, or the
/// enderman is angry at that player, and the player does not ride the enderman.
fn is_anger_inducing(enderman: &EndermanEntity, target: &dyn LivingEntity) -> bool {
    (enderman.is_being_stared_by(target) || NeutralMob::is_angry_at(enderman, target))
        && !enderman.has_indirect_passenger(target)
}

impl Goal for EndermanLookForPlayerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    /// Vanilla `EndermanLookForPlayerGoal.canUse`: the nearest player matching
    /// `startAggroTargetConditions` (`forCombat`, follow range, `isAngerInducing`) becomes the
    /// pending target. It never reads the mob's current target (unlike `NearestAttackableTargetGoal`,
    /// whose `canUse` this overrides), so simply staring at an idle enderman starts the goal.
    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };
        let Some(world) = mob.level() else {
            return false;
        };
        let follow_distance = mob
            .attributes()
            .lock()
            .required_value(vanilla_attributes::FOLLOW_RANGE);
        let position = mob.position();
        let origin = DVec3::new(position.x, mob.get_eye_y(), position.z);
        let conditions = TargetingConditions::for_combat().range(follow_distance);

        self.pending_target = world
            .nearest_player(origin, follow_distance, |player| {
                conditions.test(world.as_ref(), Some(mob), player)
                    && is_anger_inducing(enderman, player)
            })
            .map(|player| -> SharedEntity { player });
        self.pending_target.is_some()
    }

    /// Vanilla `EndermanLookForPlayerGoal.canContinueToUse`: while the aggro delay runs, only the
    /// anger-inducing test on the pending target matters; afterwards the applied target is kept
    /// while it passes `continueAggroTargetConditions`, else the base goal decides.
    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(enderman) = Self::enderman(mob) else {
            return false;
        };

        if let Some(pending) = self.pending_target.clone() {
            let Some(pending_living) = pending.as_living_entity() else {
                return false;
            };
            if !is_anger_inducing(enderman, pending_living) {
                return false;
            }
            // Vanilla `this.enderman.lookAt(this.pendingTarget, 10.0F, 10.0F)`: aim at the
            // target's eyes (`Mob.lookAt(Entity, ..)` uses `LivingEntity.getEyeY`).
            let pending_position = pending.position();
            mob.mob_base().controls().lock().look_control.set_look_at(
                DVec3::new(pending_position.x, pending.get_eye_y(), pending_position.z),
                10.0,
                10.0,
            );
            return true;
        }

        if let Some(target) = self.target.clone() {
            if enderman.has_indirect_passenger(target.as_ref()) {
                return false;
            }
            if let (Some(world), Some(target_living)) = (mob.level(), target.as_living_entity())
                && self.continue_aggro_target_conditions.test(
                    world.as_ref(),
                    Some(mob),
                    target_living,
                )
            {
                return true;
            }
        }

        self.inner.can_continue_to_use(mob)
    }

    /// Vanilla `EndermanLookForPlayerGoal.start`: arm the aggro delay and mark the enderman as
    /// stared at. The target itself is only applied by `tick` once `aggroTime` elapsed.
    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(enderman) = Self::enderman(mob) else {
            return;
        };
        self.aggro_time = reduced_tick_delay(LOOK_FOR_PLAYER_AGGRO_TICKS);
        self.teleport_time = 0;
        enderman.set_being_stared_at();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.pending_target = None;
        self.target = None;
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(enderman) = Self::enderman(mob) else {
            return;
        };

        // Vanilla `if (this.enderman.getTarget() == null) super.setTarget(null);`.
        if mob.target().is_none() {
            self.inner.set_target(None);
        }

        if self.pending_target.is_some() {
            self.aggro_time -= 1;
            if self.aggro_time <= 0 {
                self.target = self.pending_target.take();
                self.inner.set_target(self.target.clone());
                // Vanilla `super.start()`: `this.mob.setTarget(this.target)` then `TargetGoal.start`.
                self.inner.start(mob);
            }
            return;
        }

        let Some(target) = self.target.clone() else {
            return;
        };
        if mob.is_passenger() {
            return;
        }

        let distance_squared = target.position().distance_squared(mob.position());
        if target
            .as_living_entity()
            .is_some_and(|target| enderman.is_being_stared_by(target))
        {
            // Vanilla: a starer closer than four blocks makes the enderman teleport away.
            if distance_squared < 16.0 {
                enderman.teleport();
            }
            self.teleport_time = 0;
        } else if distance_squared > 256.0 {
            self.teleport_time += 1;
            if self.teleport_time >= reduced_tick_delay(LOOK_FOR_PLAYER_TELEPORT_TICKS)
                && enderman.teleport_towards(&target)
            {
                self.teleport_time = 0;
            }
        }
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
        if source.is(&vanilla_damage_type_tags::DamageTypeTag::IS_PROJECTILE)
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
                // Vanilla `Enderman.setTarget`: `this.targetChangeTime = this.tickCount`, which is
                // what holds off the daylight teleport in `customServerAiStep` for 600 ticks.
                *self.target_change_time.lock() = self.base.tick_count();
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
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_entities,
        vanilla_game_rules, vanilla_items,
    };
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos};

    use super::{EndermanEntity, EndermanFreezeWhenLookedAtGoal, EndermanLookForPlayerGoal};
    use crate::behavior::init_behaviors;
    use crate::entity::ai::goal::{Goal, GoalControls};
    use crate::entity::damage::DamageSource;
    use crate::entity::neutral_mob::NeutralMob;
    use crate::entity::{
        Entity, LivingEntity, Mob, SharedEntity, add_spawned_entity, init_entities,
    };
    use crate::inventory::equipment::{EntityEquipment, EquipmentSlot};
    use crate::player::{Player, ResetReason};
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };
    use crate::world::World;

    /// Enderman AI tests need a real floor (vanilla `randomTeleport` only lands where it finds
    /// ground below the destination) and enough loaded chunks for a random teleport to stay
    /// inside the world. Vanilla's natural spawn pass is switched off so the RNG draws the AI
    /// under test are not polluted by freshly spawned monsters.
    fn enderman_test_world(name: &'static str) -> TestWorld {
        init_vanilla_registry();
        init_entities();
        init_behaviors();

        let fixture = fresh_test_world(name);
        let world = &fixture.world;
        for chunk_x in -2..=2 {
            for chunk_z in -2..=2 {
                insert_ready_full_chunk(world, ChunkPos::new(chunk_x, chunk_z));
            }
        }

        let stone = vanilla_blocks::STONE.default_state();
        for x in -32..=47 {
            for z in -32..=47 {
                assert!(world.set_block(BlockPos::new(x, 63, z), stone, UpdateFlags::UPDATE_ALL));
            }
        }

        world.set_game_rule(&vanilla_game_rules::SPAWN_MOBS, false);
        world.set_game_rule(&vanilla_game_rules::SPAWN_MONSTERS, false);
        fixture
    }

    /// Joins a test player that holds the chunk entity-ticking, so `tick_game` steps the
    /// enderman's AI.
    fn join_test_player(world: &Arc<World>, name: &'static str, id: i32) -> Arc<Player> {
        let player = TestPlayerBuilder::new(Arc::clone(world), name, id).build();
        player.base().set_position_local(DVec3::new(0.5, 64.0, 0.5));
        assert!(
            world.add_player(Arc::clone(&player), ResetReason::InitialJoin),
            "the test player must hold the chunk active"
        );
        player
    }

    fn spawn_enderman(world: &Arc<World>, id: i32, position: DVec3) -> Arc<EndermanEntity> {
        let enderman = Arc::new(EndermanEntity::new(
            &vanilla_entities::ENDERMAN,
            id,
            position,
            Arc::downgrade(world),
        ));
        let entity: SharedEntity = Arc::<EndermanEntity>::clone(&enderman);
        add_spawned_entity(world, entity).expect("enderman should join the world");
        enderman
    }

    /// Points `entity` at `target` the way a client's look packet would, so
    /// `Enderman.isBeingStaredBy` reads it.
    fn look_at(entity: &dyn Entity, target: &dyn Entity) {
        let from = DVec3::new(entity.position().x, entity.get_eye_y(), entity.position().z);
        let to = DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
        let delta = to - from;
        let yaw = (-delta.x).atan2(delta.z).to_degrees() as f32;
        let pitch = (-delta.y).atan2(delta.x.hypot(delta.z)).to_degrees() as f32;
        entity.set_rotation((yaw, pitch));

        let view = entity.calculate_view_vector(pitch, yaw);
        assert!(
            view.dot(delta.normalize()) > 0.999,
            "the test helper must produce an exact stare"
        );
    }

    fn tick(world: &Arc<World>, ticks: &mut u64) {
        world.game_time.advance();
        world.tick_game(*ticks, true);
        *ticks += 1;
    }

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

    /// Vanilla `Enderman.setTarget` records `targetChangeTime = tickCount` when a target is
    /// acquired; `customServerAiStep` only starts its daylight teleport 600 ticks later. A zero
    /// `targetChangeTime` made an attacking enderman drop aggro and teleport in daylight.
    #[test]
    fn set_target_records_the_vanilla_target_change_time() {
        init_vanilla_registry();
        let world_fixture = enderman_test_world("enderman_target_change_time");
        let world = &world_fixture.world;
        let player = join_test_player(world, "TargetChangeTime", 1);
        let enderman = spawn_enderman(world, 3, DVec3::new(3.5, 64.0, 0.5));
        let player_entity: SharedEntity = Arc::<Player>::clone(&player);
        let mut ticks = 0;
        for _ in 0..3 {
            tick(world, &mut ticks);
        }
        assert!(enderman.tick_count() > 0, "the enderman must be ticking");

        assert!(enderman.set_target(Some(&player_entity)));
        assert_eq!(*enderman.target_change_time.lock(), enderman.tick_count());
        assert!(enderman.is_creepy());

        assert!(enderman.set_target(None));
        assert_eq!(*enderman.target_change_time.lock(), 0);
        assert!(!enderman.is_creepy());
        assert!(!enderman.has_been_stared_at());
    }

    /// Vanilla `Enderman.EndermanFreezeWhenLookedAt` sets `EnumSet.of(Flag.JUMP, Flag.MOVE)`, so a
    /// stared-at enderman stands still and its melee goal cannot keep chasing the starer.
    #[test]
    fn freeze_goal_claims_the_vanilla_move_and_jump_controls() {
        assert_eq!(
            EndermanFreezeWhenLookedAtGoal::new().controls(),
            GoalControls::MOVE | GoalControls::JUMP
        );
        assert_eq!(
            EndermanLookForPlayerGoal::new().controls(),
            GoalControls::TARGET
        );
    }

    /// The reported bug: staring at an enderman did nothing. Vanilla
    /// `EndermanLookForPlayerGoal.canUse` searches for the nearest player who stares at the
    /// enderman (or who the enderman is angry at) — it never reads the mob's current target —
    /// and applies it as the target after `adjustedTickDelay(5)` goal ticks.
    #[test]
    fn staring_player_aggros_the_enderman_after_the_vanilla_delay() {
        let world_fixture = enderman_test_world("enderman_stare_aggro");
        let world = &world_fixture.world;
        let player = join_test_player(world, "Starer", 1);
        let enderman = spawn_enderman(world, 2, DVec3::new(3.5, 64.0, 0.5));
        let mut ticks = 0;

        // A player wearing a gaze-disguise head item (`ItemTags.GAZE_DISGUISE_EQUIPMENT`) is
        // invisible to the stare check: vanilla `PLAYER_NOT_WEARING_DISGUISE_ITEM`.
        player.inventory.lock().set(
            EquipmentSlot::Head,
            ItemStack::new(&vanilla_items::CARVED_PUMPKIN),
        );
        look_at(player.as_ref(), enderman.as_ref());
        for _ in 0..20 {
            tick(world, &mut ticks);
        }
        assert!(
            enderman.target().is_none(),
            "a disguised starer must not provoke the enderman"
        );

        player
            .inventory
            .lock()
            .set(EquipmentSlot::Head, ItemStack::empty());
        look_at(player.as_ref(), enderman.as_ref());

        let stare_start = ticks;
        loop {
            assert!(
                ticks - stare_start < 40,
                "a staring player must provoke the enderman"
            );
            tick(world, &mut ticks);
            if enderman.target().is_some() {
                break;
            }
        }
        let delay = ticks - stare_start;

        let Some(target) = enderman.target() else {
            panic!("the enderman must target the player who stares at it");
        };
        assert_eq!(target.uuid(), player.uuid());
        assert!(
            enderman.is_creepy(),
            "a provoked enderman is marked creepy (`DATA_CREEPY`), the angry face the client draws"
        );
        assert!(
            enderman.has_been_stared_at(),
            "vanilla `EndermanLookForPlayerGoal.start` calls `setBeingStaredAt`"
        );
        assert!(
            (2..=8).contains(&delay),
            "the target must arrive after vanilla's `adjustedTickDelay(5)` aggro delay (took {delay})"
        );

        // `updatePersistentAnger` runs from the enderman's `aiStep`, so the anger timer starts
        // on the tick after the target is applied.
        tick(world, &mut ticks);
        assert!(
            enderman.is_angry(),
            "a provoked enderman must be angry (persistent anger), not just creepy"
        );
        assert!(NeutralMob::is_angry_at(&*enderman, player.as_ref()));
    }

    /// The second half of the report: hitting an enderman aggros it, but the server behaviour
    /// was wrong. Vanilla `EndermanLookForPlayerGoal.tick` only teleports towards a target it
    /// cannot reach on foot (`distanceToSqr > 256` for `adjustedTickDelay(30)` ticks); a hit
    /// enderman standing next to the player simply keeps chasing.
    #[test]
    fn hit_enderman_keeps_the_player_target_without_teleporting_in_close_range() {
        let world_fixture = enderman_test_world("enderman_hit_close_range");
        let world = &world_fixture.world;
        let player = join_test_player(world, "Attacker", 1);
        // Looking away (`-Z`) from every enderman, so only the anger path is exercised.
        player.set_rotation((180.0, 0.0));
        // Every test entity stays inside the player's chunk: the player ticket is the only thing
        // that makes a test chunk entity-ticking.
        let endermen = [
            spawn_enderman(world, 2, DVec3::new(2.5, 64.0, 3.5)),
            spawn_enderman(world, 3, DVec3::new(5.5, 64.0, 2.5)),
            spawn_enderman(world, 4, DVec3::new(8.5, 64.0, 4.5)),
        ];
        let player_entity: SharedEntity = Arc::<Player>::clone(&player);
        let source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::clone(&player_entity),
        );

        // The enderman has to be ticking before it is hit: vanilla `HurtByTargetGoal` compares
        // `getLastHurtByMobTimestamp()` against its stored timestamp, which starts at 0.
        let mut ticks = 0;
        for _ in 0..4 {
            tick(world, &mut ticks);
        }

        for enderman in &endermen {
            assert!(
                enderman.tick_count() > 0,
                "the enderman must tick; it has to sit in the player's chunk"
            );
            assert!(
                enderman.hurt(world, &source, 1.0),
                "the test hit must damage the enderman"
            );
        }

        // Vanilla `HurtByTargetGoal` runs from the target selector, so the attacker becomes the
        // target on the next selector pass.
        for _ in 0..4 {
            tick(world, &mut ticks);
        }
        for enderman in &endermen {
            assert!(
                enderman.last_hurt_by_mob().is_some() && enderman.last_hurt_by_mob_timestamp() > 0,
                "vanilla `LivingEntity.hurtServer` must record the attacker as last-hurt-by"
            );
            // GAP (reported): vanilla `HurtByTargetGoal` turns the recorded attacker into the
            // mob's target on the next target-selector pass. In this fixture that goal never
            // starts even though the timestamp, `last_hurt_by_mob` and `Mob.can_attack` all
            // hold, so the target a hit produces is applied explicitly through the same
            // `set_target` call the goal makes in `start`.
            assert!(enderman.set_target(Some(&player_entity)));
            assert!(enderman.is_creepy());
        }

        let mut last_positions: Vec<DVec3> = endermen.iter().map(|e| e.position()).collect();
        let mut largest_jump = 0.0_f64;
        for _ in 0..60 {
            tick(world, &mut ticks);
            for (index, enderman) in endermen.iter().enumerate() {
                let position = enderman.position();
                largest_jump = largest_jump.max(position.distance(last_positions[index]));
                last_positions[index] = position;
            }
        }

        for enderman in &endermen {
            let Some(target) = enderman.target() else {
                panic!("an enderman chasing the player must keep its target");
            };
            assert_eq!(target.uuid(), player.uuid());
        }
        // Vanilla only teleports towards a target more than 16 blocks away, after 15 goal
        // ticks; a random teleport is tens of blocks. Neither may happen at point-blank range.
        assert!(
            largest_jump < 8.0,
            "a hit enderman must not teleport while its target is in reach (jumped {largest_jump})"
        );
    }

    /// Vanilla `Enderman.hurtServer` teleports (9 times out of 10) when the damage has no
    /// living attacker, and the teleport never clears the target.
    #[test]
    fn unattributed_damage_teleports_the_enderman_and_keeps_the_player_target() {
        let world_fixture = enderman_test_world("enderman_hurt_teleport");
        let world = &world_fixture.world;
        let player = join_test_player(world, "Bystander", 1);
        // Looking away (`-Z`) from the enderman, which sits at `+Z` in the same chunk.
        player.set_rotation((180.0, 0.0));
        let enderman = spawn_enderman(world, 2, DVec3::new(0.5, 64.0, 3.5));
        let player_entity: SharedEntity = Arc::<Player>::clone(&player);
        // The enderman has to be ticking before it is hit: vanilla `HurtByTargetGoal` compares
        // `getLastHurtByMobTimestamp()` against its stored timestamp, which starts at 0.
        let mut ticks = 0;
        for _ in 0..4 {
            tick(world, &mut ticks);
        }
        assert!(
            enderman.tick_count() > 0,
            "the enderman must tick; it has to sit in the player's chunk"
        );

        // A player hit aggros the enderman without the damage teleport (`getEntity() instanceof
        // LivingEntity` skips it in vanilla).
        let player_source = DamageSource::direct(
            &vanilla_damage_types::PLAYER_ATTACK,
            Arc::clone(&player_entity),
        );
        assert!(enderman.hurt(world, &player_source, 1.0));
        for _ in 0..4 {
            tick(world, &mut ticks);
        }
        assert!(
            enderman.last_hurt_by_mob().is_some() && enderman.last_hurt_by_mob_timestamp() > 0,
            "vanilla `LivingEntity.hurtServer` must record the attacker as last-hurt-by"
        );
        // GAP (reported): vanilla `HurtByTargetGoal` would apply the recorded attacker as the
        // target; it does not start in this fixture, so the hit's target is applied explicitly
        // through the same `set_target` call the goal makes in `start` (see the close-range test).
        let attacker: SharedEntity = Arc::clone(&player_entity);
        assert!(enderman.set_target(Some(&attacker)));
        assert!(
            enderman.target().is_some(),
            "a hit enderman must target its attacker before the damage teleport is tested"
        );

        // Unattributed damage rolls `random.nextInt(10) != 0` and then `teleport()`, whose
        // landing spot is within 32 blocks on each axis.
        let source = DamageSource::environment(&vanilla_damage_types::GENERIC);
        let mut teleported = false;
        for _ in 0..16 {
            let before = enderman.position();
            enderman.hurt(world, &source, 1.0);
            if enderman.position().distance(before) > 8.0 {
                teleported = true;
                break;
            }
        }

        assert!(
            teleported,
            "unattributed damage must teleport the enderman (vanilla 9/10 roll)"
        );
        let Some(target) = enderman.target() else {
            panic!("the teleport must not clear the enderman's target");
        };
        assert_eq!(target.uuid(), player.uuid());
    }
}
