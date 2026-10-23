//! Vanilla `Piglin` (`net.minecraft.world.entity.monster.piglin.Piglin`).
//!
//! Vanilla `Piglin` extends `AbstractPiglin` (which Steel has no counterpart for, so
//! the shared zombification/anger/sound state lives here) and is brain-driven through
//! [`brain`] (`PiglinAi`). The crossbow/gold-sword spawn equipment, the eight-slot
//! `InventoryCarrier`, the baby speed modifier and the dancing/charging synced flags
//! are the vanilla fields this entity owns.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PiglinEntityData;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

use crate::behavior::InteractionResult;
use crate::entity::ai::behavior::piglin::ai;
use crate::entity::ai::brain::Brain;
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::conversion::ConversionParams;
use crate::entity::damage::DamageSource;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::{LevelReader, World};

use super::brain;

/// Vanilla `Piglin.SPEED_MODIFIER_BABY_ID`.
const SPEED_MODIFIER_BABY_ID: Identifier = Identifier::vanilla_static("baby");
/// Vanilla `Piglin.SPEED_MODIFIER_BABY` amount (`ADD_MULTIPLIED_BASE`).
const SPEED_MODIFIER_BABY_AMOUNT: f64 = 0.2;
/// Vanilla `Piglin` XP reward.
const XP_REWARD: i32 = 5;
/// Vanilla `Piglin.CHANCE_OF_WEARING_EACH_ARMOUR_ITEM`.
const CHANCE_OF_WEARING_EACH_ARMOUR_ITEM: f32 = 0.1;
/// Vanilla `Piglin.PROBABILITY_OF_SPAWNING_AS_BABY`.
const PROBABILITY_OF_SPAWNING_AS_BABY: f32 = 0.2;
/// Vanilla `Piglin.PROBABILITY_OF_SPAWNING_WITH_CROSSBOW_INSTEAD_OF_SWORD`.
const PROBABILITY_OF_SPAWNING_WITH_CROSSBOW: f32 = 0.5;
/// Vanilla `Piglin` chance of a golden spear instead of a golden sword.
const PROBABILITY_OF_SPAWNING_WITH_SPEAR: i32 = 10;
/// Vanilla `AbstractPiglin.CONVERSION_TIME`.
const CONVERSION_TIME: i32 = 300;
/// Vanilla `AbstractPiglin` `DEFAULT_STEP_HEIGHT`.
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Piglin.MAX_PASSENGERS_ON_ONE_HOGLIN`.
const MAX_PASSENGERS_ON_ONE_HOGLIN: i32 = 3;

const PIGLIN_BABY_WIDTH: f32 = 0.49;
const PIGLIN_BABY_HEIGHT: f32 = 0.98;
const PIGLIN_BABY_EYE_HEIGHT: f32 = 0.78;
const PIGLIN_BABY_VEHICLE_Y: f64 = 0.1875;

const PIGLIN_BABY_VEHICLE_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, PIGLIN_BABY_VEHICLE_Y, 0.0)];

const PIGLIN_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    PIGLIN_BABY_WIDTH,
    PIGLIN_BABY_HEIGHT,
    PIGLIN_BABY_EYE_HEIGHT,
    EntityAttachments::new(&[], &PIGLIN_BABY_VEHICLE_ATTACHMENTS, &[], &[]),
);

/// Vanilla `PiglinArmPose`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PiglinArmPose {
    /// Holding a melee weapon while aggressive.
    AttackingWithMeleeWeapon,
    /// Holding a charged crossbow.
    CrossbowHold,
    /// Charging a crossbow.
    CrossbowCharge,
    /// Admiring a loved item in the off hand.
    AdmiringItem,
    /// Dancing after a hoglin kill.
    Dancing,
    /// Default pose.
    Default,
}

/// Vanilla `Piglin`.
#[entity_behavior(class = "Piglin")]
pub struct PiglinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<PiglinEntityData>,
    brain: SyncMutex<Brain>,
    /// Vanilla `Piglin.inventory` (`SimpleContainer(8)`).
    inventory: SyncMutex<InventoryCarrier>,
    /// Vanilla `Piglin.cannotHunt`.
    cannot_hunt: SyncMutex<bool>,
    /// Vanilla `AbstractPiglin.timeInOverworld`.
    time_in_overworld: SyncMutex<i32>,
    /// Mirror of `PiglinAi.isAdmiringDisabled(body) && hasMemoryValue(ATTACK_TARGET)`.
    ///
    /// The item sensor calls `wantsToPickUp` while the brain lock is held, so the two
    /// memory reads are mirrored into a plain flag by the activity update.
    pickup_blocked: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PiglinEntity`.
