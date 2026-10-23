//! Player-triggered criteria: `PlayerTrigger` and `StartRidingTrigger`.

use crate::advancement::criterion::{CriterionTrigger, CriterionTriggerInstance};
use crate::advancement::triggers::{PlayerCondition, TriggerEvent};
use steel_utils::Identifier;

/// Instance of a `PlayerTrigger` criterion (`PlayerTrigger.TriggerInstance`).
///
/// Vanilla registers one singleton per id (`location`, `slept_in_bed`, `tick`,
/// `hero_of_the_village`, `voluntary_exile`, `avoid_vibration`); they all share
/// this instance type and only the shared `player` condition.
#[derive(Debug, Default)]
pub struct PlayerTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
}

impl PlayerTriggerInstance {
    /// Vanilla `PlayerTrigger.TriggerInstance.matches`: only the base `player`
    /// condition is consulted, so the instance always matches.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        match event {
            TriggerEvent::Player(base) | TriggerEvent::StartRiding(base) => {
                self.player.matches(&base.context(None))
            }
            _ => false,
        }
    }
}

impl CriterionTriggerInstance for PlayerTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// A `PlayerTrigger` singleton; one per `CriteriaTriggers` id.
#[derive(Debug)]
pub struct PlayerTrigger {
    id: &'static str,
}

impl PlayerTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for PlayerTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// `CriteriaTriggers.LOCATION`.
pub static LOCATION: PlayerTrigger = PlayerTrigger::new("location");
/// `CriteriaTriggers.SLEPT_IN_BED`.
pub static SLEPT_IN_BED: PlayerTrigger = PlayerTrigger::new("slept_in_bed");
/// `CriteriaTriggers.TICK`.
pub static TICK: PlayerTrigger = PlayerTrigger::new("tick");
/// `CriteriaTriggers.RAID_WIN` (`hero_of_the_village`).
pub static RAID_WIN: PlayerTrigger = PlayerTrigger::new("hero_of_the_village");
/// `CriteriaTriggers.RAID_OMEN` (`voluntary_exile`).
pub static RAID_OMEN: PlayerTrigger = PlayerTrigger::new("voluntary_exile");
/// `CriteriaTriggers.AVOID_VIBRATION`.
pub static AVOID_VIBRATION: PlayerTrigger = PlayerTrigger::new("avoid_vibration");

/// Instance of a `started_riding` criterion (`StartRidingTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct StartRidingTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
}

impl StartRidingTriggerInstance {
    /// Vanilla `StartRidingTrigger.TriggerInstance.matches`: only `player`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        match event {
            TriggerEvent::StartRiding(base) => self.player.matches(&base.context(None)),
            _ => false,
        }
    }
}

impl CriterionTriggerInstance for StartRidingTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.START_RIDING_TRIGGER`.
#[derive(Debug)]
pub struct StartRidingTrigger;

impl CriterionTrigger for StartRidingTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("started_riding")
    }
}

/// The static `minecraft:started_riding` trigger.
pub static START_RIDING: StartRidingTrigger = StartRidingTrigger;
