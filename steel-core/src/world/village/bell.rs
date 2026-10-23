//! Bell reactions shared by villagers (vanilla `BellBlockEntity` villager
//! notification).
//!
//! Vanilla `BellBlockEntity.updateEntities` records `HEARD_BELL_TIME = gameTime` on every
//! living entity within 32 blocks of a rung bell. Everything else about the bell is
//! driven by the villager brain: `ReactToBell` reads that memory and switches to the HIDE
//! activity (or leaves the activity alone while a raid is active), and `WakeUp` in the
//! CORE package stops sleeping once REST is no longer the active activity.
//!
//! A sleeping villager therefore wakes up on the tick after the bell ring, exactly like
//! vanilla; the bell itself never calls `stopSleeping`.

use steel_registry::vanilla_entities;
use steel_utils::{BlockPos, WorldAabb};

use crate::entity::Mob;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::module_type::HEARD_BELL_TIME;
use crate::world::World;

/// Vanilla `BellBlockEntity.HEAR_BELL_RADIUS`: entities this close to the bell hear it.
pub const HEAR_BELL_RADIUS: f64 = 32.0;
/// Vanilla `BellBlockEntity.SEARCH_RADIUS`: the bell caches entities this close.
pub const SEARCH_RADIUS: f64 = 48.0;

/// Records `HEARD_BELL_TIME` on every entity in `entities` within [`HEAR_BELL_RADIUS`]
/// of the bell at `pos` (vanilla `BellBlockEntity.updateEntities`).
///
/// Entities without a brain are skipped: in vanilla every living entity has one, while in
/// Steel only brain mobs (villagers, iron golems, …) do.
pub fn notify_heard_bell(world: &World, pos: BlockPos, entities: &[SharedEntity]) {
    let game_time = world.game_time();

    for entity in entities {
        if !entity.is_alive() || entity.is_removed() {
            continue;
        }
        if !pos.closer_to_center_than(entity.position(), HEAR_BELL_RADIUS) {
            continue;
        }
        let Some(brain) = entity.as_mob().and_then(Mob::brain) else {
            continue;
        };
        brain.lock().set_memory(&HEARD_BELL_TIME, Some(game_time));
    }
}

/// Wakes every sleeping villager within `radius` of `center`, returning how many were
/// woken.
///
/// Vanilla has no such call: a sleeping villager wakes because the bell sets
/// `HEARD_BELL_TIME`, `ReactToBell` switches it away from REST, and `WakeUp` then calls
/// `stopSleeping`. Raids start long after the bell ring, so the raid side calls this
/// directly to stand villagers up in the same tick the raid needs them (vanilla reaches
/// the same state through the activity switch, one tick later).
pub fn wake_sleeping_villagers(world: &World, center: BlockPos, radius: f64) -> usize {
    let bounds = WorldAabb::new(
        f64::from(center.x()),
        f64::from(center.y()),
        f64::from(center.z()),
        f64::from(center.x() + 1),
        f64::from(center.y() + 1),
        f64::from(center.z() + 1),
    )
    .inflate(radius);

    let mut woken = 0;
    for entity in world.get_entities_in_aabb(&bounds) {
        if entity.entity_type() != &vanilla_entities::VILLAGER {
            continue;
        }
        let Some(living) = entity.as_living_entity() else {
            continue;
        };
        if !living.is_sleeping() {
            continue;
        }
        living.stop_sleeping();
        woken += 1;
    }

    woken
}