unsafe impl DowncastType for PiglinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/piglin");
}

impl PiglinEntity {
    /// Creates a new piglin entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved piglin entity.
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

        // Vanilla `AbstractPiglin` constructor: can pick up loot, opens doors, and
        // treats neighboring fire as very expensive but fire itself as impassable.
        mob_base.set_can_pick_up_loot(true);
        mob_base.navigation().lock().set_can_open_doors(true);
        {
            let mut malus = mob_base.pathfinding_malus().lock();
            malus.set(crate::entity::ai::path::PathType::FireInNeighbor, 16.0);
            malus.set(crate::entity::ai::path::PathType::Fire, -1.0);
        }

        let mut entity_data = PiglinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            brain: SyncMutex::new(brain::make_piglin_brain()),
            inventory: SyncMutex::new(InventoryCarrier::new(8)),
            cannot_hunt: SyncMutex::new(false),
            time_in_overworld: SyncMutex::new(0),
            pickup_blocked: SyncMutex::new(false),
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

    /// Vanilla `Piglin.isBaby` (`DATA_BABY_ID`).
    #[must_use]
    pub fn is_baby(&self) -> bool {
        *self.entity_data.lock().baby.get()
    }

    /// Vanilla `Piglin.setBaby`: toggles the synced flag and the baby speed modifier.
    pub fn set_baby(&self, baby: bool) {
        self.entity_data.lock().baby.set(baby);

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

    /// Vanilla `AbstractPiglin.isAdult`.
    #[must_use]
    pub fn is_adult(&self) -> bool {
        !self.is_baby()
    }

    /// Vanilla `Piglin.canHunt`.
    #[must_use]
    pub fn cannot_hunt(&self) -> bool {
        *self.cannot_hunt.lock()
    }

    /// Vanilla `Piglin.setCannotHunt`.
    pub fn set_cannot_hunt(&self, cannot_hunt: bool) {
        *self.cannot_hunt.lock() = cannot_hunt;
    }

    /// Vanilla `AbstractPiglin.isImmuneToZombification`.
    #[must_use]
    pub fn is_immune_to_zombification(&self) -> bool {
        *self.entity_data.lock().immune_to_zombification.get()
    }

    /// Vanilla `AbstractPiglin.setImmuneToZombification`.
    pub fn set_immune_to_zombification(&self, immune: bool) {
        self.entity_data.lock().immune_to_zombification.set(immune);
    }

    /// Vanilla `AbstractPiglin.isConverting`: not immune, has AI, and the level's
    /// `piglins_zombify` environment attribute applies here.
    ///
    /// Steel reads the attribute from the world's dimension type; the per-position
    /// environment-attribute timeline is not evaluated (documented deviation: the
    /// attribute is dimension-wide, which matches every vanilla dimension today).
    #[must_use]
    pub fn is_converting(&self) -> bool {
        !self.is_immune_to_zombification()
            && !self.is_no_ai()
            && self
                .level()
                .is_some_and(|world| world.dimension_type.piglins_zombify)
    }

    /// Vanilla `Piglin.isDancing`.
    #[must_use]
    pub fn is_dancing(&self) -> bool {
        *self.entity_data.lock().is_dancing.get()
    }

    /// Vanilla `Piglin.setDancing`.
    pub fn set_dancing(&self, dancing: bool) {
        self.entity_data.lock().is_dancing.set(dancing);
    }

    /// Vanilla `Piglin.isChargingCrossbow`.
    #[must_use]
    pub fn is_charging_crossbow(&self) -> bool {
        *self.entity_data.lock().is_charging_crossbow.get()
    }

    /// Vanilla `Piglin.setChargingCrossbow`.
    pub fn set_charging_crossbow(&self, charging: bool) {
        self.entity_data.lock().is_charging_crossbow.set(charging);
    }

    /// Vanilla `Piglin.getInventory`.
    #[must_use]
    pub fn get_inventory(&self) -> &SyncMutex<InventoryCarrier> {
        &self.inventory
    }

    /// Vanilla `Piglin.addToInventory`.
    #[must_use]
    pub fn add_to_inventory(&self, item_stack: ItemStack) -> ItemStack {
        self.inventory.lock().add_item(&item_stack)
    }

    /// Vanilla `Piglin.canAddToInventory`.
    #[must_use]
    pub fn can_add_to_inventory(&self, item_stack: &ItemStack) -> bool {
        self.inventory.lock().can_add_item(item_stack)
    }

    /// Vanilla `PiglinAi.wantsToPickup`'s memory gate, mirrored by the activity update.
    #[must_use]
    pub fn is_pickup_blocked(&self) -> bool {
        *self.pickup_blocked.lock()
    }

    /// Vanilla `Piglin.holdInMainHand` (`setItemSlotAndDropWhenKilled(MAINHAND, …)`).
    pub fn hold_in_main_hand(&self, item_stack: ItemStack) {
        self.living_base
            .equipment()
            .lock()
            .set(EquipmentSlot::MainHand, item_stack);
        self.refresh_equipment_attribute_modifiers(EquipmentSlot::MainHand);
        self.set_guaranteed_drop(EquipmentSlot::MainHand);
        self.set_persistence_required();
    }

    /// Vanilla `Piglin.holdInOffHand` (`setItemSlotAndDropWhenKilled(OFFHAND, …)`).
    ///
    /// Vanilla only forces persistence for non-barter items.
    pub fn hold_in_off_hand(&self, item_stack: ItemStack) {
        let is_barter_item = ai::is_barter_currency(&item_stack);
        self.living_base
            .equipment()
            .lock()
            .set(EquipmentSlot::OffHand, item_stack);
        self.refresh_equipment_attribute_modifiers(EquipmentSlot::OffHand);
        self.set_guaranteed_drop(EquipmentSlot::OffHand);
        if !is_barter_item {
            self.set_persistence_required();
        }
    }

    /// Vanilla `Piglin.getArmPose`.
    #[must_use]
    pub fn arm_pose(&self) -> PiglinArmPose {
        if self.is_dancing() {
            return PiglinArmPose::Dancing;
        }

        let off_hand = self
            .as_living_entity()
            .map_or_else(ItemStack::empty, |living| {
                living.get_item_in_hand(InteractionHand::OffHand)
            });
        if ai::is_loved_item(&off_hand) {
            return PiglinArmPose::AdmiringItem;
        }

        if self.is_aggressive() && self.is_holding_melee_weapon() {
            return PiglinArmPose::AttackingWithMeleeWeapon;
        }
        if self.is_charging_crossbow() {
            return PiglinArmPose::CrossbowCharge;
        }

        let crossbow_charged = self.as_living_entity().is_some_and(|living| {
            living.is_holding(&mut |stack| stack.is(&vanilla_items::CROSSBOW))
        }) && self.as_living_entity().is_some_and(|living| {
            living.is_holding(&mut |stack| {
                stack.has(steel_registry::data_components::vanilla_components::CHARGED_PROJECTILES)
            })
        });
        if crossbow_charged {
            PiglinArmPose::CrossbowHold
        } else {
            PiglinArmPose::Default
        }
    }

    /// Vanilla `AbstractPiglin.isHoldingMeleeWeapon`: the main-hand stack has `TOOL`.
    #[must_use]
    fn is_holding_melee_weapon(&self) -> bool {
        self.as_living_entity().is_some_and(|living| {
            living.is_holding(&mut |stack| {
                stack.has_component(&steel_registry::data_components::vanilla_components::TOOL.key)
            })
        })
    }

    /// Vanilla `Piglin.createSpawnWeapon`: crossbow, else golden sword (10% spear).
    fn create_spawn_weapon(&self) -> ItemStack {
        if rand::random::<f32>() < PROBABILITY_OF_SPAWNING_WITH_CROSSBOW {
            return ItemStack::new(&vanilla_items::CROSSBOW);
        }
        if rand::random_range(0..PROBABILITY_OF_SPAWNING_WITH_SPEAR) == 0 {
            ItemStack::new(&vanilla_items::GOLDEN_SPEAR)
        } else {
            ItemStack::new(&vanilla_items::GOLDEN_SWORD)
        }
    }

    /// Vanilla `Piglin.playConvertedSound`.
    fn play_converted_sound(&self) {
        self.make_sound(Some(&sound_events::ENTITY_PIGLIN_CONVERTED_TO_ZOMBIFIED));
    }

    /// Vanilla `AbstractPiglin.finishConversion` + `Piglin.finishConversion`.
    fn finish_conversion(&self) {
        ai::cancel_admiring(self);
        let drops = {
            let mut inventory = self.inventory.lock();
            let drops: Vec<ItemStack> = inventory
                .inventory()
                .items()
                .iter()
                .filter(|stack| !stack.is_empty())
                .cloned()
                .collect();
            inventory.clear();
            drops
        };
        for drop in drops {
            self.spawn_at_location(drop, 0.0);
        }

        let converted = self.convert_to(
            &vanilla_entities::ZOMBIFIED_PIGLIN,
            ConversionParams::single(true, true),
            Some(&|zombified| {
                zombified.add_mob_effect(MobEffectInstance::with_duration(
                    &steel_registry::vanilla_mob_effects::NAUSEA,
                    200,
                    0,
                ));
            }),
        );
        if converted.is_some() {
            self.play_converted_sound();
        }
    }

    /// Vanilla `AbstractPiglin.playAmbientSound`: only while idle.
    fn play_ambient_sound_if_idle(&self) {
        if ai::is_idle(self.as_entity_event_source()) {
            self.make_sound(self.ambient_sound());
        }
    }
}

impl Entity for PiglinEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    /// Vanilla `Piglin.getDefaultDimensions` uses the baby hitbox for babies.
    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.is_baby() {
            PIGLIN_BABY_DIMENSIONS.scale(scale)
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
        self.play_sound(&sound_events::ENTITY_PIGLIN_STEP, 0.15, 1.0);
    }

