//! Vanilla `ZombifiedPiglin` (`net.minecraft.world.entity.monster.zombie.ZombifiedPiglin`).
//!
//! Vanilla extends `Zombie`; Steel's `ZombieEntity` is a concrete type with no inheritance, so
//! the zombified piglin re-implements the pieces it inherits (baby support, door breaking,
//! zombie-style melee) and adds the vanilla anger/alert behaviour. Documented deviation: it does
//! not share `Zombie`'s reinforcement and drowning-conversion code, neither of which vanilla's
//! zombified piglin enables (`convertsInWater` is false, spawn reinforcements chance is 0).

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::ZombifiedPiglinEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_blocks, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier,
};

use crate::behavior::BLOCK_BEHAVIORS;
use crate::entity::ai::goal::{
    HurtByTargetGoal, NearestAttackableTargetGoal, ResetUniversalAngerTargetGoal,
    WaterAvoidingRandomStrollGoal, ZombieAttackGoal,
};
use crate::entity::ai::path::PathType;
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::zombie::zombie;
use crate::entity::neutral_mob::{NeutralMob, PersistentAnger};
use crate::entity::{
    AgeableMobGroupData, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::{LevelReader, World};

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Zombie.BABY_XP_MULTIPLIER`.
const BABY_XP_MULTIPLIER: f64 = 2.5;
/// Vanilla `Zombie.SPEED_MODIFIER_BABY_ID` (`minecraft:baby`); `ZombifiedPiglin`
/// inherits `Zombie.setBaby`.
const SPEED_MODIFIER_BABY_ID: Identifier = Identifier::vanilla_static("baby");
/// Vanilla `Zombie.SPEED_MODIFIER_BABY` amount.
const SPEED_MODIFIER_BABY_AMOUNT: f64 = 0.5;
/// Vanilla `Zombie.finalizeSpawn`: `0.55F * difficulty.getSpecialMultiplier()`.
const ZOMBIE_CAN_PICK_UP_LOOT_CHANCE: f32 = 0.55;
/// Vanilla `Zombie.getSpawnAsBabyOdds`: `random.nextFloat() < 0.05F`.
const ZOMBIE_SPAWN_AS_BABY_CHANCE: f32 = 0.05;
/// Vanilla `Zombie.BREAK_DOOR_CHANCE`.
const ZOMBIE_BREAK_DOOR_CHANCE: f32 = 0.1;
/// Vanilla `ZombifiedPiglin.populateDefaultEquipmentSlots`: `random.nextInt(20) == 0`.
const GOLDEN_SPEAR_WEIGHT_BOUND: i32 = 20;
/// Vanilla `ZombifiedPiglin.SPEED_MODIFIER_ATTACKING_ID`.
const SPEED_MODIFIER_ATTACKING_ID: Identifier = Identifier::vanilla_static("attacking");
/// Vanilla `ZombifiedPiglin.SPEED_MODIFIER_ATTACKING` amount.
const SPEED_MODIFIER_ATTACKING_AMOUNT: f64 = 0.05;
/// Vanilla `ZombifiedPiglin.ALERT_RANGE_Y`.
const ALERT_RANGE_Y: f64 = 10.0;
/// Vanilla alert interval (`TimeUtil.rangeOfSeconds(4, 6)`).
const ALERT_INTERVAL_MIN: i32 = 4 * 20;
const ALERT_INTERVAL_MAX: i32 = 6 * 20;
/// Vanilla `ZombifiedPiglin.PERSISTENT_ANGER_TIME` (`rangeOfSeconds(20, 39)`).
const PERSISTENT_ANGER_TIME_MIN: i64 = 20 * 20;
const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;
/// Vanilla first-anger-sound delay (`rangeOfSeconds(0, 1)`).
const FIRST_ANGER_SOUND_DELAY_MAX: i32 = 20;

const ZOMBIFIED_PIGLIN_BABY_WIDTH: f32 = 0.49;
const ZOMBIFIED_PIGLIN_BABY_HEIGHT: f32 = 0.98;
const ZOMBIFIED_PIGLIN_BABY_EYE_HEIGHT: f32 = 0.78;
const ZOMBIFIED_PIGLIN_BABY_VEHICLE_Y: f64 = 0.1875;

const ZOMBIFIED_PIGLIN_BABY_VEHICLE_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(
        0.0,
        ZOMBIFIED_PIGLIN_BABY_VEHICLE_Y,
        0.0,
    )];

const ZOMBIFIED_PIGLIN_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    ZOMBIFIED_PIGLIN_BABY_WIDTH,
    ZOMBIFIED_PIGLIN_BABY_HEIGHT,
    ZOMBIFIED_PIGLIN_BABY_EYE_HEIGHT,
    EntityAttachments::new(&[], &ZOMBIFIED_PIGLIN_BABY_VEHICLE_ATTACHMENTS, &[], &[]),
);

/// Vanilla `ZombifiedPiglin`.
#[entity_behavior(class = "ZombifiedPiglin")]
pub struct ZombifiedPiglinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<ZombifiedPiglinEntityData>,
    persistent_anger: PersistentAnger,
    /// Vanilla `ZombifiedPiglin.isBaby` state (the zombie's `IsBaby` tag).
    baby: SyncMutex<bool>,
    /// Vanilla `ZombifiedPiglin.canBreakDoors`.
    can_break_doors: SyncMutex<bool>,
    /// Vanilla `ZombifiedPiglin.playFirstAngerSoundIn`.
    play_first_anger_sound_in: SyncMutex<i32>,
    /// Vanilla `ZombifiedPiglin.ticksUntilNextAlert`.
    ticks_until_next_alert: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ZombifiedPiglinEntity`.
unsafe impl DowncastType for ZombifiedPiglinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/zombified_piglin");
}

