//! Vanilla `Mob`'s default spawn-equipment passes.
//!
//! Vanilla implements the humanoid armor roll and the `mob_spawn_equipment`
//! enchantment pass as protected methods on `Mob`, which every hostile
//! humanoid inherits and calls from `finalizeSpawn`. Steel's mobs are
//! standalone structs, so the two passes live here as free functions over
//! `&dyn Mob` and each mob calls the ones vanilla's class hierarchy reaches.

use steel_registry::enchantment::provider;
use steel_registry::enchantment::table::EnchantmentRandom;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use steel_utils::random::Random;
use steel_utils::types::Difficulty;

use crate::entity::Mob;
use crate::inventory::equipment::EquipmentSlot;

/// Vanilla `Mob.MAX_WEARING_ARMOR_CHANCE`.
const MAX_WEARING_ARMOR_CHANCE: f32 = 0.15;
/// Vanilla `Mob.WEARING_ARMOR_UPGRADE_MATERIAL_CHANCE`.
const WEARING_ARMOR_UPGRADE_MATERIAL_CHANCE: f32 = 0.1087;
/// Vanilla `Mob.WEARING_ARMOR_UPGRADE_MATERIAL_ATTEMPTS`.
const WEARING_ARMOR_UPGRADE_MATERIAL_ATTEMPTS: i32 = 3;
/// Vanilla `Mob.EQUIPMENT_POPULATION_ORDER`.
const EQUIPMENT_POPULATION_ORDER: [EquipmentSlot; 4] = [
    EquipmentSlot::Head,
    EquipmentSlot::Chest,
    EquipmentSlot::Legs,
    EquipmentSlot::Feet,
];
/// Vanilla `Mob.enchantSpawnedWeapon` chance.
const WEAPON_ENCHANT_CHANCE: f32 = 0.25;
/// Vanilla `Mob.enchantSpawnedArmor` chance.
const ARMOR_ENCHANT_CHANCE: f32 = 0.5;
/// Vanilla `EnchantmentHelper` key `minecraft:mob_spawn_equipment`.
const MOB_SPAWN_EQUIPMENT_KEY: &str = "mob_spawn_equipment";

/// Vanilla `Mob.populateDefaultEquipmentSlots`: the difficulty-scaled armor roll.
pub(crate) fn populate_default_equipment_slots(
    mob: &dyn Mob,
    random: &mut impl Random,
    difficulty_modifier: f32,
) {
    if random.next_f32() >= MAX_WEARING_ARMOR_CHANCE * difficulty_modifier {
        return;
    }

    let mut armor_type = random.next_i32_bounded(3);
    for _ in 0..WEARING_ARMOR_UPGRADE_MATERIAL_ATTEMPTS {
        if random.next_f32() < WEARING_ARMOR_UPGRADE_MATERIAL_CHANCE {
            armor_type += 1;
        }
    }

    let partial_chance = if mob
        .level()
        .is_some_and(|world| world.difficulty() == Difficulty::Hard)
    {
        0.1
    } else {
        0.25
    };

    let mut first = true;
    for slot in EQUIPMENT_POPULATION_ORDER {
        if !first && random.next_f32() < partial_chance {
            break;
        }
        first = false;

        if mob.equipment_in_slot(slot).is_empty()
            && let Some(item) = equipment_for_slot(slot, armor_type)
        {
            mob.living_base()
                .equipment()
                .lock()
                .set(slot, ItemStack::new(item));
        }
    }
}

