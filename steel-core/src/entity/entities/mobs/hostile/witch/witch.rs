//! Vanilla `Witch` (vanilla `net.minecraft.world.entity.monster.Witch`).
//!
//! Vanilla `Witch extends Raider`: it is a raid mob (the raid spawn table has a witch
//! row) but not an illager, so Steel implements the [`Raider`] capability trait plus
//! [`RaiderBase`], exactly like `Vindicator`, and keeps no illager arm-pose state.
//!
//! Potion selection is driven from the AI step, mirroring vanilla `Witch.aiStep`. Vanilla
//! has no `UseItemGoal` for the witch: it picks a potion in `aiStep`, equips it, sets
//! `usingTime = getUseDuration(...)` and applies the effects itself when the countdown
//! ends. Steel's living item-use pipeline already owns the countdown, the equip/clear of
//! the main hand and the effect application, so this file starts that pipeline with the
//! vanilla-selected potion and clears the hand when the pipeline finishes.

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::entity_behavior;
use steel_registry::data_components::PotionContents;
use steel_registry::data_components::vanilla_components;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::potion::PotionRef;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::WitchEntityData;
use steel_registry::{
    RegistryReference, sound_events, vanilla_attributes, vanilla_custom_stats,
    vanilla_damage_type_tags, vanilla_items, vanilla_mob_effects, vanilla_potions,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{DowncastType, DowncastTypeKey, Identifier};

use crate::enchantment_helper::get_damage_protection;
use crate::entity::ai::goal::{
    FloatGoal, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableWitchTargetGoal,
    NearestHealableRaiderTargetGoal, PathfindToRaidGoal, RaiderCelebrationGoal,
    RaiderMoveThroughVillageGoal, RaiderObtainLeaderBannerGoal, RandomLookAroundGoal,
    RangedAttackGoal, RangedAttackMob, WaterAvoidingRandomStrollGoal, WitchTargetState,
};
use crate::entity::attribute::{AttributeModifier, AttributeModifierOperation};
use crate::entity::combat_rules::get_damage_after_magic_absorb;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::illager::abstract_illager::RaiderKeys;
use crate::entity::entities::objects::projectiles::ThrownSplashPotionEntity;
use crate::entity::projectile::spawn_projectile;
use crate::entity::raid::RaidSnapshot;
use crate::entity::raid::raider::{Raider, RaiderBase};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, Projectile as _, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Monster.xpReward`; the witch does not override it.
const XP_REWARD: i32 = 5;
/// Vanilla `Witch.SPEED_MODIFIER_DRINKING_ID` (`minecraft:drinking`).
const DRINKING_MODIFIER_ID: Identifier = Identifier::vanilla_static("drinking");
/// Vanilla `Witch.SPEED_MODIFIER_DRINKING` (`ADD_VALUE`, `-0.25`).
const DRINKING_MODIFIER_AMOUNT: f64 = -0.25;
/// Vanilla `Witch.rangedAttackUncertainty`.
const RANGED_ATTACK_UNCERTAINTY: f32 = 8.0;
/// Vanilla `Witch.performRangedAttack` velocity for a target within two blocks.
const CLOSE_THROW_VELOCITY: f32 = 0.45;
/// Vanilla `Witch.performRangedAttack` velocity for a farther target.
const FAR_THROW_VELOCITY: f32 = 0.75;
/// Vanilla `Witch.aiStep` speed-potion distance check (`distanceToSqr(this) > 121.0`).
const SPEED_POTION_DISTANCE_SQR: f64 = 121.0;
/// Vanilla `Witch.performRangedAttack` slowness distance gate.
const SLOWNESS_DISTANCE: f64 = 8.0;
/// Vanilla `Witch.performRangedAttack` weakness distance gate.
const WEAKNESS_DISTANCE: f64 = 3.0;
/// Vanilla `Witch.performRangedAttack` healing threshold for a wounded raider.
const RAIDER_HEALING_HEALTH: f32 = 4.0;
/// Vanilla `Witch.performRangedAttack` poison health gate.
const POISON_HEALTH: f32 = 8.0;

/// Vanilla witch entity.
#[entity_behavior(class = "Witch")]
pub struct WitchEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    raider_base: RaiderBase,
    entity_data: SyncMutex<WitchEntityData>,
    /// Shared handle for the two witch target goals, driven from the AI step exactly
    /// like vanilla's `healRaidersGoal`/`attackPlayersGoal` fields.
    target_state: WitchTargetState,
    /// Mirrors vanilla's `usingTime > 0` state across AI steps, so the main hand is
    /// cleared and the `minecraft:drinking` modifier removed exactly once when the
    /// item-use pipeline finishes.
    was_drinking: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `WitchEntity`.
unsafe impl DowncastType for WitchEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/witch");
}

impl WitchEntity {
    /// Creates a new witch entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved witch entity.
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
        let mut entity_data = WitchEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        let target_state = WitchTargetState::new();

        {
            // Vanilla `Witch.registerGoals` first runs `super.registerGoals()`
            // (`Raider.registerGoals`: banner hunt, raid pathfinding, village walk,
            // celebration), then adds its own goals.
            let mut goals = mob_base.goal_selector().lock();
            goals.add_goal(1, RaiderObtainLeaderBannerGoal::new());
            goals.add_goal(1, FloatGoal::new(&mob_base));
            goals.add_goal(2, RangedAttackGoal::new(1.0, 60, 10.0));
            goals.add_goal(2, WaterAvoidingRandomStrollGoal::new(1.0));
            goals.add_goal(3, PathfindToRaidGoal::new());
            goals.add_goal(3, LookAtPlayerGoal::new(8.0));
            goals.add_goal(3, RandomLookAroundGoal::new());
            goals.add_goal(4, RaiderMoveThroughVillageGoal::new(1.05, 1));
            goals.add_goal(5, RaiderCelebrationGoal::new());

            let mut targets = mob_base.target_selector().lock();
            targets.add_goal(
                1,
                HurtByTargetGoal::new()
                    .with_ignored_damage_types(RaiderKeys::ALL)
                    .set_alert_others(empty()),
            );
            targets.add_goal(
                2,
                NearestHealableRaiderTargetGoal::for_witch(target_state.clone()),
            );
            targets.add_goal(
                3,
                NearestAttackableWitchTargetGoal::for_witch_players(target_state.clone()),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            raider_base: RaiderBase::new(),
            entity_data: SyncMutex::new(entity_data),
            target_state,
            was_drinking: SyncMutex::new(false),
        }
    }

    /// Vanilla `Witch.isDrinkingPotion` (`DATA_USING_ITEM`).
    #[must_use]
    pub fn is_drinking_potion(&self) -> bool {
        *self.entity_data.lock().using_item.get()
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }
        let display = self.living_base.mob_effect_display_state();
        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.raider_mut().mob_mut().living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }
        self.entity_data.set_base_invisible_flag(display.invisible);
    }

    /// Vanilla `Witch.aiStep` prologue: advance the heal cooldown and allow player
    /// targeting only while the witch is not busy healing raiders.
    fn update_witch_target_state(&self) {
        self.target_state.decrement_heal_cooldown();
        self.target_state
            .set_can_attack(self.target_state.heal_cooldown() <= 0);
    }

    /// Vanilla `Witch.aiStep` potion block.
    ///
    /// The potion selection, equip, drink sound and `minecraft:drinking` speed modifier
    /// run here; the countdown and effect application are the living item-use pipeline's.
    fn tick_potion_use(&self) {
        let using = self.is_using_item();
        let mut was_drinking = self.was_drinking.lock();

        if using {
            *was_drinking = true;
            return;
        }

        if *was_drinking {
            // Vanilla `usingTime-- <= 0`: clear the main hand, drop the speed modifier
            // and the synced drinking flag. Effects were applied by the potion item's
            // `finish_using` when the pipeline completed.
            *was_drinking = false;
            self.set_item_in_hand(InteractionHand::MainHand, ItemStack::empty());
            self.attributes()
                .lock()
                .remove_modifier(vanilla_attributes::MOVEMENT_SPEED, &DRINKING_MODIFIER_ID);
            self.entity_data.lock().using_item.set(false);
            return;
        }

        let Some(potion) = self.select_self_potion() else {
            return;
        };

        self.set_item_in_hand(InteractionHand::MainHand, potion_stack(potion));
        self.start_using_item(InteractionHand::MainHand);
        self.entity_data.lock().using_item.set(true);
        self.play_sound(
            &sound_events::ENTITY_WITCH_DRINK,
            1.0,
            0.8 + rand::random::<f32>() * 0.4,
        );
        self.attributes().lock().set_modifier(
            vanilla_attributes::MOVEMENT_SPEED,
            AttributeModifier {
                id: DRINKING_MODIFIER_ID,
                amount: DRINKING_MODIFIER_AMOUNT,
                operation: AttributeModifierOperation::AddValue,
            },
            false,
        );
        *was_drinking = true;
    }

    /// Vanilla `Witch.aiStep` self-potion branch chain. Returns the first potion whose
    /// branch matches, like vanilla's `else if` chain.
    fn select_self_potion(&self) -> Option<PotionRef> {
        if rand::random::<f32>() < 0.15
            && self.is_eye_in_water()
            && !self.has_mob_effect(vanilla_mob_effects::WATER_BREATHING)
        {
            return Some(&vanilla_potions::WATER_BREATHING);
        }
        if rand::random::<f32>() < 0.15
            && (self.is_on_fire()
                || self.last_damage_source().is_some_and(|source| {
                    source.is(&vanilla_damage_type_tags::DamageTypeTag::IS_FIRE)
                }))
            && !self.has_mob_effect(vanilla_mob_effects::FIRE_RESISTANCE)
        {
            return Some(&vanilla_potions::FIRE_RESISTANCE);
        }
        if rand::random::<f32>() < 0.05 && self.get_health() < self.get_max_health() {
            return Some(&vanilla_potions::HEALING);
        }
        if rand::random::<f32>() < 0.5
            && !self.has_mob_effect(vanilla_mob_effects::SPEED)
            && self.target().is_some_and(|target| {
                target.distance_to_sqr(self.position()) > SPEED_POTION_DISTANCE_SQR
            })
        {
            return Some(&vanilla_potions::SWIFTNESS);
        }
        None
    }

    /// Vanilla `Witch.performRangedAttack` potion selection.
    fn select_attack_potion(&self, target: &SharedEntity, dist: f64) -> PotionRef {
        if target.as_raider().is_some() {
            // Vanilla heals a badly wounded raider, regenerates a healthier one, and
            // drops the target so the witch stops attacking its ally.
            let health = target
                .as_living_entity()
                .map_or(f32::MAX, |target| target.get_health());
            Mob::set_target(self, None);
            return if health <= RAIDER_HEALING_HEALTH {
                &vanilla_potions::HEALING
            } else {
                &vanilla_potions::REGENERATION
            };
        }

        let target_living = target.as_living_entity();
        if dist >= SLOWNESS_DISTANCE
            && !target_living
                .is_some_and(|target| target.has_mob_effect(vanilla_mob_effects::SLOWNESS))
        {
            return &vanilla_potions::SLOWNESS;
        }
        if target_living.is_some_and(|target| target.get_health() >= POISON_HEALTH)
            && !target_living
                .is_some_and(|target| target.has_mob_effect(vanilla_mob_effects::POISON))
        {
            return &vanilla_potions::POISON;
        }
        if dist <= WEAKNESS_DISTANCE
            && !target_living
                .is_some_and(|target| target.has_mob_effect(vanilla_mob_effects::WEAKNESS))
            && rand::random::<f32>() < 0.25
        {
            return &vanilla_potions::WEAKNESS;
        }

        &vanilla_potions::HARMING
    }
}

/// Vanilla `PotionContents.createItemStack(Items.POTION, potion)`.
fn potion_stack(potion: PotionRef) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::POTION);
    stack.set(
        vanilla_components::POTION_CONTENTS,
        PotionContents::empty().with_potion(RegistryReference::new(potion)),
    );
    stack
}

/// Vanilla `PotionContents.createItemStack(Items.SPLASH_POTION, potion)`.
fn splash_potion_stack(potion: PotionRef) -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::SPLASH_POTION);
    stack.set(
        vanilla_components::POTION_CONTENTS,
        PotionContents::empty().with_potion(RegistryReference::new(potion)),
    );
    stack
}

impl Entity for WitchEntity {
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

    fn save_additional(&self, nbt: &mut simdnbt::owned::NbtCompound) {
        self.save_mob(nbt);
        // Vanilla `Witch` adds no NBT of its own; only the inherited raider fields
        // (`Raider.addAdditionalSaveData`) are saved.
        self.raider_base.save(nbt);
    }

    fn load_additional(&self, nbt: simdnbt::borrow::NbtCompound<'_, '_>) {
        self.load_mob(nbt);
        if let Some(raid_id) = self.raider_base.load(&nbt) {
            self.rejoin_saved_raid(raid_id);
        }
    }
}

/// [`LivingEntity::get_damage_after_magic_absorb`] default body, inlined: a UFCS call
/// from an override would resolve back to that override and recurse.
fn base_damage_after_magic_absorb(
    entity: &WitchEntity,
    source: &DamageSource,
    mut damage: f32,
) -> f32 {
    if source.is(&vanilla_damage_type_tags::DamageTypeTag::BYPASSES_EFFECTS) {
        return damage;
    }

    if !source.is(&vanilla_damage_type_tags::DamageTypeTag::BYPASSES_RESISTANCE)
        && let Some(resistance) = entity.mob_effect(vanilla_mob_effects::RESISTANCE)
    {
        let absorb_value = (resistance.amplifier() + 1) * 5;
        let absorb = 25 - absorb_value;
        let old_damage = damage;
        damage = (damage * absorb as f32 / 25.0).max(0.0);
        if (0.0..f32::MAX).contains(&(old_damage - damage)) {
            let stats_to_award = ((old_damage - damage) * 10.0).round() as i32;
            if let Some(player) = entity.as_player() {
                player.award_custom_stat_with_count(
                    &vanilla_custom_stats::DAMAGE_RESISTED,
                    stats_to_award,
                );
            } else if let Some(causer) = source.causing_entity()
                && let Some(player) = causer.as_player()
            {
                player.award_custom_stat_with_count(
                    &vanilla_custom_stats::DAMAGE_DEALT_RESISTED,
                    stats_to_award,
                );
            }
        }
    }

    if damage <= 0.0 {
        return 0.0;
    }
    if source.is(&vanilla_damage_type_tags::DamageTypeTag::BYPASSES_ENCHANTMENTS) {
        return damage;
    }

    let enchantment_armor = get_damage_protection(entity, source);
    if enchantment_armor > 0.0 {
        damage = get_damage_after_magic_absorb(damage, enchantment_armor);
    }
    damage
}

impl LivingEntity for WitchEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self
            .entity_data
            .lock()
            .raider()
            .mob()
            .living_entity()
            .health
            .get()
    }

    fn set_health(&self, health: f32) {
        let clamped = health.clamp(0.0, self.get_max_health());
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WITCH_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WITCH_DEATH)
    }

    /// Vanilla `Witch.getDamageAfterMagicAbsorb`: self-inflicted damage is nullified and
    /// `#witch_resistant_to` damage is reduced to 15%.
    fn get_damage_after_magic_absorb(&self, source: &DamageSource, damage: f32) -> f32 {
        // Vanilla `Witch.getDamageAfterMagicAbsorb` starts from
        // `super.getDamageAfterMagicAbsorb`; a UFCS call to the `LivingEntity` default body
        // would resolve back to this override and recurse, so the base body is reached
        // through the inlined helper below (same approach as `Camel.actually_hurt`).
        let mut damage = base_damage_after_magic_absorb(self, source, damage);
        if source
            .causing_entity()
            .is_some_and(|causer| causer.id() == self.id())
        {
            damage = 0.0;
        }
        if source.is(&vanilla_damage_type_tags::DamageTypeTag::WITCH_RESISTANT_TO) {
            damage *= 0.15;
        }
        damage
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        // Vanilla `Witch.aiStep` runs its own logic before `super.aiStep()` and guards it
        // with `!level.isClientSide()`; Steel has no client side, so only `isAlive` remains.
        if Entity::is_alive(self) {
            self.update_witch_target_state();
            self.tick_potion_use();
            if rand::random::<f32>() < 7.5E-4 {
                self.broadcast_entity_event(
                    steel_utils::entity_events::EntityStatus::WitchHatMagic,
                );
            }
        }
        Raider::raider_ai_step(self, entity);
        Monster::monster_ai_step(self, entity)
    }

    fn on_death(&self, source: &DamageSource) {
        Raider::raider_on_death(self, source);
    }
}

