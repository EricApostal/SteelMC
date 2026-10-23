//! Vanilla `Zombie` base entity
//! (`net.minecraft.world.entity.monster.zombie.Zombie`).

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
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::ZombieEntityData;
use steel_registry::{
    level_events, sound_events, vanilla_attributes, vanilla_entities, vanilla_game_rules,
    vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::Difficulty;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier,
};

use crate::entity::ai::goal::{
    FloatGoal, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::conversion::ConversionParams;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::npc::VillagerEntity;
use crate::entity::entities::mobs::npc::villager::VillagerDataHolder;
use crate::entity::entities::mobs::zombie::ZombieVillagerEntity;
use crate::entity::npc::trading::Merchant;
use crate::entity::registry::ENTITIES;
use crate::entity::{
    AgeableMobGroupData, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::World;

/// Vanilla `Zombie.SPEED_MODIFIER_BABY_ID` (`minecraft:baby`).
const SPEED_MODIFIER_BABY_ID: Identifier = Identifier::vanilla_static("baby");
const SPEED_MODIFIER_BABY_AMOUNT: f64 = 0.5;
/// Vanilla `Zombie.REINFORCEMENT_CALLER_CHARGE_ID`.
const REINFORCEMENT_CALLER_CHARGE_ID: Identifier =
    Identifier::vanilla_static("reinforcement_caller_charge");
/// Vanilla `Zombie.ZOMBIE_REINFORCEMENT_CALLEE_CHARGE`.
const ZOMBIE_REINFORCEMENT_CALLEE_CHARGE_ID: Identifier =
    Identifier::vanilla_static("reinforcement_callee_charge");
const ZOMBIE_REINFORCEMENT_CALLEE_CHARGE_AMOUNT: f64 = -0.05;
const REINFORCEMENT_CALLER_CHARGE_STEP: f64 = -0.05;
/// Vanilla `Zombie.LEADER_ZOMBIE_BONUS_ID`.
const LEADER_ZOMBIE_BONUS_ID: Identifier = Identifier::vanilla_static("leader_zombie_bonus");
/// Vanilla `Zombie.ZOMBIE_RANDOM_SPAWN_BONUS_ID`.
const ZOMBIE_RANDOM_SPAWN_BONUS_ID: Identifier =
    Identifier::vanilla_static("zombie_random_spawn_bonus");
/// Vanilla `Zombie.ZOMBIE_LEADER_CHANCE`.
const ZOMBIE_LEADER_CHANCE: f32 = 0.05;
/// Vanilla `Zombie.REINFORCEMENT_ATTEMPTS`.
const REINFORCEMENT_ATTEMPTS: i32 = 50;
/// Vanilla `Zombie.REINFORCEMENT_RANGE_MAX`.
const REINFORCEMENT_RANGE_MAX: i32 = 40;
/// Vanilla `Zombie.REINFORCEMENT_RANGE_MIN`.
const REINFORCEMENT_RANGE_MIN: i32 = 7;
/// Vanilla `Zombie.BREAK_DOOR_CHANCE`.
const BREAK_DOOR_CHANCE: f32 = 0.1;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
const BABY_XP_MULTIPLIER: f64 = 2.5;
/// Vanilla `ConversionTracker` totals for `Zombie`'s drowning conversion.
const DROWNING_AFFLICTION_TIME: i32 = 600;
const DROWNING_CONVERSION_TIME: i32 = 300;
const CAN_PICK_UP_LOOT_CHANCE: f32 = 0.55;
const SPAWN_AS_BABY_CHANCE: f32 = 0.05;
const JOCKEY_CHANCE: f32 = 0.05;
const ZOMBIE_ARMOR_CHANCE: f32 = 0.05;
const ZOMBIE_ARMOR_CHANCE_HARD: f32 = 0.01;
/// Vanilla `Drowned.populateDefaultEquipmentSlots`: `random.nextFloat() > 0.9`.
const DROWNED_EQUIPMENT_CHANCE: f32 = 0.9;
/// Vanilla `Drowned.populateDefaultEquipmentSlots`: `random.nextInt(16) < 10`.
const DROWNED_TRIDENT_WEIGHT: i32 = 10;
const KNOCKBACK_RESISTANCE_BONUS: f64 = 0.05;
const FOLLOW_RANGE_BONUS: f64 = 1.5;
const LEADER_REINFORCEMENT_BONUS: f64 = 0.25;
const LEADER_REINFORCEMENT_BONUS_BASE: f64 = 0.5;
const LEADER_HEALTH_BONUS: f64 = 3.0;
const LEADER_HEALTH_BONUS_BASE: f64 = 1.0;
const REINFORCEMENT_CLEAR_PLAYER_RADIUS_SQR: f64 = 49.0;
/// Vanilla `Zombie.doHurtTarget` ignition roll gain (`difficulty * 0.3F`).
const IGNITE_DIFFICULTY_GAIN: f32 = 0.3;
const TICKS_PER_SECOND: i32 = 20;

const ZOMBIE_BABY_WIDTH: f32 = 0.49;
const ZOMBIE_BABY_HEIGHT: f32 = 0.98;
const ZOMBIE_BABY_EYE_HEIGHT: f32 = 0.775;
/// Vanilla `Husk.BABY_DIMENSIONS` eye height.
const HUSK_BABY_EYE_HEIGHT: f32 = 0.825;
const ZOMBIE_BABY_VEHICLE_Y: f64 = 0.1875;

const ZOMBIE_BABY_VEHICLE_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, ZOMBIE_BABY_VEHICLE_Y, 0.0)];

