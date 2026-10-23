//! Vanilla `LookAtTradingPlayerGoal` (`goal/LookAtTradingPlayerGoal.java`).
//!
//! While a merchant is trading, it keeps looking at its trading player. Vanilla subclasses
//! `LookAtPlayerGoal`; Steel's `LookAtPlayerGoal` owns private state, so this is a standalone
//! goal with the same look behavior and a caller-supplied trading-player provider (villagers and
//! wandering traders own different trading state types).

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::ai::control::{DEFAULT_LOOK_X_MAX_ROT_ANGLE, DEFAULT_LOOK_Y_MAX_ROT_SPEED};
use crate::entity::{Entity, PathfinderMob, SharedEntity};

/// Vanilla `LookAtPlayerGoal(..., 8.0F)` look distance used by `LookAtTradingPlayerGoal`.
const TRADING_LOOK_DISTANCE: f64 = 8.0;

/// Resolves the player a merchant body is currently trading with.
pub(crate) type TradingPlayerProvider = fn(&dyn Entity) -> Option<SharedEntity>;

pub(crate) struct LookAtTradingPlayerGoal {
    look_at: Option<SharedEntity>,
    look_time: i32,
    trading_player: TradingPlayerProvider,
}

impl LookAtTradingPlayerGoal {
    #[must_use]
    pub(crate) const fn new(trading_player: TradingPlayerProvider) -> Self {
        Self {
            look_at: None,
            look_time: 0,
            trading_player,
        }
    }
}

impl Goal for LookAtTradingPlayerGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.look_at = (self.trading_player)(mob.as_entity_event_source());
        self.look_at
            .as_ref()
            .is_some_and(|target| target.as_player().is_some())
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(look_at) = &self.look_at else {
            return false;
        };
        if !look_at.is_alive() {
            return false;
        }
        if mob.position().distance_squared(look_at.position())
            > TRADING_LOOK_DISTANCE * TRADING_LOOK_DISTANCE
        {
            return false;
        }

        self.look_time > 0
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.look_time = reduced_tick_delay(40 + rand::random_range(0..40));
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.look_at = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(look_at) = &self.look_at else {
            return;
        };
        if !look_at.is_alive() {
            return;
        }

        let position = look_at.position();
        mob.mob_base().controls().lock().look_control.set_look_at(
            glam::DVec3::new(position.x, look_at.get_eye_y(), position.z),
            DEFAULT_LOOK_Y_MAX_ROT_SPEED,
            DEFAULT_LOOK_X_MAX_ROT_ANGLE,
        );
        self.look_time -= 1;
    }
}
