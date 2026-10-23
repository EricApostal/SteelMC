//! Vanilla `BehaviorBuilder` facade.
//!
//! Vanilla builds behaviors from DataFixerUpper higher-kinded applicatives; Rust
//! has no equivalent, so the same compositions are expressed as concrete
//! [`Trigger`] combinators. The static methods keep vanilla's names so Wave 2
//! behavior code reads like its Java counterpart.

use super::BehaviorControl;
use super::OneShot;
use super::Trigger;
use super::TriggerIf;
use super::TriggerOneShuffled;
use super::TriggerSequence;

/// Vanilla `BehaviorBuilder`: entry points for declarative one-shot behaviors.
pub struct BehaviorBuilder;

impl BehaviorBuilder {
    /// Vanilla `BehaviorBuilder.create`: wraps a trigger into a [`OneShot`].
    #[must_use]
    pub fn create(trigger: impl Trigger + 'static) -> OneShot<impl Trigger> {
        OneShot::new(trigger)
    }

    /// Vanilla `BehaviorBuilder.sequence(first, second)`: runs `second` only when
    /// `first` succeeds.
    #[must_use]
    pub fn sequence(first: Box<dyn Trigger>, second: Box<dyn Trigger>) -> TriggerSequence {
        TriggerSequence::new(first, second)
    }

    /// Vanilla `BehaviorBuilder.triggerIf(predicate)`.
    #[must_use]
    pub fn trigger_if<F>(predicate: F) -> TriggerIf<F>
    where
        F: Fn(&mut super::BehaviorContext<'_>) -> bool + Send + Sync,
    {
        TriggerIf::new(predicate)
    }

    /// Vanilla `BehaviorBuilder.triggerIf(predicate, behavior)`: gates `behavior`
    /// behind `predicate` and returns the composed trigger.
    #[must_use]
    pub fn trigger_if_behavior<F>(predicate: F, behavior: Box<dyn Trigger>) -> TriggerSequence
    where
        F: Fn(&mut super::BehaviorContext<'_>) -> bool + Send + Sync + 'static,
    {
        TriggerSequence::new(Box::new(TriggerIf::new(predicate)), behavior)
    }

    /// Vanilla `TriggerGate.triggerOneShuffled`: one shuffled weighted trigger per tick.
    #[must_use]
    pub fn trigger_one_shuffled(
        weighted_triggers: Vec<(Box<dyn Trigger>, i32)>,
    ) -> TriggerOneShuffled {
        TriggerOneShuffled::new(weighted_triggers)
    }

    /// Boxes a behavior for use in [`super::gate::GateBehavior`] /
    /// [`super::gate::RunOne`] child lists.
    #[must_use]
    pub fn boxed(behavior: impl BehaviorControl + 'static) -> Box<dyn BehaviorControl> {
        Box::new(behavior)
    }
}
