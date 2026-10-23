//! Villager gossip and reputation (vanilla `net.minecraft.world.entity.ai.gossip`).
//!
//! Ports the 26.3 gossip stack 1:1: [`GossipContainer`]/[`GossipType`] storage and decay,
//! [`ReputationEventType`]/[`ReputationEventHandler`] event dispatch, and the reputation price
//! math that `Villager.updateSpecialPrices` applies to merchant offers.
//!
//! Wave 2 wiring (all in the villager entity, which owns the state):
//! - `Villager` holds a [`GossipContainer`] plus `last_gossip_time`/`last_gossip_decay_time`, and
//!   implements [`ReputationEventHandler`] by mapping events to
//!   [`GossipType::add`](GossipContainer::add) calls exactly like `Villager.onReputationEventFrom`
//!   (Villager.java:864-880): curing a zombie villager adds `MAJOR_POSITIVE 20` +
//!   `MINOR_POSITIVE 25`, a trade adds `TRADING 2`, hurting a villager adds `MINOR_NEGATIVE 25`,
//!   and killing one adds `MAJOR_NEGATIVE 25`; `GOLEM_KILLED` is unused by the villager.
//! - `Villager.maybeDecayGossip` (Villager.java:833-840) decays every [`GOSSIP_DECAY_INTERVAL`]
//!   ticks and `Villager.gossip` (Villager.java:819-831) calls
//!   [`GossipContainer::transfer_from`] with the villager's own RNG, [`MAX_GOSSIP_TOPICS`] and
//!   [`GOSSIP_COOLDOWN`]. `ServerLevel.onReputationEvent` (ServerLevel.java:1694-1696) is the
//!   dispatch entry point.
//! - `Villager.updateSpecialPrices` (Villager.java:410-427) resets every offer's special price
//!   diff and adds [`special_price_diff`] with the player's reputation
//!   ([`GossipContainer::get_reputation`] with `|_| true`), the offer's price multiplier and the
//!   Hero of the Village amplifier.
//! - `Villager`.save/load store the container under `Gossips` via
//!   [`GossipContainer::save_to`]/[`GossipContainer::load_from`]; `ZombieVillager` uses the same
//!   key and [`GossipContainer::copy`] when a villager is converted.

pub mod container;
pub mod discount;
pub mod gossip_type;
pub mod reputation_event;

pub use container::{
    DISCARD_THRESHOLD, GOSSIP_COOLDOWN, GOSSIP_DECAY_INTERVAL, GossipContainer, GossipEntry,
    MAX_GOSSIP_TOPICS,
};
pub use discount::{
    hero_of_the_village_modifier, hero_of_the_village_price_discount, reputation_price_discount,
    special_price_diff,
};
pub use gossip_type::GossipType;
pub use reputation_event::{ReputationEventHandler, ReputationEventType};

#[cfg(test)]
mod tests;
