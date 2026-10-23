//! Vanilla `ZombieVillager`
//! (`net.minecraft.world.entity.monster.zombie.ZombieVillager`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::entity_data::VillagerData;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::ZombieVillagerEntityData;
use steel_registry::villager_type::VillagerType;
use steel_registry::{
    REGISTRY, RegistryExt as _, level_events, sound_events, vanilla_attributes, vanilla_entities,
    vanilla_items, vanilla_mob_effects,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::types::{Difficulty, InteractionHand};
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, Identifier, UuidExt as _,
};
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    FloatGoal, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::conversion::ConversionParams;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::npc::VillagerEntity;
use crate::entity::entities::mobs::npc::villager::VillagerDataHolder;
use crate::entity::entities::mobs::zombie::zombie::{self, zombie_effective_difficulty};
use crate::entity::npc::gossip::{GossipContainer, ReputationEventHandler, ReputationEventType};
use crate::entity::npc::trading::{Merchant, MerchantOffers};
use crate::entity::{
    AgeableMobGroupData, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `ZombieVillager.VILLAGER_CONVERSION_WAIT_MIN`.
const VILLAGER_CONVERSION_WAIT_MIN: i32 = 3600;
/// Vanilla `ZombieVillager.VILLAGER_CONVERSION_WAIT_MAX`.
const VILLAGER_CONVERSION_WAIT_MAX: i32 = 6000;
/// Vanilla `ZombieVillager.MAX_SPECIAL_BLOCKS_COUNT`.
const MAX_SPECIAL_BLOCKS_COUNT: i32 = 14;
/// Vanilla `ZombieVillager.SPECIAL_BLOCK_RADIUS`.
const SPECIAL_BLOCK_RADIUS: i32 = 4;
const SPECIAL_BLOCK_CHANCE: f32 = 0.01;
const SPECIAL_BLOCK_ADD_CHANCE: f32 = 0.3;
const NOT_CONVERTING: i32 = -1;
const DEFAULT_XP: i32 = 0;
/// Vanilla `ZombieVillager` uses `Zombie.createAttributes()`.
const XP_REWARD: i32 = 5;
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const CURE_NAUSEA_TICKS: i32 = 200;
const BABY_XP_MULTIPLIER: f64 = 2.5;
/// Vanilla `Zombie.doHurtTarget` ignition roll gain (`difficulty * 0.3F`).
const IGNITE_DIFFICULTY_GAIN: f32 = 0.3;
/// Vanilla `Zombie.finalizeSpawn`: `0.55F * difficulty.getSpecialMultiplier()`.
const ZOMBIE_CAN_PICK_UP_LOOT_CHANCE: f32 = 0.55;
/// Vanilla `Zombie.getSpawnAsBabyOdds`: `random.nextFloat() < 0.05F`.
const ZOMBIE_SPAWN_AS_BABY_CHANCE: f32 = 0.05;
/// Vanilla `Zombie.BREAK_DOOR_CHANCE`.
const ZOMBIE_BREAK_DOOR_CHANCE: f32 = 0.1;
const TICKS_PER_SECOND: i32 = 20;
const SPEED_MODIFIER_BABY_ID: Identifier = Identifier::vanilla_static("baby");

const ZOMBIE_VILLAGER_BABY_WIDTH: f32 = 0.49;
const ZOMBIE_VILLAGER_BABY_HEIGHT: f32 = 0.98;
const ZOMBIE_VILLAGER_BABY_EYE_HEIGHT: f32 = 0.67;
const ZOMBIE_VILLAGER_BABY_VEHICLE_Y: f64 = 0.125;

const ZOMBIE_VILLAGER_BABY_VEHICLE_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(
        0.0,
        ZOMBIE_VILLAGER_BABY_VEHICLE_Y,
        0.0,
    )];

const ZOMBIE_VILLAGER_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    ZOMBIE_VILLAGER_BABY_WIDTH,
    ZOMBIE_VILLAGER_BABY_HEIGHT,
    ZOMBIE_VILLAGER_BABY_EYE_HEIGHT,
    EntityAttachments::new(&[], &ZOMBIE_VILLAGER_BABY_VEHICLE_ATTACHMENTS, &[], &[]),
);