impl ZombifiedPiglinEntity {
    /// Creates a new zombified piglin entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved zombified piglin entity.
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
        let mut entity_data = ZombifiedPiglinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        // Vanilla `ZombifiedPiglin` constructor: lava is traversable at a cost.
        mob_base.pathfinding_malus().lock().set(PathType::Lava, 8.0);

        {
            // Vanilla `ZombifiedPiglin.addBehaviourGoals` + `addTargetingGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(2, ZombieAttackGoal::new(1.0, false));
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new().set_alert_others(empty()));
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players_with_interval(
                    10,
                    true,
                    false,
                    |_, _| true,
                ),
            );
            target_selector.add_goal(3, ResetUniversalAngerTargetGoal::new(true));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            persistent_anger: PersistentAnger::new(),
            baby: SyncMutex::new(false),
            can_break_doors: SyncMutex::new(false),
            play_first_anger_sound_in: SyncMutex::new(0),
            ticks_until_next_alert: SyncMutex::new(0),
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

    /// Vanilla `Zombie.isBaby`.
    #[must_use]
    pub fn is_baby(&self) -> bool {
        *self.baby.lock()
    }

    /// Vanilla `Zombie.setBaby`.
    pub fn set_baby(&self, baby: bool) {
        *self.baby.lock() = baby;
        if self.level().is_some() {
            let mut attributes = self.attributes().lock();
            attributes.remove_modifier(vanilla_attributes::MOVEMENT_SPEED, &SPEED_MODIFIER_BABY_ID);
            if baby {
                attributes.add_modifier(
                    vanilla_attributes::MOVEMENT_SPEED,
                    AttributeModifier {
                        id: SPEED_MODIFIER_BABY_ID,
                        amount: SPEED_MODIFIER_BABY_AMOUNT,
                        operation: AttributeModifierOperation::AddMultipliedBase,
                    },
                    false,
                );
            }
        }
    }

    /// Vanilla `Zombie.canBreakDoors`.
    #[must_use]
    pub fn can_break_doors(&self) -> bool {
        *self.can_break_doors.lock()
    }

    /// Vanilla `Zombie.setCanBreakDoors`.
    pub fn set_can_break_doors(&self, can_break_doors: bool) {
        *self.can_break_doors.lock() = can_break_doors;
    }

    /// Vanilla `ZombifiedPiglin.playAngerSound`.
    fn play_anger_sound(&self) {
        self.play_sound(
            &sound_events::ENTITY_ZOMBIFIED_PIGLIN_ANGRY,
            self.sound_volume() * 2.0,
            1.8,
        );
    }

    /// Vanilla `ZombifiedPiglin.alertOthers`.
    fn alert_others(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let Some(target) = self.target() else {
            return;
        };
        let follow_range = self
            .attributes()
            .lock()
            .get_value(vanilla_attributes::FOLLOW_RANGE)
            .unwrap_or(0.0);
        let search_box = self
            .bounding_box()
            .inflate_xyz(follow_range, ALERT_RANGE_Y, follow_range);

        let others = world.get_entities_in_aabb(&search_box);
        for other in others {
            if other.id() == self.id() {
                continue;
            }
            let Some(other_mob) = other.as_mob() else {
                continue;
            };
            if other
                .as_entity_event_source()
                .downcast_ref::<ZombifiedPiglinEntity>()
                .is_none()
            {
                continue;
            }
            if other_mob.target().is_some() {
                continue;
            }
            if target
                .as_living_entity()
                .is_some_and(|target_living| other_mob.is_allied_to(target_living))
            {
                continue;
            }
            other_mob.set_target(Some(&target));
        }
    }

    /// Vanilla `ZombifiedPiglin.maybeAlertOthers`.
    fn maybe_alert_others(&self) {
        if self.target().is_none() {
            return;
        }
        let mut ticks = self.ticks_until_next_alert.lock();
        if *ticks > 0 {
            *ticks -= 1;
            return;
        }

        if let Some(target) = self.target()
            && self.has_line_of_sight_cached(target.as_ref())
        {
            self.alert_others();
        }
        *ticks =
            ALERT_INTERVAL_MIN + rand::random_range(0..(ALERT_INTERVAL_MAX - ALERT_INTERVAL_MIN));
    }

    /// Vanilla `ZombifiedPiglin.customServerAiStep`.
    fn custom_step_zombified_piglin(&self) {
        let angry = self.is_angry();
        {
            let mut attributes = self.attributes().lock();
            if angry {
                if !self.is_baby()
                    && !attributes.has_modifier(
                        vanilla_attributes::MOVEMENT_SPEED,
                        &SPEED_MODIFIER_ATTACKING_ID,
                    )
                {
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
            } else {
                attributes.remove_modifier(
                    vanilla_attributes::MOVEMENT_SPEED,
                    &SPEED_MODIFIER_ATTACKING_ID,
                );
            }
        }

        {
            let mut delay = self.play_first_anger_sound_in.lock();
            if *delay > 0 {
                *delay -= 1;
                if *delay == 0 {
                    drop(delay);
                    self.play_anger_sound();
                }
            }
        }

        self.update_persistent_anger(true);
        self.maybe_alert_others();
    }
}

