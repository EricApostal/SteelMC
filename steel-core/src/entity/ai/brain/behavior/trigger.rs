//! Vanilla declarative `Trigger` and `OneShot`, plus the `BehaviorBuilder`
//! combinators that translate to plain Rust.
//!
//! Vanilla's `BehaviorBuilder` is built on `DataFixerUpper` higher-kinded types,
//! which have no Rust equivalent. The observable behavior is preserved by
//! composing [`Trigger`]s instead: `BehaviorBuilder.sequence`/`triggerIf` and
//! `TriggerGate` are covered by [`TriggerSequence`], [`TriggerIf`] and
//! [`TriggerOneShuffled`].

use std::any;

use super::super::memory::MemoryModuleTypeRef;
use super::BehaviorContext;
use super::BehaviorControl;
use super::BehaviorStatus;
use super::gate::ShufflingList;

/// Vanilla declarative `Trigger<E>`.
pub trait Trigger: Send + Sync {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool;
}

/// Vanilla `OneShot<E>`: runs a [`Trigger`] once and then stops.
pub struct OneShot<T: Trigger> {
    trigger: T,
    required_memories: Vec<MemoryModuleTypeRef>,
    status: BehaviorStatus,
}

impl<T: Trigger> OneShot<T> {
    #[must_use]
    pub fn new(trigger: T) -> Self {
        Self {
            trigger,
            required_memories: Vec::new(),
            status: BehaviorStatus::Stopped,
        }
    }

    /// Declares the memories the wrapped trigger reads.
    #[must_use]
    pub fn with_required_memories(mut self, required_memories: Vec<MemoryModuleTypeRef>) -> Self {
        self.required_memories = required_memories;
        self
    }
}

impl<T: Trigger> Trigger for OneShot<T> {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.trigger.trigger(ctx)
    }
}

impl<T: Trigger> BehaviorControl for OneShot<T> {
    fn status(&self) -> BehaviorStatus {
        self.status
    }

    fn required_memories(&self) -> Vec<MemoryModuleTypeRef> {
        self.required_memories.clone()
    }

    fn try_start(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if self.trigger.trigger(ctx) {
            self.status = BehaviorStatus::Running;
            true
        } else {
            false
        }
    }

    fn tick_or_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.do_stop(ctx);
    }

    fn do_stop(&mut self, _ctx: &mut BehaviorContext<'_>) {
        self.status = BehaviorStatus::Stopped;
    }

    fn debug_string(&self) -> String {
        format!("OneShot[{}]", any::type_name::<T>())
    }
}

/// Vanilla `BehaviorBuilder.sequence(first, second)`.
pub struct TriggerSequence {
    first: Box<dyn Trigger>,
    second: Box<dyn Trigger>,
}

impl TriggerSequence {
    #[must_use]
    pub fn new(first: Box<dyn Trigger>, second: Box<dyn Trigger>) -> Self {
        Self { first, second }
    }
}

impl Trigger for TriggerSequence {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.first.trigger(ctx) && self.second.trigger(ctx)
    }
}

/// Vanilla `BehaviorBuilder.triggerIf(predicate)`.
pub struct TriggerIf<F> {
    predicate: F,
}

impl<F> TriggerIf<F>
where
    F: Fn(&mut BehaviorContext<'_>) -> bool + Send + Sync,
{
    #[must_use]
    pub const fn new(predicate: F) -> Self {
        Self { predicate }
    }
}

impl<F> Trigger for TriggerIf<F>
where
    F: Fn(&mut BehaviorContext<'_>) -> bool + Send + Sync,
{
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        (self.predicate)(ctx)
    }
}

/// Vanilla `TriggerGate.triggerOneShuffled`: shuffles weighted triggers and runs
/// until one triggers, always reporting success.
pub struct TriggerOneShuffled {
    triggers: ShufflingList<Box<dyn Trigger>>,
}

impl TriggerOneShuffled {
    #[must_use]
    pub fn new(weighted_triggers: Vec<(Box<dyn Trigger>, i32)>) -> Self {
        let mut triggers = ShufflingList::new();
        for (trigger, weight) in weighted_triggers {
            triggers.add(trigger, weight);
        }
        Self { triggers }
    }
}

impl Trigger for TriggerOneShuffled {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.triggers.shuffle();
        for trigger in self.triggers.iter_mut() {
            if trigger.trigger(ctx) {
                break;
            }
        }
        true
    }
}
