//! Vanilla `ResetProfession`.

use steel_registry::entity_data::VillagerData;
use steel_registry::vanilla_villager_professions;
use steel_registry::{REGISTRY, RegistryExt as _};

use super::villager;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::entities::mobs::npc::villager::VillagerDataHolder as _;

/// Vanilla `ResetProfession`: a jobless, xp-less, level-1 villager loses its
/// profession (back to `minecraft:none`).
pub struct ResetProfession;

impl ResetProfession {
    /// Vanilla `ResetProfession.create()`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![&mem::JOB_SITE])
    }
}

impl Trigger for ResetProfession {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::JOB_SITE, MemoryStatus::ValueAbsent) {
            return false;
        }

        let Some(v) = villager::villager(ctx.level, ctx.body) else {
            return false;
        };

        let data = v.villager_data();
        let can_be_fired = !is_profession(data.profession, &vanilla_villager_professions::NONE.key)
            && !is_profession(data.profession, &vanilla_villager_professions::NITWIT.key);

        if can_be_fired && v.villager_xp() == 0 && data.level <= 1 {
            v.set_villager_data(VillagerData::new(
                data.villager_type,
                none_profession_index(),
                data.level,
            ));
            return true;
        }

        false
    }
}

fn is_profession(id: i32, key: &steel_utils::Identifier) -> bool {
    REGISTRY
        .villager_professions
        .id_from_key(key)
        .is_some_and(|profession| profession as i32 == id)
}

/// Registry index of `minecraft:none`.
fn none_profession_index() -> i32 {
    REGISTRY
        .villager_professions
        .id_from_key(&vanilla_villager_professions::NONE.key)
        .map_or(0, |id| id as i32)
}
