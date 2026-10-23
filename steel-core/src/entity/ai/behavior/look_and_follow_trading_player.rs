//! Vanilla `LookAndFollowTradingPlayerSink`.

use super::utils;
use super::villager;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `LookAndFollowTradingPlayerSink.WALK_CLOSE_ENOUGH`.
const WALK_CLOSE_ENOUGH: i32 = 2;
/// Vanilla `LookAndFollowTradingPlayerSink` follow range, `distanceToSqr(...) <= 16.0`.
const FOLLOW_DISTANCE_SQR: f64 = 16.0;

/// Vanilla `LookAndFollowTradingPlayerSink`: while a player trades with this
/// villager, walk toward them and keep looking at them.
pub struct LookAndFollowTradingPlayerSink {
    state: BehaviorState,
    speed_modifier: f32,
}

impl LookAndFollowTradingPlayerSink {
    /// Vanilla `LookAndFollowTradingPlayerSink(float)`.
    #[must_use]
    pub fn new(speed_modifier: f32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::WALK_TARGET, MemoryStatus::Registered),
                    (&mem::LOOK_TARGET, MemoryStatus::Registered),
                ],
                // Vanilla passes `Integer.MAX_VALUE` as the timeout and overrides
                // `timedOut` to `false`; an unreachable end timestamp is the same.
                i32::MAX,
                i32::MAX,
            ),
            speed_modifier,
        }
    }

    /// Vanilla `LookAndFollowTradingPlayerSink.followPlayer`.
    fn follow_player(&self, ctx: &mut BehaviorContext<'_>) {
        let Some(player) = trading_player(ctx.body) else {
            return;
        };

        utils::set_walk_and_look_target_to_entity(
            ctx,
            &player,
            self.speed_modifier,
            WALK_CLOSE_ENOUGH,
        );
    }
}

/// Vanilla `AbstractVillager.getTradingPlayer`.
fn trading_player(body: &SharedEntity) -> Option<SharedEntity> {
    villager::as_villager(body)?.trading_player()
}

/// Vanilla `LookAndFollowTradingPlayerSink.checkExtraStartConditions`.
fn can_follow(body: &SharedEntity) -> bool {
    let Some(player) = trading_player(body) else {
        return false;
    };

    body.is_alive()
        && !body.is_in_water()
        && !was_hurt_recently(body)
        && body.position().distance_squared(player.position()) <= FOLLOW_DISTANCE_SQR
}

/// Vanilla `LivingEntity.wasHurtRecently` is `hurtTime > 0`.
///
/// Steel tracks vanilla `invulnerableTime` (set to 20 on hurt, decremented every
/// tick) but not `hurtTime`; `invulnerableTime > 10` holds for exactly the ten
/// ticks where vanilla's `hurtTime > 0`.
fn was_hurt_recently(body: &SharedEntity) -> bool {
    body.as_living_entity()
        .is_some_and(|living| living.invulnerable_time() > 10)
}

impl Behavior for LookAndFollowTradingPlayerSink {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        can_follow(ctx.body)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        can_follow(ctx.body)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.follow_player(ctx);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.follow_player(ctx);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        utils::clear_walk_target(ctx);
        utils::clear_look_target(ctx);
    }
}