    /// Vanilla `Piglin.hurtServer`: anger/retreat handling for a living attacker.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        let was_hurt = LivingEntity::hurt_server(self, world, source, amount);
        if was_hurt
            && let Some(attacker) = source.causing_entity()
            && attacker.as_living_entity().is_some()
        {
            ai::was_hurt_by(world, self.as_entity_event_source(), attacker);
        }
        was_hurt
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("IsBaby", i8::from(self.is_baby()));
        nbt.insert("CannotHunt", i8::from(self.cannot_hunt()));
        nbt.insert(
            "IsImmuneToZombification",
            i8::from(self.is_immune_to_zombification()),
        );
        nbt.insert("TimeInOverworld", *self.time_in_overworld.lock());
        self.inventory.lock().write_to_tag(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_can_pick_up_loot(nbt.byte("CanPickUpLoot").is_none_or(|value| value != 0));
        self.set_baby(nbt.byte("IsBaby").is_some_and(|value| value != 0));
        self.set_cannot_hunt(nbt.byte("CannotHunt").is_some_and(|value| value != 0));
        self.set_immune_to_zombification(
            nbt.byte("IsImmuneToZombification")
                .is_some_and(|value| value != 0),
        );
        *self.time_in_overworld.lock() = nbt.int("TimeInOverworld").unwrap_or(0);
        self.inventory.lock().read_from_tag(&nbt);
    }
}

