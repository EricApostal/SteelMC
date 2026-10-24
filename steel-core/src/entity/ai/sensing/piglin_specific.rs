//! Vanilla `PiglinSpecificSensor`, plus the `PiglinAi` predicates it consults.

use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_utils::{BlockPos, BlockStateId};

use crate::entity::LivingEntity;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::ai::brain::memory::{
    MemoryModuleType, MemoryModuleTypeRef, MemoryStore, MemoryValueType, module_type as mem,
};
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::sensing::nearest_visible_living::get_visible_entities;
use crate::entity::ai::sensing::villager::{find_nearest_block_by_manhattan, is_adult};
use crate::world::World;

/// Vanilla `SensorType.PIGLIN_SPECIFIC_SENSOR`.
pub(crate) static PIGLIN_SPECIFIC_SENSOR: SensorType =
    SensorType::new(create, "piglin_specific_sensor");

/// Vanilla `SensorType.PIGLIN_SPECIFIC_SENSOR` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(PiglinSpecificSensor)
}

/// Vanilla `PiglinSpecificSensor`: refreshes the piglin's hunting, nemesis,
/// gold-armor and repellent memories.
pub(crate) struct PiglinSpecificSensor;

impl BrainSensor for PiglinSpecificSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
            &mem::NEAREST_LIVING_ENTITIES,
            &mem::NEAREST_VISIBLE_NEMESIS,
            &mem::NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD,
            &mem::NEAREST_PLAYER_HOLDING_WANTED_ITEM,
            &mem::NEAREST_VISIBLE_HUNTABLE_HOGLIN,
            &mem::NEAREST_VISIBLE_BABY_HOGLIN,
            &mem::NEAREST_VISIBLE_ADULT_PIGLINS,
            &mem::NEARBY_ADULT_PIGLINS,
            &mem::VISIBLE_ADULT_PIGLIN_COUNT,
            &mem::VISIBLE_ADULT_HOGLIN_COUNT,
            &mem::NEAREST_REPELLENT,
        ];
        REQUIRES
    }

    fn sense(&mut self, level: &World, body: &SharedEntity, memories: &mut MemoryStore) {
        // Vanilla `brain.setMemory(NEAREST_REPELLENT, findNearestRepellent(level, body))`.
        set_optional(
            memories,
            &mem::NEAREST_REPELLENT,
            find_nearest_repellent(level, body),
        );

        let mut nemesis: Option<SharedEntity> = None;
        let mut huntable_hoglin: Option<SharedEntity> = None;
        let mut baby_hoglin: Option<SharedEntity> = None;
        let mut baby_piglin: Option<SharedEntity> = None;
        let mut zombified: Option<SharedEntity> = None;
        let mut player_not_wearing_gold: Option<SharedEntity> = None;
        let mut player_holding_wanted_item: Option<SharedEntity> = None;
        let mut visible_adult_hoglin_count = 0;
        let mut visible_adult_piglins: Vec<SharedEntity> = Vec::new();

        let visible =
            get_visible_entities(memories).unwrap_or_else(NearestVisibleLivingEntities::empty);

        // Vanilla `visibleLivingEntities.findAll(ignored -> true)`: every visible
        // entity in cache order, keeping the first match of each memory.
        for entity in visible.find(|_| true) {
            if entity.entity_type().key == vanilla_entities::HOGLIN.key {
                if is_baby(entity) {
                    if baby_hoglin.is_none() {
                        baby_hoglin = Some(SharedEntity::clone(entity));
                    }
                } else if is_adult(entity) {
                    visible_adult_hoglin_count += 1;
                    if huntable_hoglin.is_none() && can_be_hunted(entity) {
                        huntable_hoglin = Some(SharedEntity::clone(entity));
                    }
                }
            } else if entity.entity_type().key == vanilla_entities::PIGLIN_BRUTE.key {
                visible_adult_piglins.push(SharedEntity::clone(entity));
            } else if entity.entity_type().key == vanilla_entities::PIGLIN.key {
                if is_baby(entity) {
                    if baby_piglin.is_none() {
                        baby_piglin = Some(SharedEntity::clone(entity));
                    }
                } else if is_adult(entity) {
                    visible_adult_piglins.push(SharedEntity::clone(entity));
                }
            } else if entity.entity_type().key == vanilla_entities::PLAYER.key {
                if player_not_wearing_gold.is_none()
                    && !is_wearing_safe_armor(entity)
                    && body_can_attack(body, entity)
                {
                    player_not_wearing_gold = Some(SharedEntity::clone(entity));
                }

                if player_holding_wanted_item.is_none()
                    && !entity.is_spectator()
                    && is_player_holding_loved_item(entity)
                {
                    player_holding_wanted_item = Some(SharedEntity::clone(entity));
                }
            } else {
                let is_nemesis_type = entity.entity_type().key
                    == vanilla_entities::WITHER_SKELETON.key
                    || entity.entity_type().key == vanilla_entities::WITHER.key;
                // Vanilla `!nemesis.isEmpty() || (!(WitherSkeleton) && !(WitherBoss))`:
                // once a nemesis is found, later nemesis mobs fall through to the
                // zombified check.
                if nemesis.is_some() || !is_nemesis_type {
                    if zombified.is_none() && is_zombified(entity) {
                        zombified = Some(SharedEntity::clone(entity));
                    }
                } else {
                    nemesis = Some(SharedEntity::clone(entity));
                }
            }
        }

        // Vanilla `PiglinAi.findNearbyAdultPiglins(brain)` reads the refreshed
        // `NEAREST_LIVING_ENTITIES` memory (not the visible cache).
        let nearby_adult_piglins = find_nearby_adult_piglins(memories);

        set_optional(memories, &mem::NEAREST_VISIBLE_NEMESIS, nemesis);
        set_optional(
            memories,
            &mem::NEAREST_VISIBLE_HUNTABLE_HOGLIN,
            huntable_hoglin,
        );
        set_optional(memories, &mem::NEAREST_VISIBLE_BABY_HOGLIN, baby_hoglin);
        set_optional(memories, &mem::NEAREST_VISIBLE_ZOMBIFIED, zombified);
        set_optional(
            memories,
            &mem::NEAREST_TARGETABLE_PLAYER_NOT_WEARING_GOLD,
            player_not_wearing_gold,
        );
        set_optional(
            memories,
            &mem::NEAREST_PLAYER_HOLDING_WANTED_ITEM,
            player_holding_wanted_item,
        );
        let visible_adult_piglin_count = visible_adult_piglins.len() as i32;
        memories.set(&mem::NEARBY_ADULT_PIGLINS, nearby_adult_piglins);
        memories.set(&mem::NEAREST_VISIBLE_ADULT_PIGLINS, visible_adult_piglins);
        memories.set(&mem::VISIBLE_ADULT_PIGLIN_COUNT, visible_adult_piglin_count);
        memories.set(&mem::VISIBLE_ADULT_HOGLIN_COUNT, visible_adult_hoglin_count);
    }
}

