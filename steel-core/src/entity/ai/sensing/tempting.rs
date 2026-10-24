//! Vanilla `TemptingSensor` and its `FOOD_TEMPTATIONS` / `FROG_TEMPTATIONS` variants.

use std::sync::Arc;

use steel_registry::REGISTRY;
use steel_registry::TaggedRegistryExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_attributes;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_utils::types::InteractionHand;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::{BrainSensor, SensorType};
use crate::entity::ai::targeting::TargetingConditions;
use crate::world::World;

/// Vanilla `SensorType.FOOD_TEMPTATIONS`.
pub(crate) static FOOD_TEMPTATIONS: SensorType = SensorType::new(create_animal, "food_temptations");

/// Vanilla `SensorType.FROG_TEMPTATIONS`.
pub(crate) static FROG_TEMPTATIONS: SensorType = SensorType::new(create_frog, "frog_temptations");

/// Vanilla `TemptingSensor.forAnimal`: the body's own `Animal.isFood` test.
pub(crate) fn create_animal() -> Box<dyn BrainSensor> {
    Box::new(TemptingSensor::new(|body, item_stack| {
        body.as_animal()
            .is_some_and(|animal| animal.is_food(item_stack))
    }))
}

/// Vanilla `SensorType.FROG_TEMPTATIONS`: `new TemptingSensor(FrogAi.getTemptations())`.
pub(crate) fn create_frog() -> Box<dyn BrainSensor> {
    Box::new(TemptingSensor::new(|_body, item_stack| {
        REGISTRY
            .items
            .is_in_tag(item_stack.item, &ItemTag::FROG_FOOD)
    }))
}

/// Vanilla `TemptingSensor.temptations` (`BiPredicate<PathfinderMob, ItemStack>`).
type Temptations = Arc<dyn Fn(&SharedEntity, &ItemStack) -> bool + Send + Sync>;

/// Vanilla `TemptingSensor`: remembers the nearest player holding a tempting item.
pub(crate) struct TemptingSensor {
    temptations: Temptations,
}

impl TemptingSensor {
    /// Vanilla `TemptingSensor(Predicate<ItemStack>)` / `TemptingSensor(BiPredicate)`.
    #[must_use]
    pub(crate) fn new(
        temptations: impl Fn(&SharedEntity, &ItemStack) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            temptations: Arc::new(temptations),
        }
    }

    /// Vanilla `TemptingSensor.TEMPT_TARGETING` with the body's `TEMPT_RANGE`.
    const fn targeting_conditions(tempt_range: f64) -> TargetingConditions {
        TargetingConditions::for_non_combat()
            .ignore_line_of_sight()
            .range(tempt_range)
    }

    /// Vanilla `playerHoldingTemptation`: main hand first, then off hand.
    fn player_holding_temptation(&self, body: &SharedEntity, player: &SharedEntity) -> bool {
        let Some(living) = player.as_living_entity() else {
            return false;
        };

        self.is_temptation(body, &living.get_item_in_hand(InteractionHand::MainHand))
            || self.is_temptation(body, &living.get_item_in_hand(InteractionHand::OffHand))
    }

    /// Vanilla `isTemptation(mob, itemStack)`.
    fn is_temptation(&self, body: &SharedEntity, item_stack: &ItemStack) -> bool {
        (self.temptations)(body, item_stack)
    }
}

impl BrainSensor for TemptingSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[&mem::TEMPTING_PLAYER];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        // Vanilla `Sensor<PathfinderMob>`: only pathfinder mobs are tempted.
        if entity.as_pathfinder_mob().is_none() {
            return;
        }

        let Some(tempt_range) = entity.as_living_entity().and_then(|living| {
            living
                .attributes()
                .lock()
                .get_value(vanilla_attributes::TEMPT_RANGE)
        }) else {
            return;
        };

        let targeting = Self::targeting_conditions(tempt_range);
        let body_living = entity.as_living_entity();

        let mut players: Vec<SharedEntity> = Vec::new();
        level.players.iter_players(|_uuid, player| {
            let player = Arc::clone(player);
            let player: SharedEntity = player;

            if player.is_spectator() {
                return true;
            }

            let Some(player_living) = player.as_living_entity() else {
                return true;
            };

            if !targeting.test(level, body_living, player_living) {
                return true;
            }

            if !self.player_holding_temptation(entity, &player) {
                return true;
            }

            if entity.has_passenger(player.as_ref()) {
                return true;
            }

            players.push(player);
            true
        });

        players.sort_by(|first, second| {
            entity
                .distance_to_sqr(first.position())
                .total_cmp(&entity.distance_to_sqr(second.position()))
        });

        match players.first() {
            Some(player) => memories.set(&mem::TEMPTING_PLAYER, SharedEntity::clone(player)),
            None => memories.erase(&mem::TEMPTING_PLAYER),
        }
    }
}