const ZOMBIE_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    ZOMBIE_BABY_WIDTH,
    ZOMBIE_BABY_HEIGHT,
    ZOMBIE_BABY_EYE_HEIGHT,
    EntityAttachments::new(&[], &ZOMBIE_BABY_VEHICLE_ATTACHMENTS, &[], &[]),
);

/// Vanilla `Husk.BABY_DIMENSIONS` (identical to `Zombie`'s except eye height).
const HUSK_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    ZOMBIE_BABY_WIDTH,
    ZOMBIE_BABY_HEIGHT,
    HUSK_BABY_EYE_HEIGHT,
    EntityAttachments::new(&[], &ZOMBIE_BABY_VEHICLE_ATTACHMENTS, &[], &[]),
);

/// Drowning-conversion counters (vanilla `ConversionTracker`).
#[derive(Debug, Clone, Copy, Default)]
struct DrowningConversion {
    affliction_time: i32,
    conversion_time: i32,
}

/// Discriminates which vanilla subclass shares the `Zombie` body.
///
/// Vanilla's `Husk` and `Drowned` extend `Zombie` and override a handful of
/// protected methods; Rust has no inheritance, so the shared body dispatches on
/// this value instead of each subclass duplicating the `Zombie` logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ZombieVariant {
    Zombie,
    Husk,
    Drowned,
}

