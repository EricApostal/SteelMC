//! Vanilla piglin behaviors (`net.minecraft.world.entity.monster.piglin`).
//!
//! The six piglin-only behaviors live in vanilla's `monster.piglin` package rather
//! than `ai.behavior`; Steel keeps them together here, next to the shared
//! [`ai`] statics they call.

pub mod ai;
pub mod remember_if_hoglin_was_killed;
pub mod start_admiring_item_if_seen;
pub mod start_hunting_hoglin;
pub mod stop_admiring_if_item_too_far_away;
pub mod stop_admiring_if_tired_of_trying_to_reach_item;
pub mod stop_holding_item_if_no_longer_admiring;

pub use remember_if_hoglin_was_killed::RememberIfHoglinWasKilled;
pub use start_admiring_item_if_seen::StartAdmiringItemIfSeen;
pub use start_hunting_hoglin::StartHuntingHoglin;
pub use stop_admiring_if_item_too_far_away::StopAdmiringIfItemTooFarAway;
pub use stop_admiring_if_tired_of_trying_to_reach_item::StopAdmiringIfTiredOfTryingToReachItem;
pub use stop_holding_item_if_no_longer_admiring::StopHoldingItemIfNoLongerAdmiring;