/// Vanilla `Brain.setMemory(type, Optional)`: an empty optional erases the memory.
fn set_optional<T: MemoryValueType>(
    memories: &mut MemoryStore,
    memory: &MemoryModuleType<T>,
    value: Option<T>,
) {
    match value {
        Some(value) => memories.set(memory, value),
        None => memories.erase(memory),
    }
}

/// Vanilla `PiglinSpecificSensor.findNearestRepellent`: the first
/// `isValidRepellent` block within 8 blocks horizontally and 4 vertically.
fn find_nearest_repellent(level: &World, body: &SharedEntity) -> Option<BlockPos> {
    find_nearest_block_by_manhattan(level, body.block_position(), 8, 4, is_valid_repellent)
}

/// Vanilla `PiglinSpecificSensor.isValidRepellent`:
/// `state.is(PIGLIN_REPELLENTS)` and, for a soul campfire,
/// `CampfireBlock.isLitCampfire(state)`.
///
/// `CampfireBlock.isLitCampfire` is `hasProperty(LIT) && state.is(CAMPFIRES) &&
/// getValue(LIT)`; both leading terms are guaranteed by the `SOUL_CAMPFIRE` test, so the
/// `LIT` value alone decides (`None` when the property is absent, as `hasProperty` would).
fn is_valid_repellent(state: BlockStateId) -> bool {
    let block = state.get_block();
    if !block.has_tag(&BlockTag::PIGLIN_REPELLENTS) {
        return false;
    }

    if block.key != vanilla_blocks::SOUL_CAMPFIRE.key {
        return true;
    }

    state
        .try_get_value(&BlockStateProperties::LIT)
        .unwrap_or(false)
}

/// Vanilla `PiglinAi.isZombified`: zombified piglins and zoglins.
#[must_use]
pub(crate) fn is_zombified(entity: &SharedEntity) -> bool {
    entity.entity_type().key == vanilla_entities::ZOMBIFIED_PIGLIN.key
        || entity.entity_type().key == vanilla_entities::ZOGLIN.key
}

/// Vanilla `PiglinAi.isWearingSafeArmor`: any armor-slot stack in `PIGLIN_SAFE_ARMOR`.
#[must_use]
pub(crate) fn is_wearing_safe_armor(entity: &SharedEntity) -> bool {
    let Some(living) = entity.as_living_entity() else {
        return false;
    };

    let mut safe = false;
    for slot in EquipmentSlot::ARMOR_SLOTS {
        living.with_equipment_slot(slot, &mut |stack: &ItemStack| {
            safe |= stack.item().has_tag(&ItemTag::PIGLIN_SAFE_ARMOR);
        });
    }

    safe
}

