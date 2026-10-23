//! Criterion trigger framework.
//!
//! Mirrors vanilla `net.minecraft.advancements.triggers`: an [`Advancement`]
//! criterion pairs a trigger (the event that fires it) with a parsed
//! [`CriterionTriggerInstance`] holding that criterion's conditions. Trigger
//! instances are parsed at build time, so [`CriterionTrigger`] only has to
//! identify the event by its registry id.
//!
//! [`Advancement`]: crate::advancement::Advancement

use crate::advancement::triggers::TriggerEvent;
use steel_utils::Identifier;

/// Parsed conditions of one criterion, tested against its trigger's event.
pub trait CriterionTriggerInstance: Send + Sync {
    /// Tests this instance against the event raised by the trigger.
    ///
    /// Implementations accept only their own [`TriggerEvent`] variant; any other
    /// variant never satisfies the criterion.
    fn test(&self, event: &TriggerEvent<'_>) -> bool;
}

/// A criterion trigger: identifies the event that grants its criteria.
pub trait CriterionTrigger: Send + Sync {
    /// The registry id of this trigger, e.g. `minecraft:inventory_changed`.
    fn id(&self) -> Identifier;
}

/// Type-erased criterion as stored by an [`Advancement`](crate::advancement::Advancement).
pub trait AnyCriterion: Send + Sync {
    /// The id of the trigger this criterion listens to.
    fn id(&self) -> Identifier;

    /// Tests this criterion's conditions against a trigger event.
    fn test(&self, event: &TriggerEvent<'_>) -> bool;
}

impl<T, G> AnyCriterion for Criterion<T, G>
where
    T: CriterionTriggerInstance + 'static,
    G: CriterionTrigger + 'static,
{
    fn id(&self) -> Identifier {
        self.trigger.id()
    }

    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.instance.test(event)
    }
}

/// One advancement criterion: a trigger plus its parsed instance.
pub struct Criterion<T: CriterionTriggerInstance + 'static, G: CriterionTrigger + ?Sized + 'static>
{
    trigger: &'static G,
    instance: Box<T>,
}

impl<T: CriterionTriggerInstance, G: CriterionTrigger + ?Sized> Criterion<T, G> {
    /// Pairs a trigger with a parsed instance.
    #[must_use]
    pub fn new(trigger: &'static G, instance: T) -> Self {
        Self {
            trigger,
            instance: Box::new(instance),
        }
    }

    /// The trigger this criterion listens to.
    #[must_use]
    pub const fn trigger(&self) -> &'static G {
        self.trigger
    }

    /// The parsed conditions of this criterion.
    #[must_use]
    pub fn instance(&self) -> &T {
        &self.instance
    }
}

impl Criterion<ImpossibleInstance, ImpossibleTrigger> {
    /// The `minecraft:impossible` criterion, grantable only by commands.
    #[must_use]
    pub fn impossible() -> Self {
        Criterion {
            trigger: &IMPOSSIBLE_TRIGGER,
            instance: Box::new(ImpossibleInstance),
        }
    }
}

impl Default for Criterion<ImpossibleInstance, ImpossibleTrigger> {
    fn default() -> Self {
        Self::impossible()
    }
}

impl<
    T: CriterionTriggerInstance + std::fmt::Debug + 'static,
    G: CriterionTrigger + ?Sized + 'static,
> std::fmt::Debug for Criterion<T, G>
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Criterion")
            .field("trigger", &self.trigger.id())
            .field("instance", &self.instance)
            .finish()
    }
}

/// The `minecraft:impossible` trigger, which can only be granted by commands.
pub struct ImpossibleTrigger;

impl CriterionTrigger for ImpossibleTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("impossible")
    }
}

/// The static `minecraft:impossible` trigger.
pub static IMPOSSIBLE_TRIGGER: ImpossibleTrigger = ImpossibleTrigger;

/// Conditions of an `minecraft:impossible` criterion (vanilla `TriggerInstance.INSTANCE`).
#[derive(Debug, Default, Clone, Copy)]
pub struct ImpossibleInstance;

impl CriterionTriggerInstance for ImpossibleInstance {
    fn test(&self, _event: &TriggerEvent<'_>) -> bool {
        // Vanilla `ImpossibleTrigger` has no `trigger` method: it never fires.
        false
    }
}