/// Vanilla `Mob.getEquipmentForSlot`.
#[must_use]
fn equipment_for_slot(slot: EquipmentSlot, armor_type: i32) -> Option<ItemRef> {
    let (leather, copper, golden, chainmail, iron, diamond) = match slot {
        EquipmentSlot::Head => (
            &vanilla_items::LEATHER_HELMET,
            &vanilla_items::COPPER_HELMET,
            &vanilla_items::GOLDEN_HELMET,
            &vanilla_items::CHAINMAIL_HELMET,
            &vanilla_items::IRON_HELMET,
            &vanilla_items::DIAMOND_HELMET,
        ),
        EquipmentSlot::Chest => (
            &vanilla_items::LEATHER_CHESTPLATE,
            &vanilla_items::COPPER_CHESTPLATE,
            &vanilla_items::GOLDEN_CHESTPLATE,
            &vanilla_items::CHAINMAIL_CHESTPLATE,
            &vanilla_items::IRON_CHESTPLATE,
            &vanilla_items::DIAMOND_CHESTPLATE,
        ),
        EquipmentSlot::Legs => (
            &vanilla_items::LEATHER_LEGGINGS,
            &vanilla_items::COPPER_LEGGINGS,
            &vanilla_items::GOLDEN_LEGGINGS,
            &vanilla_items::CHAINMAIL_LEGGINGS,
            &vanilla_items::IRON_LEGGINGS,
            &vanilla_items::DIAMOND_LEGGINGS,
        ),
        EquipmentSlot::Feet => (
            &vanilla_items::LEATHER_BOOTS,
            &vanilla_items::COPPER_BOOTS,
            &vanilla_items::GOLDEN_BOOTS,
            &vanilla_items::CHAINMAIL_BOOTS,
            &vanilla_items::IRON_BOOTS,
            &vanilla_items::DIAMOND_BOOTS,
        ),
        _ => return None,
    };

    match armor_type {
        0 => Some(leather),
        1 => Some(copper),
        2 => Some(golden),
        3 => Some(chainmail),
        4 => Some(iron),
        5 => Some(diamond),
        _ => None,
    }
}

/// Vanilla `Mob.populateDefaultEquipmentEnchantments`: the weapon roll, then
/// every humanoid armor slot.
pub(crate) fn populate_default_equipment_enchantments(
    mob: &dyn Mob,
    random: &mut (impl Random + EnchantmentRandom),
    difficulty_modifier: f32,
) {
    enchant_spawned_weapon(mob, random, difficulty_modifier);
    enchant_spawned_armor(mob, random, difficulty_modifier);
}

/// Vanilla `Mob.enchantSpawnedWeapon`.
pub(crate) fn enchant_spawned_weapon(
    mob: &dyn Mob,
    random: &mut (impl Random + EnchantmentRandom),
    difficulty_modifier: f32,
) {
    enchant_spawned_equipment(
        mob,
        EquipmentSlot::MainHand,
        random,
        WEAPON_ENCHANT_CHANCE,
        difficulty_modifier,
    );
}

/// Vanilla `Mob.populateDefaultEquipmentEnchantments`' armor loop, in vanilla
/// `EquipmentSlot.values()` order (feet, legs, chest, head).
pub(crate) fn enchant_spawned_armor(
    mob: &dyn Mob,
    random: &mut (impl Random + EnchantmentRandom),
    difficulty_modifier: f32,
) {
    for slot in [
        EquipmentSlot::Feet,
        EquipmentSlot::Legs,
        EquipmentSlot::Chest,
        EquipmentSlot::Head,
    ] {
        enchant_spawned_equipment(mob, slot, random, ARMOR_ENCHANT_CHANCE, difficulty_modifier);
    }
}

/// Vanilla `Mob.enchantSpawnedEquipment`.
fn enchant_spawned_equipment(
    mob: &dyn Mob,
    slot: EquipmentSlot,
    random: &mut (impl Random + EnchantmentRandom),
    chance: f32,
    difficulty_modifier: f32,
) {
    let mut item_stack = mob.equipment_in_slot(slot);
    if item_stack.is_empty() || random.next_f32() >= chance * difficulty_modifier {
        return;
    }

    provider::enchant_item_from_provider(
        &mut item_stack,
        &steel_utils::Identifier::vanilla_static(MOB_SPAWN_EQUIPMENT_KEY),
        difficulty_modifier,
        &mut *random,
    );
    mob.living_base().equipment().lock().set(slot, item_stack);
}