/// Vanilla `PiglinAi.isPlayerHoldingLovedItem`:
/// `entity.is(EntityTypes.PLAYER) && entity.isHolding(PiglinAi::isLovedItem)`.
///
/// `LivingEntity.isHolding` checks the main hand and then the off hand, and
/// `PiglinAi.isLovedItem(stack)` is `stack.is(ItemTags.PIGLIN_LOVED)`.
#[must_use]
pub(crate) fn is_player_holding_loved_item(entity: &SharedEntity) -> bool {
    if entity.entity_type().key != vanilla_entities::PLAYER.key {
        return false;
    }

    let Some(living) = entity.as_living_entity() else {
        return false;
    };

    living.is_holding(&mut |stack| stack.item().has_tag(&ItemTag::PIGLIN_LOVED))
}

/// Vanilla `PiglinAi.findNearbyAdultPiglins`: the adult members of
/// `NEAREST_LIVING_ENTITIES` that are `AbstractPiglin`s.
///
/// Vanilla's `AbstractPiglin` hierarchy is exactly `Piglin` and `PiglinBrute`
/// (`ZombifiedPiglin extends Zombie`, so zombified piglins are not included);
/// `isAdult()` is `!isBaby()` for both.
#[must_use]
pub(crate) fn find_nearby_adult_piglins(memories: &MemoryStore) -> Vec<SharedEntity> {
    let Some(living_entities) = memories.get(&mem::NEAREST_LIVING_ENTITIES) else {
        return Vec::new();
    };

    let mut adult_piglins = Vec::new();
    for entity in living_entities {
        if (entity.entity_type().key == vanilla_entities::PIGLIN.key
            || entity.entity_type().key == vanilla_entities::PIGLIN_BRUTE.key)
            && is_adult(&entity)
        {
            adult_piglins.push(entity);
        }
    }

    adult_piglins
}

/// Vanilla `Hoglin.canBeHunted`: `isAdult() && !cannotBeHunted`.
///
/// Steel has no `Hoglin` entity yet, so the per-entity `cannotBeHunted` flag set by
/// `PiglinAi.dontKillAnyMoreHoglinsForAWhile` (the `HUNTED_RECENTLY` hunt cooldown)
/// has no owner: only the `isAdult()` half is observable here.
#[must_use]
pub(crate) fn can_be_hunted(hoglin: &SharedEntity) -> bool {
    is_adult(hoglin)
}

/// Vanilla `body.canAttack(entity)`: `LivingEntity.canAttack(LivingEntity)`.
fn body_can_attack(body: &SharedEntity, target: &SharedEntity) -> bool {
    let (Some(body), Some(target)) = (body.as_living_entity(), target.as_living_entity()) else {
        return false;
    };

    body.can_attack(target)
}

/// Vanilla `entity instanceof AgeableMob && entity.isBaby()`.
fn is_baby(entity: &SharedEntity) -> bool {
    entity.as_living_entity().is_some_and(LivingEntity::is_baby)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::init_vanilla_registry;

    use super::*;
    use crate::entity::entities::PigEntity;

    fn soul_campfire(lit: bool) -> BlockStateId {
        vanilla_blocks::SOUL_CAMPFIRE
            .default_state()
            .set_value(&BlockStateProperties::LIT, lit)
    }

    #[test]
    fn lit_soul_campfire_is_a_repellent() {
        init_vanilla_registry();

        assert!(is_valid_repellent(soul_campfire(true)));
    }

    #[test]
    fn unlit_soul_campfire_is_not_a_repellent() {
        init_vanilla_registry();

        assert!(!is_valid_repellent(soul_campfire(false)));
    }

    #[test]
    fn non_repellent_block_is_not_a_repellent() {
        init_vanilla_registry();

        assert!(!is_valid_repellent(vanilla_blocks::STONE.default_state()));
    }

    #[test]
    fn piglin_repellents_tag_holds_the_soul_campfire() {
        init_vanilla_registry();

        assert!(
            soul_campfire(true)
                .get_block()
                .has_tag(&BlockTag::PIGLIN_REPELLENTS)
        );
    }

    #[test]
    fn find_nearby_adult_piglins_ignores_non_piglins() {
        init_vanilla_registry();
        let pig: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            2,
            DVec3::ZERO,
            Weak::new(),
        ));

        let mut memories = MemoryStore::new();
        memories.register(&mem::NEAREST_LIVING_ENTITIES);
        memories.set(&mem::NEAREST_LIVING_ENTITIES, vec![pig]);

        assert!(find_nearby_adult_piglins(&memories).is_empty());
    }

    #[test]
    fn find_nearby_adult_piglins_without_the_memory_is_empty() {
        init_vanilla_registry();

        let mut memories = MemoryStore::new();
        memories.register(&mem::NEAREST_LIVING_ENTITIES);

        assert!(find_nearby_adult_piglins(&memories).is_empty());
    }
}
