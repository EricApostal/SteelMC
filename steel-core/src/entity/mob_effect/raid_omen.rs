//! `RaidOmenMobEffect` behavior (vanilla `net.minecraft.world.effect.RaidOmenMobEffect`).

use super::MobEffectBehavior;
use crate::entity::LivingEntity;
use crate::world::World;

/// Raid omen mob effect behavior.
pub struct RaidOmenBehavior;

impl MobEffectBehavior for RaidOmenBehavior {
    fn should_apply_effect_tick_this_tick(&self, remaining_duration: i32, _amplifier: i32) -> bool {
        // Vanilla `RaidOmenMobEffect.shouldApplyEffectTickThisTick` waits for the last tick.
        remaining_duration == 1
    }

    fn apply_effect_tick(&self, world: &World, user: &dyn LivingEntity, _amplifier: i32) -> bool {
        let Some(player) = user.as_player() else {
            return true;
        };
        if user.is_spectator() {
            return true;
        }

        let Some(pos) = player.raid_omen_position() else {
            return true;
        };

        world
            .raids()
            .lock()
            .create_or_extend_raid(world, player, pos);
        player.clear_raid_omen_position();
        false
    }
}
