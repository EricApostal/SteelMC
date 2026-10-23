//! Delta-tracking state for `CSetHealth` packet deduplication.
//!
//! Mirrors the three `ServerPlayer` fields `lastSentHealth`, `lastSentFood` and
//! `lastFoodSaturationZero`, so we only send `CSetHealth` when one of them
//! actually changes.

use crate::player::Player;

/// Tracks the last health/food/saturation values sent to the client.
pub struct HealthSyncState {
    /// Last health value sent to the client.
    pub last_health: f32,
    /// Last food level sent to the client.
    pub last_food: i32,
    /// Whether saturation was zero last time we sent health.
    pub saturation_zero: bool,
}

impl HealthSyncState {
    /// Creates a new state that will trigger a send on the first tick.
    ///
    /// Uses vanilla `ServerPlayer`'s field initializers so the first
    /// `ClientboundSetHealthPacket` is always sent.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_health: -1.0e8,
            last_food: -99_999_999,
            saturation_zero: true,
        }
    }

    /// Returns true if the given values differ from what was last sent.
    #[expect(
        clippy::float_cmp,
        reason = "intentional exact comparison: we only want to send updates when the value changes from what we last sent"
    )]
    #[must_use]
    pub fn needs_update(&self, health: f32, food: i32, saturation_zero: bool) -> bool {
        self.last_health != health
            || self.last_food != food
            || self.saturation_zero != saturation_zero
    }

    /// Records that we just sent the given values to the client.
    pub const fn record_sent(&mut self, health: f32, food: i32, saturation_zero: bool) {
        self.last_health = health;
        self.last_food = food;
        self.saturation_zero = saturation_zero;
    }

    /// Invalidates the state so the next tick will re-send.
    ///
    /// Mirrors vanilla `ServerPlayer.resetSentInfo`, which only resets the
    /// health field (`lastSentHealth`). Experience and food-entity-data
    /// sent-info live in their own dirty flags.
    pub const fn invalidate(&mut self) {
        self.last_health = -1.0e8;
    }
}

impl Player {
    /// Invalidates the delta-tracking state so that the next `tick()` will send
    /// `CSetHealth` to the client (vanilla: `resetSentInfo`).
    pub fn reset_sent_info(&self) {
        self.health_sync.lock().invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::HealthSyncState;

    /// The initial state uses vanilla `ServerPlayer`'s field initializers, so
    /// the first tick always sends `CSetHealth` for any reachable values.
    #[test]
    fn first_tick_always_sends() {
        let state = HealthSyncState::new();
        assert!(state.needs_update(20.0, 20, false));
        assert!(state.needs_update(0.0, 0, true));
        // Only the exact sentinel triple itself compares equal — unreachable
        // in practice (health is clamped to `[0, max]`, food to `[0, 20]`).
        assert!(!state.needs_update(-1.0e8, -99_999_999, true));
    }

    /// Vanilla re-sends when health, food level, or saturation zero-ness
    /// changes — and only then.
    #[test]
    fn resends_only_on_change() {
        let mut state = HealthSyncState::new();
        state.record_sent(19.5, 17, false);

        assert!(!state.needs_update(19.5, 17, false));
        assert!(state.needs_update(20.0, 17, false));
        assert!(state.needs_update(19.5, 16, false));
        assert!(state.needs_update(19.5, 17, true));
    }

    /// Vanilla `resetSentInfo` clears only the health field, leaving the food
    /// and saturation sent-info intact.
    #[test]
    fn invalidate_resets_only_health() {
        let mut state = HealthSyncState::new();
        state.record_sent(10.0, 12, true);
        assert!(!state.needs_update(10.0, 12, true));

        state.invalidate();

        assert!(state.needs_update(10.0, 12, true));
        assert_eq!(state.last_food, 12);
        assert!(state.saturation_zero);
    }
}