impl NeutralMob for ZombifiedPiglinEntity {
    fn persistent_anger(&self) -> &PersistentAnger {
        &self.persistent_anger
    }

    /// Vanilla `ZombifiedPiglin.startPersistentAngerTimer`.
    fn start_persistent_anger_timer(&self) {
        self.set_time_to_remain_angry(
            PERSISTENT_ANGER_TIME_MIN
                + rand::random_range(0..(PERSISTENT_ANGER_TIME_MAX - PERSISTENT_ANGER_TIME_MIN)),
        );
    }
}

impl Entity for ZombifiedPiglinEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    /// Vanilla `ZombifiedPiglin.getDefaultDimensions`.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.is_baby() {
            ZOMBIFIED_PIGLIN_BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
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
        // Vanilla `ZombifiedPiglin` inherits `Zombie.playStepSound`, which plays no step sound
        // of its own (zombies use `ENTITY_ZOMBIE_STEP` only through the shared hostile step).
        self.play_sound(&sound_events::ENTITY_ZOMBIE_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("IsBaby", i8::from(self.is_baby()));
        nbt.insert("CanBreakDoors", i8::from(self.can_break_doors()));
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_baby(nbt.byte("IsBaby").is_some_and(|value| value != 0));
        self.set_can_break_doors(nbt.byte("CanBreakDoors").is_some_and(|value| value != 0));
        self.read_persistent_anger_save_data(&nbt);
    }
}

