//! Vanilla `TradeWithPlayerGoal` (`goal/TradeWithPlayerGoal.java`): a villager that is being
//! traded with stands still and faces the customer.
//!
//! Vanilla stores the `AbstractVillager`; Steel reads the trading player through the
//! [`Merchant`](crate::entity::npc::trading::Merchant) capability so the goal serves both
//! villagers and wandering traders.
//!
//! The wandering trader keeps a private `TradeWithPlayerGoal` variant that works from its
//! `TraderState` (`npc/wandering_trader/goals.rs`); this shared goal is the vanilla
//! `AbstractVillager` one.
//!
//! **Not yet wired**: `mod trade_with_player;` is intentionally absent from `goal/mod.rs`
//! until `Entity::as_merchant()` lands (requested from the `entity/entity/mod.rs` owner).
//! Wiring it then is one `mod` + one `pub(crate) use` line; nothing else in this file needs
//! to change.

use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};

/// Vanilla `TradeWithPlayerGoal.canUse`: the trader gives up past 4 blocks.
const MAX_TRADING_DISTANCE_SQR: f64 = 16.0;

/// Vanilla `TradeWithPlayerGoal`.
pub struct TradeWithPlayerGoal;

impl TradeWithPlayerGoal {
    /// Vanilla `new TradeWithPlayerGoal(mob)`.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Default for TradeWithPlayerGoal {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `LivingEntity.wasHurtRecently` (`hurtTime > 0`); Steel tracks
/// `invulnerableTime > 10` for exactly those ticks, like the wandering trader's goal.
fn was_hurt_recently(mob: &dyn PathfinderMob) -> bool {
    mob.living_base().invulnerable_time() > 10
}

impl Goal for TradeWithPlayerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::JUMP | GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !mob.is_alive() || mob.is_in_water() || !mob.on_ground() || was_hurt_recently(mob) {
            return false;
        }

        let Some(merchant) = mob.as_merchant() else {
            return false;
        };
        let Some(trader) = merchant.get_trading_player() else {
            return false;
        };

        mob.position().distance_squared(trader.position()) <= MAX_TRADING_DISTANCE_SQR
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(merchant) = mob.as_merchant() {
            merchant.set_trading_player(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_with_player_goal_claims_jump_and_move() {
        let goal = TradeWithPlayerGoal::new();

        assert_eq!(goal.controls(), GoalControls::JUMP | GoalControls::MOVE);
    }
}