impl LivingEntity for PiglinEntity {
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

    fn is_baby(&self) -> bool {
        PiglinEntity::is_baby(self)
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PIGLIN_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PIGLIN_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    /// Vanilla `Piglin.dropCustomDeathLoot`: the carried inventory spills.
    fn drop_custom_death_loot(&self, _source: &DamageSource, _killed_by_player: bool) {
        let drops = {
            let mut inventory = self.inventory.lock();
            let drops: Vec<ItemStack> = inventory
                .inventory()
                .items()
                .iter()
                .filter(|stack| !stack.is_empty())
                .cloned()
                .collect();
            inventory.clear();
            drops
        };
        for drop in drops {
            self.spawn_at_location(drop, 0.0);
        }
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for PiglinEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Piglin.getAmbientSound` (`PiglinAi.getSoundForCurrentActivity`).
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        let world = self.level()?;
        let brain = self.brain.lock();
        ai::get_sound_for_current_activity(&brain, &world, self.as_entity_event_source())
    }

    /// Vanilla `AbstractPiglin.playAmbientSound`: only while idle.
    fn play_ambient_sound(&self) {
        self.play_ambient_sound_if_idle();
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

    /// Vanilla `Piglin.customServerAiStep`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        let update = {
            let mut brain = self.brain.lock();
            brain::tick_piglin(&mut brain, &world, entity);
            let update = brain::update_piglin_activity(&mut brain);
            *self.pickup_blocked.lock() = ai::is_admiring_disabled(brain.memories())
                && brain.has_memory_value(
                    &crate::entity::ai::brain::memory::module_type::ATTACK_TARGET,
                );
            update
        };

        if update.activity_changed
            && let Some(sound) =
                ai::get_sound_for_current_activity(&self.brain.lock(), &world, entity)
        {
            self.make_sound(Some(sound));
        }

        self.set_aggressive(update.aggressive);
        if !update.has_ride_target && self.is_baby_riding_baby() {
            self.stop_riding();
        }
        self.set_dancing(update.dancing);

        // Vanilla `AbstractPiglin.customServerAiStep` conversion clock.
        if self.is_converting() {
            let mut time = self.time_in_overworld.lock();
            *time += 1;
            if *time > CONVERSION_TIME {
                drop(time);
                self.finish_conversion();
            }
        } else {
            *self.time_in_overworld.lock() = 0;
        }
    }

    /// Vanilla `Piglin.finalizeSpawn`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let mut random =
            steel_utils::random::legacy_random::LegacyRandom::from_seed(rand::random());
        use steel_utils::random::Random as _;

        if spawn_reason != EntitySpawnReason::Structure {
            if random.next_f32() < PROBABILITY_OF_SPAWNING_AS_BABY {
                self.set_baby(true);
            } else if self.is_adult() {
                let weapon = self.create_spawn_weapon();
                self.hold_in_main_hand(weapon);
            }
        }

        {
            let mut brain = self.brain.lock();
            brain::init_memories(&mut brain);
        }

        self.populate_default_equipment_slots();
        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    /// Vanilla `Piglin.wantsToPickUp`.
    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        let mob_griefing = self
            .level()
            .is_none_or(|world| world.get_game_rule(&MOB_GRIEFING));
        mob_griefing
            && self.can_pick_up_loot()
            && ai::wants_to_pickup(self.as_entity_event_source(), item_stack)
    }

    /// Vanilla `Piglin.pickUpItem`.
    fn pick_up_item(&self, world: &Arc<World>, item_entity: &crate::entity::entities::ItemEntity) {
        ai::pick_up_item(world, self.as_entity_event_source(), item_entity);
    }

    /// Vanilla `Piglin.canReplaceCurrentItem`.
    fn can_replace_current_item(
        &self,
        new_item_stack: &ItemStack,
        current_item_stack: &ItemStack,
        slot: EquipmentSlot,
    ) -> bool {
        if current_item_stack.has_enchantment_effect(
            steel_registry::enchantment_effect::EnchantmentEffectComponent::PreventArmorChange,
        ) {
            return false;
        }

        let preferred = self.get_preferred_weapon_type();
        let new_wanted = ai::is_loved_item(new_item_stack)
            || preferred
                .as_ref()
                .is_some_and(|tag| new_item_stack.item().has_tag(tag));
        let current_wanted = ai::is_loved_item(current_item_stack)
            || preferred
                .as_ref()
                .is_some_and(|tag| current_item_stack.item().has_tag(tag));

        if new_wanted != current_wanted {
            return new_wanted;
        }

        Mob::can_replace_current_item(self, new_item_stack, current_item_stack, slot)
    }

    /// Vanilla `Piglin.getPreferredWeaponType`: adults prefer the piglin weapon tag.
    fn get_preferred_weapon_type(&self) -> Option<Identifier> {
        if self.is_baby() {
            None
        } else {
            Some(ItemTag::PIGLIN_PREFERRED_WEAPONS)
        }
    }

    /// Vanilla `Piglin.mobInteract` through `PiglinAi.mobInteract`.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        ai::mob_interact(&world, self.as_entity_event_source(), player, hand)
    }

    /// Vanilla `Piglin.getBaseExperienceReward`.
    fn base_experience_reward_mob(&self) -> i32 {
        XP_REWARD
    }

    fn mob_can_be_leashed(&self) -> bool {
        true
    }

    /// Vanilla `Piglin.startRiding`: a baby rides the top passenger of a hoglin stack.
    fn start_riding(&self, entity_to_ride: &SharedEntity) -> bool {
        let mut target = entity_to_ride.clone();
        if self.is_baby() && target.entity_type().key == vanilla_entities::HOGLIN.key {
            target = top_passenger(&target, MAX_PASSENGERS_ON_ONE_HOGLIN);
        }
        Entity::start_riding(self, &target)
    }

    /// Vanilla `Piglin.checkPiglinSpawnRules`: never above nether wart blocks.
    fn check_mob_spawn_rules(
        entity_type: EntityTypeRef,
        level: &dyn LevelReader,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
    ) -> bool
    where
        Self: Sized,
    {
        let _ = (entity_type, spawn_reason);
        check_piglin_spawn_rules(level, pos)
    }
}

impl Monster for PiglinEntity {}

impl PathfinderMob for PiglinEntity {}

impl PiglinEntity {
    /// Vanilla `Piglin.populateDefaultEquipmentSlots`: adults may wear golden armor.
    fn populate_default_equipment_slots(&self) {
        if !self.is_adult() {
            return;
        }

        let armor = [
            (EquipmentSlot::Head, &vanilla_items::GOLDEN_HELMET),
            (EquipmentSlot::Chest, &vanilla_items::GOLDEN_CHESTPLATE),
            (EquipmentSlot::Legs, &vanilla_items::GOLDEN_LEGGINGS),
            (EquipmentSlot::Feet, &vanilla_items::GOLDEN_BOOTS),
        ];
        for (slot, item) in armor {
            if rand::random::<f32>() < CHANCE_OF_WEARING_EACH_ARMOUR_ITEM {
                self.living_base
                    .equipment()
                    .lock()
                    .set(slot, ItemStack::new(item));
                self.refresh_equipment_attribute_modifiers(slot);
            }
        }
    }

    /// Vanilla `PiglinAi.isBabyRidingBaby`: a baby piglin riding a baby piglin/hoglin.
    fn is_baby_riding_baby(&self) -> bool {
        if !self.is_baby() {
            return false;
        }
        let Some(vehicle) = self.vehicle() else {
            return false;
        };
        let is_baby_vehicle = vehicle
            .as_living_entity()
            .is_some_and(LivingEntity::is_baby);
        let rideable = vehicle.downcast_ref::<PiglinEntity>().is_some()
            || vehicle
                .downcast_ref::<crate::entity::entities::HoglinEntity>()
                .is_some();
        rideable && is_baby_vehicle
    }
}

/// Vanilla `Piglin.checkPiglinSpawnRules`.
#[must_use]
pub fn check_piglin_spawn_rules(level: &dyn LevelReader, pos: BlockPos) -> bool {
    level.get_block_state(pos.below()).get_block()
        != &steel_registry::vanilla_blocks::NETHER_WART_BLOCK
}

/// Vanilla `Piglin.getTopPassenger`.
fn top_passenger(vehicle: &SharedEntity, counter: i32) -> SharedEntity {
    let passengers = vehicle.passengers();
    if counter == 1 || passengers.is_empty() {
        return vehicle.clone();
    }
    top_passenger(&passengers[0], counter - 1)
}

/// `steel_registry::blocks::block_state_ext::BlockStateExt` is needed for `get_block`.
use steel_registry::blocks::block_state_ext::BlockStateExt as _;

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::vanilla_item_tags::ItemTag;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};

    use super::PiglinEntity;
    use crate::entity::ai::behavior::piglin::ai;
    use crate::entity::{Entity, LivingEntity, Mob};
    use crate::inventory::equipment::EquipmentSlot;
    use steel_registry::item_stack::ItemStack;

    fn piglin() -> PiglinEntity {
        init_vanilla_registry();
        PiglinEntity::new(&vanilla_entities::PIGLIN, 1, DVec3::ZERO, Weak::new())
    }

    #[test]
    fn piglin_baby_state_drives_the_speed_modifier() {
        let piglin = piglin();
        assert!(piglin.is_adult());
        piglin.set_baby(true);
        assert!(piglin.is_baby());
        assert!(!piglin.is_adult());

        let amount = piglin.attributes().lock().modifier_value(
            steel_registry::vanilla_attributes::MOVEMENT_SPEED,
            &super::SPEED_MODIFIER_BABY_ID,
        );
        assert_eq!(amount, Some(0.2));
    }

    #[test]
    fn piglin_barter_and_pickup_rules_match_vanilla() {
        let piglin = piglin();
        let body = piglin.as_entity_event_source();

        assert!(ai::is_barter_currency(&ItemStack::new(
            &vanilla_items::GOLD_INGOT
        )));
        assert!(ai::is_loved_item(&ItemStack::new(
            &vanilla_items::GOLD_INGOT
        )));
        assert!(piglin.wants_to_pick_up(&ItemStack::new(&vanilla_items::GOLD_INGOT)));
        assert!(!piglin.wants_to_pick_up(&ItemStack::new(&vanilla_items::GOLDEN_HELMET)));
        // Adults carry loved items; the off hand is empty, so gold is accepted.
        assert!(ai::wants_to_pickup(
            body,
            &ItemStack::new(&vanilla_items::GOLD_INGOT)
        ));
    }

    #[test]
    fn piglin_equipment_preference_and_offhand_hold() {
        let piglin = piglin();
        piglin.hold_in_off_hand(ItemStack::new(&vanilla_items::GOLD_INGOT));
        assert_eq!(
            piglin.equipment_in_slot(EquipmentSlot::OffHand).item().key,
            vanilla_items::GOLD_INGOT.key
        );

        assert_eq!(
            Mob::get_preferred_weapon_type(&piglin),
            Some(ItemTag::PIGLIN_PREFERRED_WEAPONS)
        );
        piglin.set_baby(true);
        assert_eq!(Mob::get_preferred_weapon_type(&piglin), None);
    }

    #[test]
    fn piglin_nbt_round_trips_baby_and_zombification_state() {
        let piglin = piglin();
        piglin.set_baby(true);
        piglin.set_cannot_hunt(true);
        piglin.set_immune_to_zombification(true);

        let mut nbt = simdnbt::owned::NbtCompound::new();
        piglin.save_additional(&mut nbt);
        assert_eq!(nbt.byte("IsBaby"), Some(1));
        assert_eq!(nbt.byte("CannotHunt"), Some(1));
        assert_eq!(nbt.byte("IsImmuneToZombification"), Some(1));
        assert_eq!(piglin.entity_type(), &vanilla_entities::PIGLIN);
    }
}
