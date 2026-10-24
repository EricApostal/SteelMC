//! Death-loot plumbing that needs the enchantment runtime.
//!
//! The death path itself lives on `LivingEntity`; this module holds the two
//! pieces vanilla factors out of it, plus the world view loot tables need:
//!
//! - [`drop_mob_equipment`] — vanilla `Mob.dropCustomDeathLoot`'s equipment
//!   loop, including `minecraft:equipment_drops` and
//!   `minecraft:prevent_equipment_drop` effects.
//! - [`experience_reward_with_enchantments`] — vanilla
//!   `LivingEntity.getExperienceReward`, i.e. the base reward folded through the
//!   killer's `minecraft:mob_experience` effects.
//! - [`loot_level_view`] — the
//!   [`LevelView`](steel_registry::advancement::predicates::LevelView) a loot table needs for
//!   `minecraft:location_check`.

use std::sync::Arc;

use rand::{Rng, RngExt as _};
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_utils::BlockPos;

use crate::enchantment_helper;
use crate::entity::damage::DamageSource;
use crate::entity::mob::Mob;
use crate::entity::{Entity, LivingEntity};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::advancement::triggers::WorldLevelView;
use crate::world::World;

/// Returns the [`LevelView`] a loot table reads for `minecraft:location_check`
/// when it is evaluated in `world` at `origin`.
///
/// Vanilla's loot parameters carry the `ServerLevel` itself, so every location
/// sub-predicate (position, biomes, structures, dimension, light, block, fluid,
/// `smokey`, `can_see_sky`) resolves against the live world.
#[must_use]
pub(crate) const fn loot_level_view(
    world: &Arc<World>,
    origin: (f64, f64, f64),
) -> WorldLevelView<'_> {
    WorldLevelView::new(world, BlockPos::containing(origin.0, origin.1, origin.2))
}

/// Vanilla `LivingEntity.getExperienceReward`: the base reward folded through the
/// killer's `minecraft:mob_experience` enchantment effects.
///
/// Vanilla's `EnchantmentHelper.processMobExperience` only consults living
/// killers; every other killer (or no killer at all) leaves the amount
/// unchanged.
pub(crate) fn experience_reward_with_enchantments(
    victim: &dyn LivingEntity,
    killer: Option<&dyn Entity>,
) -> i32 {
    let base = victim.base_experience_reward();
    killer.map_or(base, |killer| {
        enchantment_helper::process_mob_experience(killer, victim, base)
    })
}

/// Runs vanilla `Mob.dropCustomDeathLoot`'s equipment loop: drops each equipment
/// slot whose drop chance passes, folded through the killer's
/// `minecraft:equipment_drops` enchantment effects, skipping stacks with
/// `minecraft:prevent_equipment_drop`.
///
/// `killed_by_player` mirrors vanilla's `lastHurtByPlayerMemoryTime > 0`; a slot
/// marked preserved (`dropChances.isPreserved`) drops regardless of it.
pub(crate) fn drop_mob_equipment<M: Mob + ?Sized>(
    mob: &M,
    source: &DamageSource,
    killed_by_player: bool,
) {
    if mob.level().is_none() {
        return;
    }

    for slot in EquipmentSlot::ALL {
        let drop_chance = mob.equipment_drop_chance(slot);
        // Vanilla skips zero-chance slots before any enchantment is consulted,
        // so a base chance of 0 can never be raised.
        if drop_chance == 0.0 {
            continue;
        }
        let preserve = mob.is_equipment_drop_preserved(slot);

        // Vanilla hands `EnchantmentHelper.processEquipmentDropChance` the
        // *killer* (`source.getEntity()`), not the dying mob, so only the
        // killer's equipment can raise the chance.
        let drop_chance = match source
            .causing_entity()
            .and_then(|entity| entity.as_living_entity())
        {
            Some(killer) => {
                enchantment_helper::modify_equipment_drop_chance(killer, source, drop_chance)
            }
            None => drop_chance,
        };

        // Vanilla: `(killedByPlayer || preserve) && random.nextFloat() < dropChance`.
        if !(killed_by_player || preserve) || rand::random::<f32>() >= drop_chance {
            continue;
        }

        let mut item_stack = {
            let mut equipment = mob.living_base().equipment().lock();
            let item_stack = equipment.get_ref(slot);
            if item_stack.is_empty()
                || item_stack
                    .has_enchantment_effect(EnchantmentEffectComponent::PreventEquipmentDrop)
            {
                continue;
            }

            equipment.take(slot)
        };

        if !preserve && item_stack.is_damageable_item() {
            let damage = dropped_equipment_damage(item_stack.get_max_damage(), &mut rand::rng());
            item_stack.set_damage_value(damage);
        }

        mob.spawn_at_location(item_stack, 0.0);
    }
}

/// Vanilla's dropped-equipment wear:
/// `maxDamage - random.nextInt(1 + random.nextInt(Math.max(maxDamage - 3, 1)))`.
fn dropped_equipment_damage(max_damage: i32, random: &mut impl Rng) -> i32 {
    let bound = (max_damage - 3).max(1);
    let inner = random.random_range(0..bound);
    let lost = random.random_range(0..=inner);
    max_damage - lost
}
