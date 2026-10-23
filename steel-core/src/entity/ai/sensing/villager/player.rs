//! Vanilla `PlayerSensor`.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::MemoryStore;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::sensor::BrainSensor;
use crate::entity::ai::sensing::villager::{
    closer_than, follow_range, is_entity_attackable, is_entity_targetable,
};
use crate::world::World;

/// Vanilla `SensorType.NEAREST_PLAYERS` factory.
pub(crate) fn create() -> Box<dyn BrainSensor> {
    Box::new(PlayerSensor)
}

/// Vanilla `PlayerSensor`.
pub(crate) struct PlayerSensor;

impl BrainSensor for PlayerSensor {
    fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        static REQUIRES: &[MemoryModuleTypeRef] = &[
            &mem::NEAREST_PLAYERS,
            &mem::NEAREST_VISIBLE_PLAYER,
            &mem::NEAREST_VISIBLE_ATTACKABLE_PLAYER,
            &mem::NEAREST_VISIBLE_ATTACKABLE_PLAYERS,
        ];
        REQUIRES
    }

    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) {
        let Some(follow_range) = follow_range(entity) else {
            return;
        };

        let mut players: Vec<SharedEntity> = Vec::new();
        level.players.iter_players(|_uuid, player| {
            let player: SharedEntity = player.clone();
            if !player.is_spectator() && closer_than(entity, &player, follow_range) {
                players.push(player);
            }
            true
        });
        players.sort_by(|first, second| {
            entity
                .distance_to_sqr(first.position())
                .total_cmp(&entity.distance_to_sqr(second.position()))
        });

        memories.set(&mem::NEAREST_PLAYERS, players.clone());

        let attack_target = memories.get(&mem::ATTACK_TARGET);
        let visible_players: Vec<SharedEntity> = players
            .iter()
            .filter(|player| {
                is_entity_targetable(level, entity, player, follow_range, attack_target.as_ref())
            })
            .cloned()
            .collect();
        set_single_memory(memories, &mem::NEAREST_VISIBLE_PLAYER, &visible_players);

        let attackable_players: Vec<SharedEntity> = visible_players
            .iter()
            .filter(|player| {
                is_entity_attackable(level, entity, player, follow_range, attack_target.as_ref())
            })
            .cloned()
            .collect();
        memories.set(
            &mem::NEAREST_VISIBLE_ATTACKABLE_PLAYERS,
            attackable_players.clone(),
        );
        set_single_memory(
            memories,
            &mem::NEAREST_VISIBLE_ATTACKABLE_PLAYER,
            &attackable_players,
        );
    }
}

/// Vanilla `brain.setMemory(type, list.isEmpty() ? null : list.get(0))`.
fn set_single_memory(
    memories: &mut MemoryStore,
    ty: &crate::entity::ai::brain::memory::MemoryModuleType<SharedEntity>,
    entities: &[SharedEntity],
) {
    match entities.first() {
        Some(entity) => memories.set(ty, entity.clone()),
        None => memories.erase(ty),
    }
}