/// Vanilla `ZombieVillager`.
#[entity_behavior(class = "ZombieVillager")]
pub struct ZombieVillagerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<ZombieVillagerEntityData>,
    can_break_doors: SyncMutex<bool>,
    villager_conversion_time: SyncMutex<i32>,
    conversion_starter: SyncMutex<Option<Uuid>>,
    gossips: SyncMutex<Option<GossipContainer>>,
    trade_offers: SyncMutex<Option<MerchantOffers>>,
    villager_xp: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ZombieVillagerEntity`.
unsafe impl DowncastType for ZombieVillagerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/zombie_villager");
}

impl ZombieVillagerEntity {
    /// Creates a new zombie villager at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a zombie villager from persisted base entity state.
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
        // The generated constructor already applies vanilla
        // `initializeZombieVillagerData` (plains type, random profession, level 1).
        let mut random = LegacyRandom::from_seed(rand::random());
        let mut entity_data = ZombieVillagerEntityData::new(&mut random);
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(2, MeleeAttackGoal::new(1.0, false));
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
        }

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
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            can_break_doors: SyncMutex::new(false),
            villager_conversion_time: SyncMutex::new(NOT_CONVERTING),
            conversion_starter: SyncMutex::new(None),
            gossips: SyncMutex::new(None),
            trade_offers: SyncMutex::new(None),
            villager_xp: SyncMutex::new(DEFAULT_XP),
        }
    }

    /// Vanilla `ZombieVillager.isConverting`.
    pub fn is_converting(&self) -> bool {
        *self.entity_data.lock().converting.get()
    }

    /// Vanilla `ZombieVillager.isBaby`.
    pub fn is_baby(&self) -> bool {
        *self.entity_data.lock().zombie.baby.get()
    }

    /// Vanilla `ZombieVillager.setBaby`, including the transient baby speed modifier.
    pub fn set_baby(&self, baby: bool) {
        self.entity_data.lock().zombie.baby.set(baby);
        if self.level().is_some() {
            let mut attributes = self.attributes().lock();
            attributes.remove_modifier(vanilla_attributes::MOVEMENT_SPEED, &SPEED_MODIFIER_BABY_ID);
            if baby {
                attributes.add_modifier(
                    vanilla_attributes::MOVEMENT_SPEED,
                    AttributeModifier {
                        id: SPEED_MODIFIER_BABY_ID,
                        amount: 0.5,
                        operation: AttributeModifierOperation::AddMultipliedBase,
                    },
                    false,
                );
            }
        }
    }

    /// Vanilla `ZombieVillager.canBreakDoors`.
    pub fn can_break_doors(&self) -> bool {
        *self.can_break_doors.lock()
    }

    /// Vanilla `ZombieVillager.setCanBreakDoors`.
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

    /// Vanilla `ZombieVillagerVillagerDataHolder.getVillagerData`.
    pub fn villager_data(&self) -> VillagerData {
        *self.entity_data.lock().villager_data.get()
    }

    /// Vanilla `ZombieVillagerVillagerDataHolder.setVillagerData`.
    pub fn set_villager_data(&self, villager_data: VillagerData) {
        let mut entity_data = self.entity_data.lock();
        if entity_data.villager_data.get().profession != villager_data.profession {
            *self.trade_offers.lock() = None;
        }
        entity_data.villager_data.set(villager_data);
    }

    /// Vanilla `ZombieVillagerVillagerDataHolder.getVillagerDataFinalized`.
    pub fn villager_data_finalized(&self) -> bool {
        *self.entity_data.lock().villager_data_finalized.get()
    }

    /// Vanilla `ZombieVillagerVillagerDataHolder.setVillagerDataFinalized`.
    pub fn set_villager_data_finalized(&self, finalized: bool) {
        self.entity_data
            .lock()
            .villager_data_finalized
            .set(finalized);
    }

    /// Vanilla `ZombieVillager.getVillagerXp`.
    pub fn villager_xp(&self) -> i32 {
        *self.villager_xp.lock()
    }

    /// Vanilla `ZombieVillager.setVillagerXp`.
    pub fn set_villager_xp(&self, villager_xp: i32) {
        *self.villager_xp.lock() = villager_xp;
    }

    /// Vanilla `ZombieVillager.getGossips().copy()`.
    pub fn gossips_copy(&self) -> GossipContainer {
        self.gossips
            .lock()
            .as_ref()
            .map_or_else(GossipContainer::new, GossipContainer::copy)
    }

    /// Vanilla `ZombieVillager.setGossips`.
    pub fn set_gossips(&self, gossips: GossipContainer) {
        *self.gossips.lock() = Some(gossips);
    }

    /// Vanilla `ZombieVillager.setTradeOffers`.
    pub fn set_trade_offers(&self, trade_offers: MerchantOffers) {
        *self.trade_offers.lock() = Some(trade_offers);
    }

    /// Vanilla `ZombieVillager.removeWhenFarAway`.
    pub fn remove_when_far_away_zombie_villager(&self, _dist_sqr: f64) -> bool {
        !self.is_converting() && self.villager_xp() == 0
    }

    /// Vanilla `ZombieVillager.convertsInWater`.
    pub fn converts_in_water(&self) -> bool {
        false
    }

    /// The player that started the cure, if any (`ConversionPlayer`).
    pub fn conversion_starter(&self) -> Option<Uuid> {
        *self.conversion_starter.lock()
    }

    /// Vanilla `ZombieVillager.setVillagerConversionTime` (test hook).
    pub fn set_villager_conversion_time(&self, conversion_time: i32) {
        *self.villager_conversion_time.lock() = conversion_time;
    }

    /// Vanilla `ZombieVillager.startConverting`.
    pub fn start_converting(&self, player: Option<Uuid>, time: i32) {
        *self.conversion_starter.lock() = player;
        *self.villager_conversion_time.lock() = time;
        self.entity_data.lock().converting.set(true);
        self.remove_mob_effect(vanilla_mob_effects::WEAKNESS);

        let amplifier = i32::from(u8::from(
            self.level()
                .map_or(Difficulty::Normal, |world| world.difficulty()),
        ))
        .saturating_sub(1)
        .min(0);
        self.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::STRENGTH,
            time,
            amplifier,
        ));

        self.broadcast_entity_event(EntityStatus::ZombieConverting);
        if !self.is_silent() {
            // Vanilla `handleEntityEvent(16)` plays `playLocalSound` with
            // volume `1 + rand` and pitch `rand * 0.7 + 0.3`.
            let volume = 1.0 + rand::random::<f32>();
            let pitch = rand::random::<f32>() * 0.7 + 0.3;
            self.play_sound(&sound_events::ENTITY_ZOMBIE_VILLAGER_CURE, volume, pitch);
        }
    }

    /// Vanilla `ZombieVillager.getConversionProgress`.
    fn conversion_progress(&self) -> i32 {
        let mut amount = 1;
        if rand::random::<f32>() < SPECIAL_BLOCK_CHANCE {
            return amount;
        }
        let Some(world) = self.level() else {
            return amount;
        };

        let mut special_blocks = 0;
        let base_x = self.position().x as i32;
        let base_y = self.position().y as i32;
        let base_z = self.position().z as i32;
        let mut x = base_x - SPECIAL_BLOCK_RADIUS;
        while x < base_x + SPECIAL_BLOCK_RADIUS && special_blocks < MAX_SPECIAL_BLOCKS_COUNT {
            let mut y = base_y - SPECIAL_BLOCK_RADIUS;
            while y < base_y + SPECIAL_BLOCK_RADIUS && special_blocks < MAX_SPECIAL_BLOCKS_COUNT {
                let mut z = base_z - SPECIAL_BLOCK_RADIUS;
                while z < base_z + SPECIAL_BLOCK_RADIUS && special_blocks < MAX_SPECIAL_BLOCKS_COUNT
                {
                    let state = world.get_block_state(BlockPos::new(x, y, z));
                    if state
                        .get_block()
                        .has_tag(&BlockTag::SPEEDS_UP_ZOMBIE_VILLAGER_CURING)
                    {
                        if rand::random::<f32>() < SPECIAL_BLOCK_ADD_CHANCE {
                            amount += 1;
                        }
                        special_blocks += 1;
                    }
                    z += 1;
                }
                y += 1;
            }
            x += 1;
        }

        amount
    }

    /// Vanilla `ZombieVillager.tick` tail: advances the cure timer.
    fn tick_conversion(&self) {
        if !LivingEntity::is_alive(self) || !self.is_converting() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let progress = self.conversion_progress();
        let remaining = {
            let mut time = self.villager_conversion_time.lock();
            *time -= progress;
            *time
        };
        if remaining <= 0 {
            self.finish_conversion(&world);
        }
    }

    /// Vanilla `ZombieVillager.finishConversion`.
    fn finish_conversion(&self, world: &Arc<World>) {
        let silent = self.is_silent();
        let variant = self.villager_data();
        let finalized = self.villager_data_finalized();
        let xp = self.villager_xp();
        let gossips = self.gossips.lock().as_ref().map(GossipContainer::copy);
        let offers = self.trade_offers.lock().as_ref().map(MerchantOffers::copy);

        let converted = self.convert_to(
            &vanilla_entities::VILLAGER,
            ConversionParams::single(false, false),
            Some(&move |mob: &dyn Mob| {
                let Some(villager) = mob.downcast_ref::<VillagerEntity>() else {
                    log::error!("zombie villager cure produced a non villager entity");
                    return;
                };

                for slot in self.drop_preserved_equipment(|stack| {
                    stack.has_enchantment_effect(EnchantmentEffectComponent::PreventArmorChange)
                }) {
                    villager
                        .living_base()
                        .equipment()
                        .lock()
                        .set(slot, self.equipment_in_slot(slot));
                }

                VillagerDataHolder::set_villager_data_finalized(villager, finalized);
                VillagerDataHolder::set_villager_data(villager, variant);
                if let Some(gossips) = gossips.clone() {
                    villager.set_gossips(gossips);
                }
                if let Some(offers) = offers.clone() {
                    Merchant::override_offers(villager, offers);
                }
                villager.set_villager_xp(xp);
                mob.finalize_spawn(world, EntitySpawnReason::Conversion, None);
                villager.refresh_brain();

                if let Some(starter) = *self.conversion_starter.lock() {
                    if let Some(player) = world.get_entity_by_uuid(&starter)
                        && let Some(player) = player.as_player()
                    {
                        // Vanilla `ZombieVillager.finishConversion`:
                        // `CriteriaTriggers.CURED_ZOMBIE_VILLAGER.trigger(player, this, villager)`.
                        crate::player::advancement::triggers::raise_cured_zombie_villager(
                            player, self, villager,
                        );
                        villager.on_reputation_event_from(
                            ReputationEventType::ZOMBIE_VILLAGER_CURED,
                            player,
                        );
                    }
                }

                mob.add_mob_effect(MobEffectInstance::with_duration(
                    vanilla_mob_effects::NAUSEA,
                    CURE_NAUSEA_TICKS,
                    0,
                ));
                if !silent {
                    world.level_event(
                        level_events::SOUND_ZOMBIE_CONVERTED,
                        self.block_position(),
                        0,
                        None,
                    );
                }
            }),
        );

        if converted.is_none() {
            log::warn!("zombie villager cure failed; keeping the zombie villager");
        }
    }

    /// Vanilla `ZombieVillager.dropPreservedEquipment`.
    fn drop_preserved_equipment(&self, keep: impl Fn(&ItemStack) -> bool) -> Vec<EquipmentSlot> {
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

impl Entity for ZombieVillagerEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
        self.tick_conversion();
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        if self.is_baby() {
            return ZOMBIE_VILLAGER_BABY_DIMENSIONS;
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
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_ZOMBIE_VILLAGER_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("IsBaby", i8::from(self.is_baby()));
        nbt.insert("CanBreakDoors", i8::from(self.can_break_doors()));
        nbt.insert("DrownedConversionTime", -1i32);
        nbt.insert("InWaterTime", -1i32);

        nbt.insert("VillagerData", villager_data_to_nbt(self.villager_data()));
        nbt.insert(
            "VillagerDataFinalized",
            i8::from(self.villager_data_finalized()),
        );
        if let Some(offers) = self.trade_offers.lock().as_ref() {
            nbt.insert("Offers", offers.to_nbt());
        }
        if let Some(gossips) = self.gossips.lock().as_ref() {
            gossips.save_to(nbt);
        }
        nbt.insert(
            "ConversionTime",
            if self.is_converting() {
                *self.villager_conversion_time.lock()
            } else {
                NOT_CONVERTING
            },
        );
        if let Some(starter) = *self.conversion_starter.lock() {
            nbt.insert(
                "ConversionPlayer",
                simdnbt::owned::NbtTag::IntArray(starter.to_int_array().to_vec()),
            );
        }
        nbt.insert("Xp", self.villager_xp());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_baby(nbt.byte("IsBaby").is_some_and(|value| value != 0));
        self.set_can_break_doors(nbt.byte("CanBreakDoors").is_some_and(|value| value != 0));

        let villager_data = nbt
            .compound("VillagerData")
            .and_then(|compound| villager_data_from_nbt(&compound));
        if villager_data.is_some() || nbt.byte("VillagerDataFinalized").is_some() {
            self.set_villager_data_finalized(true);
            self.entity_data
                .lock()
                .villager_data
                .set(villager_data.unwrap_or_else(zombie_villager_villager_data));
        }

        *self.trade_offers.lock() = nbt
            .compound("Offers")
            .and_then(|compound| MerchantOffers::from_nbt(&compound));
        {
            let mut gossips = GossipContainer::new();
            gossips.load_from(nbt);
            *self.gossips.lock() = (!gossips.is_empty()).then_some(gossips);
        }

        let conversion_time = nbt.int("ConversionTime").unwrap_or(NOT_CONVERTING);
        if conversion_time != NOT_CONVERTING {
            let starter = nbt
                .int_array("ConversionPlayer")
                .and_then(|array| Uuid::from_int_array(&array));
            self.start_converting(starter, conversion_time);
        } else {
            self.entity_data.lock().converting.set(false);
            *self.villager_conversion_time.lock() = NOT_CONVERTING;
        }

        *self.villager_xp.lock() = nbt.int("Xp").unwrap_or(DEFAULT_XP);
    }
}

impl LivingEntity for ZombieVillagerEntity {
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
        Some(&sound_events::ENTITY_ZOMBIE_VILLAGER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ZOMBIE_VILLAGER_DEATH)
    }

    fn voice_pitch(&self) -> f32 {
        if self.is_baby() {
            (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 2.0
        } else {
            (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0
        }
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

impl Mob for ZombieVillagerEntity {
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
        Some(&sound_events::ENTITY_ZOMBIE_VILLAGER_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        self.remove_when_far_away_zombie_villager(dist_sqr)
    }

    /// Vanilla `Zombie.doHurtTarget` tail, inherited by `ZombieVillager`.
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

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held = {
            let inventory = player.inventory.lock();
            inventory
                .get_item_in_hand(hand)
                .is(&vanilla_items::GOLDEN_APPLE)
        };
        if !held {
            return InteractionResult::Pass;
        }
        if !self.has_mob_effect(vanilla_mob_effects::WEAKNESS) {
            return InteractionResult::Consume;
        }

        self.use_player_item(player, hand);
        if self.level().is_some() {
            self.start_converting(
                Some(player.uuid()),
                rand::random_range(
                    0..=(VILLAGER_CONVERSION_WAIT_MAX - VILLAGER_CONVERSION_WAIT_MIN),
                ) + VILLAGER_CONVERSION_WAIT_MIN,
            );
        }

        InteractionResult::SuccessServer
    }

    /// Vanilla `ZombieVillager.finalizeSpawn` finalizes the villager type and then
    /// runs `Zombie.finalizeSpawn`, whose spawn tail (loot pickup, baby odds,
    /// equipment, `handleAttributes`) applies to zombie villagers too.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.finalize_villager_type(world);

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
                zombie::populate_zombie_equipment_slots(self, &mut random, difficulty_modifier);
                zombie::populate_zombie_equipment_enchantments(
                    self,
                    &mut random,
                    difficulty_modifier,
                );
            }
        }

        zombie::handle_zombie_spawn_attributes(
            self,
            difficulty_modifier,
            spawn_reason,
            &mut random,
            &|can_break_doors| self.set_can_break_doors(can_break_doors),
        );

        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }
}

impl ZombieVillagerEntity {
    /// Vanilla `VillagerDataHolder.finalizeVillagerType`.
    fn finalize_villager_type(&self, world: &Arc<World>) {
        if self.villager_data_finalized() {
            return;
        }
        let Some(biome) = world.biome_at(self.block_position()) else {
            return;
        };
        let villager_type = VillagerType::by_biome(biome);
        let type_id = REGISTRY
            .villager_types
            .id_from_key(&villager_type.key)
            .map_or(0, |id| id as i32);
        let data = self.villager_data();
        self.set_villager_data(VillagerData::new(type_id, data.profession, data.level));
        self.set_villager_data_finalized(true);
    }
}

impl Monster for ZombieVillagerEntity {}

impl PathfinderMob for ZombieVillagerEntity {}

/// Vanilla `ZombieVillager.initializeZombieVillagerData`: plains type, no
/// profession, level 1, with a random registered profession.
fn zombie_villager_villager_data() -> VillagerData {
    let type_id = villager_type_id("plains");
    let profession_count = REGISTRY.villager_professions.len();
    let profession = if profession_count == 0 {
        villager_profession_id("none")
    } else {
        rand::random_range(0..profession_count) as i32
    };
    VillagerData::new(type_id, profession, 1)
}

fn villager_type_id(name: &'static str) -> i32 {
    REGISTRY
        .villager_types
        .id_from_key(&Identifier::vanilla_static(name))
        .map_or(0, |id| id as i32)
}

fn villager_profession_id(name: &'static str) -> i32 {
    REGISTRY
        .villager_professions
        .id_from_key(&Identifier::vanilla_static(name))
        .map_or(0, |id| id as i32)
}

fn villager_data_to_nbt(data: VillagerData) -> NbtCompound {
    let mut compound = NbtCompound::new();
    let type_key = REGISTRY
        .villager_types
        .by_id(data.villager_type.max(0) as usize)
        .map(|entry| entry.key.to_string());
    if let Some(type_key) = type_key {
        compound.insert("type", type_key);
    }
    let profession_key = REGISTRY
        .villager_professions
        .by_id(data.profession.max(0) as usize)
        .map(|entry| entry.key.to_string());
    if let Some(profession_key) = profession_key {
        compound.insert("profession", profession_key);
    }
    compound.insert("level", data.level);
    compound
}

fn villager_data_from_nbt(nbt: &BorrowedNbtCompoundView<'_, '_>) -> Option<VillagerData> {
    let type_id = nbt
        .string("type")
        .and_then(|value| value.to_str().parse::<Identifier>().ok())
        .and_then(|key| REGISTRY.villager_types.id_from_key(&key))
        .map_or_else(|| villager_type_id("plains"), |id| id as i32);
    let profession_id = nbt
        .string("profession")
        .and_then(|value| value.to_str().parse::<Identifier>().ok())
        .and_then(|key| REGISTRY.villager_professions.id_from_key(&key))
        .map_or_else(|| villager_profession_id("none"), |id| id as i32);
    let level = nbt.int("level").unwrap_or(1).max(1);
    Some(VillagerData::new(type_id, profession_id, level))
}
