//! Vanilla goals of the wandering trader that Steel does not share with other mobs.
//!
//! `TradeWithPlayerGoal`, `LookAtTradingPlayerGoal` and the private vanilla inner
//! class `WanderingTrader.WanderToPositionGoal` all drive the trader's merchant
//! state, so they take the shared [`TraderState`] instead of an entity reference.

use std::sync::Arc;

use glam::DVec3;
use steel_utils::BlockPos;

use crate::entity::ai::control::{DEFAULT_LOOK_X_MAX_ROT_ANGLE, DEFAULT_LOOK_Y_MAX_ROT_SPEED};
use crate::entity::ai::goal::{Goal, GoalControls};
use crate::entity::npc::trading::Merchant;
use crate::entity::{Entity, LivingEntity, PathfinderMob, SharedEntity};

use super::merchant::TraderState;

/// Vanilla `LivingEntity.wasHurtRecently` is `hurtTime > 0`.
///
/// Steel tracks vanilla `invulnerableTime` (set to 20 on hurt, decremented every
/// tick) but not `hurtTime`; `invulnerableTime > 10` holds for exactly the ten
/// ticks where vanilla's `hurtTime > 0`.
fn was_hurt_recently(mob: &dyn PathfinderMob) -> bool {
    mob.living_base().invulnerable_time() > 10
}

/// Vanilla `TradeWithPlayerGoal`.
pub(crate) struct TradeWithPlayerGoal {
    state: Arc<TraderState>,
}

impl TradeWithPlayerGoal {
    pub(crate) const fn new(state: Arc<TraderState>) -> Self {
        Self { state }
    }
}

impl Goal for TradeWithPlayerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::JUMP
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if !Entity::is_alive(mob.as_entity_event_source())
            || mob.is_in_water()
            || !mob.on_ground()
            || was_hurt_recently(mob)
        {
            return false;
        }

        self.state
            .trading_player()
            .is_some_and(|trader| mob.distance_to_sqr(trader.position()) <= 16.0)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        let merchant: &dyn Merchant = &*self.state;
        merchant.set_trading_player(None);
    }
}

/// Vanilla `LookAtTradingPlayerGoal`: always looks at the player currently trading.
pub(crate) struct LookAtTradingPlayerGoal {
    state: Arc<TraderState>,
    look_time: i32,
}

impl LookAtTradingPlayerGoal {
    pub(crate) const fn new(state: Arc<TraderState>) -> Self {
        Self {
            state,
            look_time: 0,
        }
    }
}

impl Goal for LookAtTradingPlayerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        self.state.is_trading()
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        self.look_time > 0 && self.state.is_trading()
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        // Vanilla `Mth.positiveCeilDiv(40 + random.nextInt(40), 2)`.
        self.look_time = (40 + rand::random_range(0..40) + 1) / 2;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(trader) = self.state.trading_player() else {
            return;
        };

        let position = trader.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            DVec3::new(position.x, trader.get_eye_y(), position.z),
            DEFAULT_LOOK_Y_MAX_ROT_SPEED,
            DEFAULT_LOOK_X_MAX_ROT_ANGLE,
        );
        self.look_time -= 1;
    }
}

/// Vanilla `WanderingTrader.WanderToPositionGoal` (the vanilla private inner class).
pub(crate) struct WanderToPositionGoal {
    state: Arc<TraderState>,
    stop_distance: f64,
    speed_modifier: f64,
}

impl WanderToPositionGoal {
    pub(crate) const fn new(
        state: Arc<TraderState>,
        stop_distance: f64,
        speed_modifier: f64,
    ) -> Self {
        Self {
            state,
            stop_distance,
            speed_modifier,
        }
    }

    /// Vanilla `isTooFarAway`: `!pos.closerToCenterThan(trader.position(), distance)`.
    fn is_too_far_away(mob: &dyn PathfinderMob, pos: BlockPos, distance: f64) -> bool {
        !pos.closer_to_center_than(mob.position(), distance)
    }
}

impl Goal for WanderToPositionGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.state
            .wander_target()
            .is_some_and(|target| Self::is_too_far_away(mob, target, self.stop_distance))
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(wander_target) = self.state.wander_target() else {
            return;
        };
        if !mob.mob_base().navigation().lock().is_done() {
            return;
        }

        if Self::is_too_far_away(mob, wander_target, 10.0) {
            let position = mob.position();
            let (target_x, target_y, target_z) = wander_target.get_center();
            let direction = (DVec3::new(target_x, target_y, target_z) - position).normalize();
            let target = direction * 10.0 + position;
            mob.move_to_pos(target, self.speed_modifier);
        } else {
            let (x, y, z) = wander_target.get_center();
            mob.move_to_pos(DVec3::new(x, y, z), self.speed_modifier);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.state.set_wander_target(None);
        mob.mob_base().navigation().lock().stop();
    }
}

/// Vanilla `Mob.class` look target: every living entity that is not a player.
pub(crate) fn is_mob_look_target(living: &dyn LivingEntity) -> bool {
    living.as_player().is_none()
}

/// Vanilla `LookAtPlayerGoal.DEFAULT_PROBABILITY`.
pub(crate) const LOOK_AT_PROBABILITY: f32 = 0.02;
