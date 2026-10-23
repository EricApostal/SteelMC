//! Vanilla `ValidateNearbyPoi`.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::{BlockPos, GlobalPos, WorldAabb};

use super::poi::{self, PoiPredicate};
use super::villager;
use crate::entity::LivingEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{MemoryModuleType, MemoryModuleTypeRef};
use crate::world::World;

/// Vanilla `ValidateNearbyPoi.MAX_DISTANCE`.
const MAX_DISTANCE: f64 = 16.0;

/// Vanilla `ValidateNearbyPoi`: drops a POI memory whose POI vanished or whose bed is taken.
///
/// Steel has one loaded `World` per dimension, so the vanilla `poiLevel` lookup is
/// the dimension equality check plus `ctx.level` itself.
pub struct ValidateNearbyPoi {
    poi_type: PoiPredicate,
    memory: &'static MemoryModuleType<GlobalPos>,
}

impl ValidateNearbyPoi {
    /// Vanilla `ValidateNearbyPoi.create(poiType, memoryType)`.
    #[must_use]
    pub fn create(
        poi_type: PoiPredicate,
        memory: &'static MemoryModuleType<GlobalPos>,
    ) -> OneShot<Self> {
        let required: Vec<MemoryModuleTypeRef> = vec![memory];
        OneShot::new(Self { poi_type, memory }).with_required_memories(required)
    }
}

impl Trigger for ValidateNearbyPoi {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(global_pos) = ctx.get_memory(self.memory) else {
            return false;
        };
        let pos = global_pos.pos;
        if global_pos.dimension != ctx.level.key
            || !pos.closer_to_center_than(ctx.body.position(), MAX_DISTANCE)
        {
            return false;
        }

        if !poi::exists(ctx.level, pos, self.poi_type) {
            ctx.erase_memory(self.memory);
        } else if bed_is_occupied(ctx, pos) {
            ctx.erase_memory(self.memory);
            if !bed_is_occupied_by_villager(ctx.level, pos) {
                poi::release(ctx.level, pos);
            }
        }

        true
    }
}

/// Vanilla `ValidateNearbyPoi.bedIsOccupied`.
fn bed_is_occupied(ctx: &BehaviorContext<'_>, pos: BlockPos) -> bool {
    let state = ctx.level.get_block_state(pos);
    state
        .get_block()
        .has_tag(&BlockTag::VILLAGERS_CAN_SLEEP_ON_BED)
        && state.get_value(&BlockStateProperties::OCCUPIED)
        && !ctx
            .body
            .as_living_entity()
            .is_some_and(LivingEntity::is_sleeping)
}

/// Vanilla `ValidateNearbyPoi.bedIsOccupiedByVillager`.
fn bed_is_occupied_by_villager(level: &World, pos: BlockPos) -> bool {
    let (x, y, z) = (f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
    let aabb = WorldAabb::new(x, y, z, x + 1.0, y + 1.0, z + 1.0);
    level.get_entities_in_aabb(&aabb).iter().any(|entity| {
        villager::as_villager(entity).is_some()
            && entity
                .as_living_entity()
                .is_some_and(LivingEntity::is_sleeping)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{
        REGISTRY, RegistryExt as _, init_vanilla_registry, vanilla_entities, vanilla_poi_types,
    };

    use super::*;
    use crate::entity::SharedEntity;
    use crate::entity::ai::brain::behavior::BehaviorControl as _;
    use crate::entity::ai::brain::memory::MemoryStatus;
    use crate::entity::ai::brain::memory::MemoryStore;
    use crate::entity::ai::brain::memory::module_type as mem;
    use crate::entity::entities::PigEntity;
    use crate::test_support::fresh_test_world;

    fn home_position() -> BlockPos {
        BlockPos::new(4, 64, 4)
    }

    fn body_near(pos: BlockPos) -> SharedEntity {
        Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()),
                f64::from(pos.z()) + 0.5,
            ),
            Weak::new(),
        ))
    }

    fn run_trigger(world: &Arc<World>, body: &SharedEntity) -> MemoryStore {
        let mut behavior =
            ValidateNearbyPoi::create(PoiPredicate::Type(&vanilla_poi_types::HOME), &mem::HOME);
        let mut memories = MemoryStore::new();
        memories.register(&mem::HOME);
        memories.set(
            &mem::HOME,
            GlobalPos::new(world.key.clone(), home_position()),
        );

        let mut ctx = BehaviorContext::new(&mut memories, world, body, 0);
        assert!(behavior.try_start(&mut ctx));
        memories
    }

    #[test]
    fn erases_memory_when_the_poi_is_gone() {
        init_vanilla_registry();
        let world = fresh_test_world("validate_nearby_poi_missing");
        let body = body_near(home_position());

        let memories = run_trigger(&world.world, &body);
        assert!(!memories.check_key(mem::HOME.key(), MemoryStatus::ValuePresent));
    }

    #[test]
    fn keeps_memory_when_the_poi_exists() {
        init_vanilla_registry();
        let world = fresh_test_world("validate_nearby_poi_present");
        let home_type = REGISTRY
            .poi_types
            .id_from_key(&vanilla_poi_types::HOME.key)
            .expect("home poi type is registered");
        world
            .world
            .poi_storage
            .lock()
            .add(home_position(), home_type, 1);
        let body = body_near(home_position());

        let memories = run_trigger(&world.world, &body);
        assert!(memories.get(&mem::HOME).is_some());
    }
}