/// Vanilla `Zombie`.
#[entity_behavior(class = "Zombie")]
pub struct ZombieEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<ZombieEntityData>,
    can_break_doors: SyncMutex<bool>,
    drowning: SyncMutex<DrowningConversion>,
    variant: ZombieVariant,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ZombieEntity`.
unsafe impl DowncastType for ZombieEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/zombie");
}

impl ZombieEntity {
    /// Creates a new zombie at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_variant(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            ZombieVariant::Zombie,
        )
    }

    /// Reconstructs a zombie from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_variant(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            ZombieVariant::Zombie,
        )
    }

    /// Builds the shared `Zombie` body for a concrete vanilla subclass.
    ///
    /// `HuskEntity` and `DrownedEntity` own one of these and delegate to it;
    /// `variant` supplies the overridden behaviour that vanilla gets from Java
    /// inheritance.
    pub(crate) fn new_with_variant(
        base: EntityBase,
        entity_type: EntityTypeRef,
        variant: ZombieVariant,
    ) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        mob_base.set_xp_reward(XP_REWARD);
        let mut entity_data = ZombieEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        // Vanilla `Zombie.registerGoals`: the turtle-egg goal (priority 4) has no
        // Steel `RemoveBlockGoal` counterpart yet.
        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(2, MeleeAttackGoal::new(1.0, false));
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
        }

        // Vanilla `Zombie.addBehaviourGoals` target selector. `MoveThroughVillageGoal`
        // (goal 6) has no Steel counterpart yet; `ZombieAttackGoal` is served by the
        // shared `MeleeAttackGoal`, which already drives the aggressive flag.
        {
            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(
                1,
                HurtByTargetGoal::new().set_alert_others(std::iter::empty()),
            );
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
            );
            target_selector.add_goal(
                3,
                NearestAttackableTargetGoal::new(false, |entity, _| {
                    entity.entity_type() == &vanilla_entities::VILLAGER
                        || entity.entity_type() == &vanilla_entities::WANDERING_TRADER
                }),
            );
            target_selector.add_goal(
                3,
                NearestAttackableTargetGoal::new(true, |entity, _| {
                    entity.entity_type() == &vanilla_entities::IRON_GOLEM
                }),
            );
            target_selector.add_goal(
                5,
                NearestAttackableTargetGoal::new_with_interval(10, true, false, |entity, _| {
                    entity.entity_type() == &vanilla_entities::TURTLE
                        && entity.is_baby()
                        && entity.on_ground()
                }),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            can_break_doors: SyncMutex::new(false),
            drowning: SyncMutex::new(DrowningConversion::default()),
            variant,
        }
    }

    /// Which vanilla subclass this body behaves as.
    pub(crate) fn variant(&self) -> ZombieVariant {
        self.variant
    }

    /// Vanilla `Zombie.isBaby`.
    pub fn is_baby(&self) -> bool {
        *self.entity_data.lock().baby.get()
    }

    /// Vanilla `Zombie.setBaby`, including the transient baby speed modifier.
    pub fn set_baby(&self, baby: bool) {
        self.entity_data.lock().baby.set(baby);
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
    pub fn can_break_doors(&self) -> bool {
        *self.can_break_doors.lock()
    }

    /// Vanilla `Zombie.setCanBreakDoors`.
    ///
    /// Steel has no `BreakDoorGoal`, so only the navigation door flag is toggled.
    pub fn set_can_break_doors(&self, can_break_doors: bool) {
        let mut navigation = self.mob_base().navigation().lock();
        navigation.set_can_open_doors(can_break_doors);

        let mut current = self.can_break_doors.lock();
        if *current == can_break_doors {
            return;
        }

        *current = can_break_doors;
        navigation.set_can_open_doors(can_break_doors);
    }

    /// Vanilla `Zombie.isUnderWaterConverting`.
    pub fn is_under_water_converting(&self) -> bool {
        *self.entity_data.lock().drowned_conversion.get()
    }

    /// Vanilla `Zombie.convertsInWater`; `ZombieVillager`/`Drowned` return `false`.
    pub fn converts_in_water(&self) -> bool {
        self.variant != ZombieVariant::Drowned
    }

    /// Vanilla `Zombie.getSpawnAsBabyOdds`.
    #[must_use]
    pub fn get_spawn_as_baby_odds(random: &mut impl steel_utils::random::Random) -> bool {
        random.next_f32() < SPAWN_AS_BABY_CHANCE
    }

    /// Vanilla `Zombie.getConversionSound`; `Husk` returns 1041.
    pub fn conversion_sound(&self) -> i32 {
        match self.variant {
            ZombieVariant::Husk => level_events::SOUND_HUSK_TO_ZOMBIE,
            ZombieVariant::Zombie | ZombieVariant::Drowned => level_events::SOUND_ZOMBIE_TO_DROWNED,
        }
    }

    /// Vanilla `Zombie.convertsToWhenDrowning`; `Husk` converts back to `Zombie`.
    pub fn converts_to_when_drowning(&self) -> EntityTypeRef {
        match self.variant {
            ZombieVariant::Husk => &vanilla_entities::ZOMBIE,
            ZombieVariant::Zombie | ZombieVariant::Drowned => &vanilla_entities::DROWNED,
        }
    }

    /// Vanilla `Zombie.setInWaterTime` (test hook).
    pub fn set_in_water_time(&self, in_water_time: i32) {
        self.drowning.lock().affliction_time = in_water_time;
    }

    /// Vanilla `Zombie.setConversionTime` (test hook).
    pub fn set_conversion_time(&self, conversion_time: i32) {
        self.start_drowning_conversion(conversion_time);
    }

    /// The current drowning conversion timer.
    pub fn conversion_time(&self) -> i32 {
        self.drowning.lock().conversion_time
    }

    /// Vanilla `Zombie.canSpawnInLiquids`; `Drowned` returns `true`.
    pub fn can_spawn_in_liquids(&self) -> bool {
        self.variant == ZombieVariant::Drowned
    }

    /// Current zombie special-type data (`DATA_SPECIAL_TYPE_ID`).
    pub fn special_type(&self) -> i32 {
        *self.entity_data.lock().special_type.get()
    }

    /// Sets the zombie special-type data (`DATA_SPECIAL_TYPE_ID`).
    pub fn set_special_type(&self, special_type: i32) {
        self.entity_data.lock().special_type.set(special_type);
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

    /// Vanilla `Zombie.tick` tail: advances the drowning conversion tracker.
    fn tick_drowning_conversion(&self) {
        if !self.converts_in_water() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        if !LivingEntity::is_alive(self) || self.is_no_ai() {
            return;
        }

        if self.is_eye_in_water() {
            if self.is_under_water_converting() {
                let finished = {
                    let mut drowning = self.drowning.lock();
                    drowning.conversion_time -= 1;
                    drowning.conversion_time < 0
                };
                if finished {
                    self.convert_to_drowned(&world);
                }
            } else {
                let start = {
                    let mut drowning = self.drowning.lock();
                    drowning.affliction_time += 1;
                    drowning.affliction_time >= DROWNING_AFFLICTION_TIME
                };
                if start {
                    self.start_drowning_conversion(DROWNING_CONVERSION_TIME);
                }
            }
        } else {
            self.drowning.lock().affliction_time = -1;
            self.entity_data.lock().drowned_conversion.set(false);
        }
    }

    fn start_drowning_conversion(&self, time: i32) {
        self.drowning.lock().conversion_time = time;
        self.entity_data.lock().drowned_conversion.set(true);
    }

    fn convert_to_drowned(&self, world: &Arc<World>) {
        let target = self.converts_to_when_drowning();
        let sound = self.conversion_sound();
        let converted = self.convert_to(
            target,
            ConversionParams::single(true, true),
            Some(&move |mob: &dyn Mob| {
                if let Some(zombie) = mob.downcast_ref::<ZombieEntity>() {
                    let mut random = LegacyRandom::from_seed(rand::random());
                    zombie.handle_attributes(1.0, EntitySpawnReason::Conversion, &mut random);
                }
                if !mob.is_silent() {
                    world.level_event(sound, mob.block_position(), 0, None);
                }
            }),
        );
        if converted.is_none() {
            // Vanilla always has `DROWNED` registered; Steel's drowned factory is a
            // separate slice, so the conversion may be rejected until it lands.
            log::warn!("zombie could not convert to drowned");
        }
    }

    /// Vanilla `Zombie.convertVillagerToZombieVillager`.
    pub fn convert_villager_to_zombie_villager(
        &self,
        world: &Arc<World>,
        villager: &VillagerEntity,
    ) -> bool {
        let silent = self.is_silent();
        let event_pos = self.block_position();
        let converted = villager.convert_to(
            &vanilla_entities::ZOMBIE_VILLAGER,
            ConversionParams::single(true, true),
            Some(&move |mob: &dyn Mob| {
                let Some(zombie) = mob.downcast_ref::<ZombieVillagerEntity>() else {
                    log::error!("villager conversion produced a non zombie villager entity");
                    return;
                };
                zombie.set_villager_data_finalized(VillagerDataHolder::villager_data_finalized(
                    villager,
                ));
                mob.finalize_spawn(
                    world,
                    EntitySpawnReason::Conversion,
                    Some(SpawnGroupData::AgeableMob(AgeableMobGroupData::new(
                        false,
                        AgeableMobGroupData::DEFAULT_BABY_SPAWN_CHANCE,
                    ))),
                );
                zombie.set_villager_data(VillagerDataHolder::villager_data(villager));
                zombie.set_gossips(villager.gossips_copy());
                zombie.set_trade_offers(Merchant::offers(villager).copy());
                zombie.set_villager_xp(villager.villager_xp());
                if !silent {
                    world.level_event(level_events::SOUND_ZOMBIE_INFECTED, event_pos, 0, None);
                }
            }),
        );
        converted.is_some()
    }

    /// Vanilla `Zombie.handleAttributes`.
    fn handle_attributes(
        &self,
        difficulty_modifier: f32,
        spawn_reason: EntitySpawnReason,
        random: &mut impl steel_utils::random::Random,
    ) {
        self.randomize_reinforcements_chance(random);
        {
            let mut attributes = self.attributes().lock();
            attributes.set_modifier(
                vanilla_attributes::KNOCKBACK_RESISTANCE,
                AttributeModifier {
                    id: ZOMBIE_RANDOM_SPAWN_BONUS_ID,
                    amount: random.next_f64() * KNOCKBACK_RESISTANCE_BONUS,
                    operation: AttributeModifierOperation::AddValue,
                },
                true,
            );

            let follow_range_modifier =
                random.next_f64() * FOLLOW_RANGE_BONUS * f64::from(difficulty_modifier);
            if follow_range_modifier > 1.0 {
                attributes.set_modifier(
                    vanilla_attributes::FOLLOW_RANGE,
                    AttributeModifier {
                        id: ZOMBIE_RANDOM_SPAWN_BONUS_ID,
                        amount: follow_range_modifier,
                        operation: AttributeModifierOperation::AddMultipliedTotal,
                    },
                    true,
                );
            }
        }

        if random.next_f32() < difficulty_modifier * ZOMBIE_LEADER_CHANCE {
            {
                let mut attributes = self.attributes().lock();
                attributes.set_modifier(
                    vanilla_attributes::SPAWN_REINFORCEMENTS,
                    AttributeModifier {
                        id: LEADER_ZOMBIE_BONUS_ID,
                        amount: random.next_f64() * LEADER_REINFORCEMENT_BONUS
                            + LEADER_REINFORCEMENT_BONUS_BASE,
                        operation: AttributeModifierOperation::AddValue,
                    },
                    true,
                );
                attributes.set_modifier(
                    vanilla_attributes::MAX_HEALTH,
                    AttributeModifier {
                        id: LEADER_ZOMBIE_BONUS_ID,
                        amount: random.next_f64() * LEADER_HEALTH_BONUS + LEADER_HEALTH_BONUS_BASE,
                        operation: AttributeModifierOperation::AddMultipliedTotal,
                    },
                    true,
                );
            }

            if !matches!(
                spawn_reason,
                EntitySpawnReason::Conversion
                    | EntitySpawnReason::Load
                    | EntitySpawnReason::DimensionTravel
            ) {
                self.set_health(self.get_max_health());
            }

            self.set_can_break_doors(true);
        }
    }

    fn randomize_reinforcements_chance(&self, random: &mut impl steel_utils::random::Random) {
        let amount = random.next_f64() * 0.1;
        self.attributes()
            .lock()
            .set_base_value(vanilla_attributes::SPAWN_REINFORCEMENTS, amount);
    }

    /// Checked when damage lands; mirrors the vanilla `hurtServer` re-target and
    /// reinforcement roll. The vanilla source entity is not observable through
    /// Steel's `before_actually_hurt` hook, so re-targeting to the attacker is
    /// omitted; reinforcements defend the current target.
    /// Reinforcements re-target and roll (vanilla `Zombie.hurtServer`).
    pub(crate) fn on_about_to_be_hurt(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if world.difficulty() != Difficulty::Hard {
            return;
        }
        if !world.get_game_rule(&vanilla_game_rules::SPAWN_MOBS) {
            return;
        }
        let chance = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::SPAWN_REINFORCEMENTS);
        let mut random = LegacyRandom::from_seed(rand::random());
        if f64::from(random.next_f32()) >= chance {
            return;
        }
        let Some(target) = self.target() else {
            return;
        };
        let entity_type = self.entity_type();
        let origin = self.block_position();
        for _ in 0..REINFORCEMENT_ATTEMPTS {
            let x = origin.x()
                + random.next_i32_between(REINFORCEMENT_RANGE_MIN, REINFORCEMENT_RANGE_MAX)
                    * random.next_i32_between(-1, 1);
            let y = origin.y()
                + random.next_i32_between(REINFORCEMENT_RANGE_MIN, REINFORCEMENT_RANGE_MAX)
                    * random.next_i32_between(-1, 1);
            let z = origin.z()
                + random.next_i32_between(REINFORCEMENT_RANGE_MIN, REINFORCEMENT_RANGE_MAX)
                    * random.next_i32_between(-1, 1);
            let spawn_pos = BlockPos::new(x, y, z);

            if !<ZombieEntity as Monster>::check_monster_spawn_rules(
                entity_type,
                &world,
                EntitySpawnReason::Reinforcement,
                spawn_pos,
                &mut random,
            ) {
                continue;
            }
            if !self.can_spawn_in_liquids()
                && (world.get_block_state(spawn_pos).has_fluid()
                    || world.get_block_state(spawn_pos.above()).has_fluid())
            {
                continue;
            }
            if world
                .nearest_player_distance_sqr(DVec3::new(
                    f64::from(x) + 0.5,
                    f64::from(y),
                    f64::from(z) + 0.5,
                ))
                .is_some_and(|distance| distance < REINFORCEMENT_CLEAR_PLAYER_RADIUS_SQR)
            {
                continue;
            }

            let Some(reinforcement) = ENTITIES.create(
                entity_type,
                next_entity_id(),
                DVec3::new(f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5),
                Arc::downgrade(&world),
            ) else {
                return;
            };
            let Some(reinforcement_mob) = reinforcement.as_mob() else {
                return;
            };
            if !reinforcement_mob.check_spawn_obstruction(&world) {
                continue;
            }
            if world.try_add_entity(Arc::clone(&reinforcement)).is_err() {
                continue;
            }

            reinforcement_mob.set_target(Some(&target));
            reinforcement_mob.finalize_spawn(&world, EntitySpawnReason::Reinforcement, None);

            let caller_charge = self
                .attributes()
                .lock()
                .required_value(vanilla_attributes::SPAWN_REINFORCEMENTS)
                + REINFORCEMENT_CALLER_CHARGE_STEP;
            self.attributes().lock().set_modifier(
                vanilla_attributes::SPAWN_REINFORCEMENTS,
                AttributeModifier {
                    id: REINFORCEMENT_CALLER_CHARGE_ID,
                    amount: caller_charge,
                    operation: AttributeModifierOperation::AddValue,
                },
                true,
            );
            reinforcement_mob.attributes().lock().add_modifier(
                vanilla_attributes::SPAWN_REINFORCEMENTS,
                AttributeModifier {
                    id: ZOMBIE_REINFORCEMENT_CALLEE_CHARGE_ID,
                    amount: ZOMBIE_REINFORCEMENT_CALLEE_CHARGE_AMOUNT,
                    operation: AttributeModifierOperation::AddValue,
                },
                true,
            );
            break;
        }
    }

    /// Vanilla `Zombie.getStepSound`; `Husk`/`Drowned` override it.
    pub fn step_sound(&self) -> SoundEventRef {
        match self.variant {
            ZombieVariant::Husk => &sound_events::ENTITY_HUSK_STEP,
            ZombieVariant::Drowned => &sound_events::ENTITY_DROWNED_STEP,
            ZombieVariant::Zombie => &sound_events::ENTITY_ZOMBIE_STEP,
        }
    }

    fn populate_default_equipment_slots(
        &self,
        random: &mut impl steel_utils::random::Random,
        difficulty_modifier: f32,
    ) {
        if self.variant == ZombieVariant::Drowned {
            // Vanilla `Drowned.populateDefaultEquipmentSlots`, which does not call
            // `Zombie`'s table.
            if random.next_f32() > DROWNED_EQUIPMENT_CHANCE {
                let item = if random.next_i32_bounded(16) < DROWNED_TRIDENT_WEIGHT {
                    &vanilla_items::TRIDENT
                } else {
                    &vanilla_items::FISHING_ROD
                };
                self.living_base()
                    .equipment()
                    .lock()
                    .set(EquipmentSlot::MainHand, ItemStack::new(item));
            }
            return;
        }

        let chance = if difficulty_modifier >= 1.0 {
            ZOMBIE_ARMOR_CHANCE_HARD
        } else {
            ZOMBIE_ARMOR_CHANCE
        };
        if random.next_f32() >= chance {
            return;
        }

        let item = match random.next_i32_bounded(6) {
            0 => &vanilla_items::IRON_SWORD,
            1 => &vanilla_items::IRON_SPEAR,
            _ => &vanilla_items::IRON_SHOVEL,
        };
        self.living_base()
            .equipment()
            .lock()
            .set(EquipmentSlot::MainHand, ItemStack::new(item));
    }

    /// Vanilla `Mob.dropPreservedEquipment(level, shouldDrop)`.
    ///
    /// Returns the slots whose stacks the predicate keeps (vanilla's
    /// `slotsPreventedFromDropping`) and drops the preserved ones that are not kept.
    pub fn drop_preserved_equipment(
        &self,
        keep: impl Fn(&ItemStack) -> bool,
    ) -> Vec<EquipmentSlot> {
        let mut kept = Vec::new();
        for slot in EquipmentSlot::ALL {
            let stack = self.equipment_in_slot(slot);
            if stack.is_empty() {
                continue;
            }
            if keep(&stack) {
                kept.push(slot);
            } else if self.is_equipment_drop_preserved(slot) {
                let _ = self.spawn_at_location(stack, 0.0);
            }
        }
        kept
    }

    /// Vanilla baby-zombie chicken jockey roll (`Zombie.finalizeSpawn`).
    fn try_spawn_jockey(&self, world: &Arc<World>, random: &mut impl steel_utils::random::Random) {
        if random.next_f32() < JOCKEY_CHANCE {
            let nearby = world.nearest_entity_in_aabb_matching(
                &self.bounding_box().inflate_xyz(5.0, 3.0, 5.0),
                self.position(),
                |entity| {
                    entity.entity_type() == &vanilla_entities::CHICKEN && entity.vehicle().is_none()
                },
            );
            if let Some(chicken) = nearby {
                self.start_riding(&chicken);
            }
            return;
        }

        if random.next_f32() < JOCKEY_CHANCE {
            let Some(chicken) = ENTITIES.create(
                &vanilla_entities::CHICKEN,
                next_entity_id(),
                self.position(),
                Arc::downgrade(world),
            ) else {
                return;
            };
            let Some(chicken_mob) = chicken.as_mob() else {
                return;
            };
            chicken_mob.snap_to(self.position(), self.rotation().0, 0.0);
            chicken_mob.finalize_spawn(world, EntitySpawnReason::Jockey, None);
            self.start_riding(&chicken);
            if let Err(error) = world.try_add_entity(Arc::clone(&chicken)) {
                log::warn!("failed to add zombie jockey chicken: {error}");
            }
        }
    }
}

impl Entity for ZombieEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        self.tick_drowning_conversion();
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        if self.is_baby() {
            // Vanilla `Husk.BABY_DIMENSIONS` differs from `Zombie`'s eye height.
            return if self.variant == ZombieVariant::Husk {
                HUSK_BABY_DIMENSIONS
            } else {
                ZOMBIE_BABY_DIMENSIONS
            };
        }
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
        if self.variant == ZombieVariant::Drowned {
            return &sound_events::ENTITY_DROWNED_SWIM;
        }
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(self.step_sound(), 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("IsBaby", i8::from(self.is_baby()));
        nbt.insert("CanBreakDoors", i8::from(self.can_break_doors()));

        let drowning = *self.drowning.lock();
        nbt.insert(
            "DrownedConversionTime",
            if self.is_under_water_converting() {
                drowning.conversion_time
            } else {
                -1
            },
        );
        nbt.insert(
            "InWaterTime",
            if self.is_eye_in_water() {
                drowning.affliction_time
            } else {
                -1
            },
        );
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_baby(nbt.byte("IsBaby").is_some_and(|value| value != 0));
        self.set_can_break_doors(nbt.byte("CanBreakDoors").is_some_and(|value| value != 0));

        self.drowning.lock().affliction_time = nbt.int("InWaterTime").unwrap_or(0);
        let conversion_time = nbt.int("DrownedConversionTime").unwrap_or(-1);
        if conversion_time != -1 {
            self.start_drowning_conversion(conversion_time);
        } else {
            self.entity_data.lock().drowned_conversion.set(false);
        }
    }

    fn killed_entity(
        &self,
        world: &World,
        entity: &dyn LivingEntity,
        _source: &DamageSource,
    ) -> bool {
        let difficulty = world.difficulty();
        if !matches!(difficulty, Difficulty::Normal | Difficulty::Hard) {
            return true;
        }
        let Some(villager) = entity.downcast_ref::<VillagerEntity>() else {
            return true;
        };
        if difficulty != Difficulty::Hard && rand::random::<bool>() {
            return true;
        }

        let Some(world) = self.level() else {
            return true;
        };
        if self.convert_villager_to_zombie_villager(&world, villager) {
            return false;
        }

        true
    }
}

impl LivingEntity for ZombieEntity {
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
        // Vanilla `Drowned.getHurtSound` switches on being in water.
        Some(match self.variant {
            ZombieVariant::Husk => &sound_events::ENTITY_HUSK_HURT,
            ZombieVariant::Drowned => {
                if self.is_in_water() {
                    &sound_events::ENTITY_DROWNED_HURT_WATER
                } else {
                    &sound_events::ENTITY_DROWNED_HURT
                }
            }
            ZombieVariant::Zombie => &sound_events::ENTITY_ZOMBIE_HURT,
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(match self.variant {
            ZombieVariant::Husk => &sound_events::ENTITY_HUSK_DEATH,
            ZombieVariant::Drowned => {
                if self.is_in_water() {
                    &sound_events::ENTITY_DROWNED_DEATH_WATER
                } else {
                    &sound_events::ENTITY_DROWNED_DEATH
                }
            }
            ZombieVariant::Zombie => &sound_events::ENTITY_ZOMBIE_DEATH,
        })
    }

    fn before_actually_hurt(&self, _source: &DamageSource, _amount: f32) {
        self.on_about_to_be_hurt();
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn base_experience_reward(&self) -> i32 {
        if self.is_baby() {
            let scaled = (f64::from(self.xp_reward()) * BABY_XP_MULTIPLIER) as i32;
            self.set_xp_reward(scaled);
        }
        Mob::base_experience_reward_mob(self)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for ZombieEntity {
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
        Some(match self.variant {
            ZombieVariant::Husk => &sound_events::ENTITY_HUSK_AMBIENT,
            ZombieVariant::Drowned => {
                if self.is_in_water() {
                    &sound_events::ENTITY_DROWNED_AMBIENT_WATER
                } else {
                    &sound_events::ENTITY_DROWNED_AMBIENT
                }
            }
            ZombieVariant::Zombie => &sound_events::ENTITY_ZOMBIE_AMBIENT,
        })
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        !(item_stack.is(&vanilla_items::EGG) && self.is_baby() && self.vehicle().is_some())
    }

    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        !item_stack.is(&vanilla_items::GLOW_INK_SAC) && self.can_hold_item(item_stack)
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        true
    }

    /// Vanilla `Zombie.doHurtTarget` tail: a burning, empty-handed zombie sets its
    /// target on fire after a landed hit.
    fn after_hurt_target(&self, _world: &World, target: &SharedEntity, was_hurt: bool) {
        if !was_hurt || !self.is_on_fire() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let main_hand_empty = self
            .living_base()
            .equipment()
            .lock()
            .get_ref(EquipmentSlot::MainHand)
            .is_empty();
        if !main_hand_empty {
            return;
        }

        let effective_difficulty = zombie_effective_difficulty(&world);
        let mut random = LegacyRandom::from_seed(rand::random());
        if random.next_f32() < effective_difficulty * IGNITE_DIFFICULTY_GAIN {
            target.ignite_for_ticks(2 * (effective_difficulty as i32) * TICKS_PER_SECOND);
        }
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let mut random = LegacyRandom::from_seed(rand::random());
        let difficulty_modifier = zombie_special_multiplier(world);

        let mut group_data = self.finalize_spawn_mob_base(world, spawn_reason, group_data);

        if spawn_reason != EntitySpawnReason::Conversion {
            self.set_can_pick_up_loot(
                random.next_f32() < CAN_PICK_UP_LOOT_CHANCE * difficulty_modifier,
            );
        }

        if group_data.is_none() {
            // Steel's `SpawnGroupData` has no zombie variant; vanilla's
            // `ZombieGroupData.canSpawnJockey` is always `true`, so only the baby
            // flag needs carrying.
            group_data = Some(SpawnGroupData::AgeableMob(AgeableMobGroupData::new(
                Self::get_spawn_as_baby_odds(&mut random),
                AgeableMobGroupData::DEFAULT_BABY_SPAWN_CHANCE,
            )));
        }

        if let Some(SpawnGroupData::AgeableMob(data)) = group_data {
            if data.should_spawn_baby() {
                self.set_baby(true);
                self.try_spawn_jockey(world, &mut random);
            }

            self.set_can_break_doors(random.next_f32() < difficulty_modifier * BREAK_DOOR_CHANCE);
            if spawn_reason != EntitySpawnReason::Conversion {
                self.populate_default_equipment_slots(&mut random, difficulty_modifier);
            }
        }

        // Steel has no `SpecialDates` calendar, so vanilla's Halloween pumpkin-head
        // roll is not applied.

        self.handle_attributes(difficulty_modifier, spawn_reason, &mut random);
        group_data
    }
}

impl Monster for ZombieEntity {}

impl PathfinderMob for ZombieEntity {}

/// Vanilla `DifficultyInstance.getEffectiveDifficulty` for the world's current state.
///
/// Steel has no `DifficultyInstance` (chunk inhabited time, moon brightness), so
/// only the base difficulty and global game time feed the vanilla formula.
pub(crate) fn zombie_effective_difficulty(world: &World) -> f32 {
    let difficulty = world.difficulty();
    if difficulty == Difficulty::Peaceful {
        return 0.0;
    }

    let total_game_time = world.game_time() as f32;
    let is_hard = difficulty == Difficulty::Hard;
    let global_scale = ((total_game_time - 72_000.0) / 1_440_000.0).clamp(0.0, 1.0) * 0.25;
    let scale = 0.75 + global_scale;
    let local_scale = if is_hard { 1.0 } else { 0.75 };
    f32::from(difficulty as u8) * (scale + local_scale)
}

/// Vanilla `DifficultyInstance.getSpecialMultiplier`.
fn zombie_special_multiplier(world: &World) -> f32 {
    let effective = zombie_effective_difficulty(world);
    if effective < 2.0 {
        0.0
    } else if effective > 4.0 {
        1.0
    } else {
        (effective - 2.0) / 2.0
    }
}
