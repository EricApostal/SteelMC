//! `BadOmenMobEffect` behavior (vanilla `net.minecraft.world.effect.BadOmenMobEffect`).

use steel_registry::vanilla_mob_effects;
use steel_utils::types::Difficulty;

use super::MobEffectBehavior;
use crate::entity::{Entity as _, LivingEntity, MobEffectInstance};
use crate::world::World;

/// Vanilla `BadOmenMobEffect.RAID_OMEN_DURATION`.
const RAID_OMEN_DURATION: i32 = 600;

/// Bad omen mob effect behavior.
pub struct BadOmenBehavior;

impl MobEffectBehavior for BadOmenBehavior {
    fn should_apply_effect_tick_this_tick(&self, _tick_count: i32, _amplifier: i32) -> bool {
        true
    }

    fn apply_effect_tick(&self, world: &World, user: &dyn LivingEntity, amplifier: i32) -> bool {
        let Some(player) = user.as_player() else {
            return true;
        };

        if player.is_spectator()
            || world.difficulty() == Difficulty::Peaceful
            || !world.is_village(player.block_position())
        {
            return true;
        }

        let at_max_omen = world
            .get_raid_at(player.block_position())
            .is_some_and(|raid| raid.raid_omen_level >= raid.max_raid_omen_level());

        if at_max_omen {
            return true;
        }

        // Vanilla converts bad omen into raid omen, leaving the raid itself to
        // `RaidOmenMobEffect` when the effect expires.
        player.add_mob_effect(MobEffectInstance::with_duration(
            vanilla_mob_effects::RAID_OMEN,
            RAID_OMEN_DURATION,
            amplifier,
        ));
        player.set_raid_omen_position(player.block_position());
        false
    }
}