impl Mob for WitchEntity {
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
        Some(&sound_events::ENTITY_WITCH_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().raider().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .mob_flags
            .set(flags);
    }

    fn requires_custom_persistence(&self) -> bool {
        Raider::raider_requires_custom_persistence(self)
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        Raider::raider_remove_when_far_away(self, dist_sqr)
    }
}

impl Monster for WitchEntity {}

impl PathfinderMob for WitchEntity {}

impl RangedAttackMob for WitchEntity {
    /// Vanilla `Witch.performRangedAttack`: throw a splash potion at `target`.
    fn perform_ranged_attack(&self, target: &SharedEntity, _power: f32) -> bool {
        // Vanilla throws nothing while drinking a potion.
        if self.is_drinking_potion() {
            return false;
        }
        let Some(world) = self.level() else {
            return false;
        };
        let Some(owner) = world.get_entity_by_id(self.id()) else {
            return false;
        };

        let target_movement = target.velocity();
        let xd = target.position().x + target_movement.x - self.position().x;
        let yd = target.get_eye_y() - 1.1 - self.position().y;
        let zd = target.position().z + target_movement.z - self.position().z;
        let dist = (xd * xd + zd * zd).sqrt();

        let potion = self.select_attack_potion(target, dist);
        let mut stack = splash_potion_stack(potion);
        let entity =
            ThrownSplashPotionEntity::new_from_owner(&world, &owner, stack.copy_with_count(1));

        let velocity = if dist <= 2.0 {
            CLOSE_THROW_VELOCITY
        } else {
            FAR_THROW_VELOCITY
        };
        let spawned = spawn_projectile(&world, entity, &mut stack, |projectile| {
            projectile.shoot(
                DVec3::new(xd, yd + dist * 0.2, zd),
                velocity,
                RANGED_ATTACK_UNCERTAINTY,
            );
        });

        self.play_sound(
            &sound_events::ENTITY_WITCH_THROW,
            1.0,
            0.8 + rand::random::<f32>() * 0.4,
        );
        spawned.is_some()
    }
}

impl Raider for WitchEntity {
    fn raider_base(&self) -> &RaiderBase {
        &self.raider_base
    }

    fn is_celebrating(&self) -> bool {
        *self.entity_data.lock().raider().is_celebrating.get()
    }

    fn set_celebrating(&self, celebrating: bool) {
        self.entity_data
            .lock()
            .raider_mut()
            .is_celebrating
            .set(celebrating);
    }

    /// Vanilla `Witch.applyRaidBuffs` is empty: a witch receives no raid equipment or
    /// enchantments.
    fn apply_raid_buffs(
        &self,
        _world: &Arc<World>,
        _raid: &RaidSnapshot,
        _wave: i32,
        _is_captain: bool,
    ) {
    }

    /// Vanilla `Witch.canBeLeader`.
    fn can_be_leader(&self) -> bool {
        false
    }

    fn celebrate_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_WITCH_CELEBRATE
    }
}