impl LivingEntity for ZombifiedPiglinEntity {
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
        Some(&sound_events::ENTITY_ZOMBIFIED_PIGLIN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ZOMBIFIED_PIGLIN_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    /// Vanilla `Zombie.getBaseExperienceReward`, which `ZombifiedPiglin`
    /// inherits: a baby's reward is its (mutated) `xpReward` scaled by 2.5.
    fn base_experience_reward(&self) -> i32 {
        if self.is_baby() {
            let scaled = (f64::from(self.xp_reward()) * BABY_XP_MULTIPLIER) as i32;
            self.set_xp_reward(scaled);
        }
        Mob::base_experience_reward_mob(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        self.custom_step_zombified_piglin();
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for ZombifiedPiglinEntity {
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
        Some(&sound_events::ENTITY_ZOMBIFIED_PIGLIN_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `ZombifiedPiglin.setTarget`: a new target restarts the anger sound and alert
    /// timers.
    fn set_target(&self, target: Option<&SharedEntity>) -> bool {
        let had_target = self.target().is_some();
        // Vanilla `ZombifiedPiglin.setTarget` calls `super.setTarget`; the `Mob` default
        // body is `mob_base().set_target(..)`, so call that directly — a
        // `Mob::set_target(self, ..)` UFCS call would re-enter this override.
        let changed = self
            .mob_base()
            .set_target(target, |target| Mob::is_valid_target(self, target));
        if !had_target && target.is_some() {
            *self.play_first_anger_sound_in.lock() =
                rand::random_range(0..FIRST_ANGER_SOUND_DELAY_MAX);
            *self.ticks_until_next_alert.lock() = ALERT_INTERVAL_MIN
                + rand::random_range(0..(ALERT_INTERVAL_MAX - ALERT_INTERVAL_MIN));
        }
        changed
    }

    /// Vanilla `ZombifiedPiglin.checkZombifiedPiglinSpawnRules`: never above nether wart blocks.
    fn check_mob_spawn_rules(
        entity_type: EntityTypeRef,
        level: &dyn LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        if level.get_block_state(pos.offset(0, -1, 0)).get_block()
            == &vanilla_blocks::NETHER_WART_BLOCK
        {
            return false;
        }
        // Inlined `Mob::check_mob_spawn_rules` default, which vanilla
        // `ZombifiedPiglin.checkZombifiedPiglinSpawnRules` reaches through
        // `checkMobSpawnRules`; Rust has no super-call, so the shared body is repeated
        // here. The non-peaceful gate comes from `Monster::check_monster_spawn_rules`.
        if spawn_reason.is_spawner() {
            return true;
        }
        let below = pos.below();
        let state = level.get_block_state(below);
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .is_valid_spawn(state, level, below, entity_type)
    }

    /// Vanilla `Zombie.finalizeSpawn`, which `ZombifiedPiglin` inherits: loot
    /// pickup, baby odds, door breaking, equipment, then `handleAttributes`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let mut random = LegacyRandom::from_seed(rand::random());
        let difficulty_modifier = world
            .current_difficulty_at(self.block_position())
            .special_multiplier();

        if spawn_reason != EntitySpawnReason::Conversion {
            self.set_can_pick_up_loot(
                random.next_f32() < ZOMBIE_CAN_PICK_UP_LOOT_CHANCE * difficulty_modifier,
            );
        }

        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::AgeableMob(AgeableMobGroupData::new(
                random.next_f32() < ZOMBIE_SPAWN_AS_BABY_CHANCE,
                AgeableMobGroupData::DEFAULT_BABY_SPAWN_CHANCE,
            )))
        });

        if let Some(SpawnGroupData::AgeableMob(data)) = group_data {
            if data.should_spawn_baby() {
                self.set_baby(true);
            }

            self.set_can_break_doors(
                random.next_f32() < difficulty_modifier * ZOMBIE_BREAK_DOOR_CHANCE,
            );
            if spawn_reason != EntitySpawnReason::Conversion {
                // Vanilla `ZombifiedPiglin.populateDefaultEquipmentSlots`.
                let item = if random.next_i32_bounded(GOLDEN_SPEAR_WEIGHT_BOUND) == 0 {
                    &vanilla_items::GOLDEN_SPEAR
                } else {
                    &vanilla_items::GOLDEN_SWORD
                };
                self.living_base()
                    .equipment()
                    .lock()
                    .set(EquipmentSlot::MainHand, ItemStack::new(item));
                zombie::populate_zombie_equipment_enchantments(
                    self,
                    &mut random,
                    difficulty_modifier,
                );
            }
        }

        // Vanilla `ZombifiedPiglin.randomizeReinforcementsChance` pins the base
        // chance to `0.0` instead of drawing a new one.
        zombie::handle_zombie_spawn_attributes(
            self,
            difficulty_modifier,
            spawn_reason,
            &mut random,
            &|can_break_doors| self.set_can_break_doors(can_break_doors),
            Some(0.0),
        );

        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }
}

impl Monster for ZombifiedPiglinEntity {}

impl PathfinderMob for ZombifiedPiglinEntity {}

#[cfg(test)]
mod tests {
    use simdnbt::owned::NbtCompound;
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::ZombifiedPiglinEntity;
    use crate::entity::Entity;

    #[test]
    fn zombified_piglin_baby_and_anger_state_round_trip() {
        init_vanilla_registry();
        let piglin = ZombifiedPiglinEntity::new(
            &vanilla_entities::ZOMBIFIED_PIGLIN,
            1,
            DVec3::ZERO,
            Weak::new(),
        );

        assert!(!piglin.is_baby());
        piglin.set_baby(true);
        piglin.set_can_break_doors(true);

        let mut nbt = NbtCompound::new();
        piglin.save_additional(&mut nbt);
        assert_eq!(nbt.byte("IsBaby"), Some(1));
        assert_eq!(nbt.byte("CanBreakDoors"), Some(1));
        assert_eq!(piglin.entity_type(), &vanilla_entities::ZOMBIFIED_PIGLIN);
    }
}
