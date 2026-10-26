//! `/schedule` bookkeeping, mirroring vanilla
//! `net.minecraft.server.ServerFunctionManager` and `net.minecraft.util.TimerQueue`.
//!
//! Functions are scheduled onto absolute game ticks and popped when the tick
//! advances past them, exactly like vanilla's `TimerQueue`. Steel keeps the queue
//! free of world types so `/schedule` and the reload path can share it.
//!
//! Deliberate difference: vanilla persists the queue as a `SavedData` entry in
//! `level.dat`; Steel keeps it in memory, so pending schedules end with the server
//! process.

use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
};

use steel_utils::Identifier;

/// Vanilla `ScheduleCommand.ScheduleMode`: what to do with an existing entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScheduleMode {
    /// Vanilla `append`: keep the existing entry and add another.
    Append,
    /// Vanilla `replace`: drop the existing entry first.
    Replace,
}

/// Vanilla `FunctionCallback`/`FunctionTagCallback`: what a scheduled entry runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ScheduledCallback {
    /// `FunctionCallback`: one function id.
    Function(Identifier),
    /// `FunctionTagCallback`: every function in a tag, in tag order.
    Tag(Identifier),
}

impl ScheduledCallback {
    /// Vanilla `TimerQueue`'s string id: `id` for functions, `#id` for tags.
    #[must_use]
    pub(crate) fn key(&self) -> String {
        match self {
            Self::Function(id) => id.to_string(),
            Self::Tag(id) => format!("#{id}"),
        }
    }
}

/// Vanilla `TimerQueue<QualifiedName>` equivalent for scheduled functions.
#[derive(Debug, Default)]
pub(crate) struct ScheduleRegistry {
    queue: BinaryHeap<Reverse<ScheduledEntry>>,
    /// Vanilla `TimerQueue.sequentialId`: orders entries due on the same tick.
    sequential_id: u64,
}

impl ScheduleRegistry {
    /// Vanilla `ScheduleCommand.schedule`: queues `callback` for `at_tick`.
    ///
    /// `Replace` drops every pending entry for the callback first, exactly like
    /// vanilla's `TimerQueue.remove` followed by `TimerQueue.schedule`.
    pub(crate) fn schedule(
        &mut self,
        callback: ScheduledCallback,
        at_tick: i64,
        mode: ScheduleMode,
    ) {
        if mode == ScheduleMode::Replace {
            self.clear_callback(&callback);
        } else if self
            .queue
            .iter()
            .any(|entry| entry.0.callback == callback && entry.0.tick == at_tick)
        {
            // Vanilla `TimerQueue.schedule` ignores a duplicate `(id, time)` pair.
            return;
        }

        let sequential_id = self.sequential_id;
        self.sequential_id = self.sequential_id.wrapping_add(1);
        self.queue.push(Reverse(ScheduledEntry {
            tick: at_tick,
            sequential_id,
            callback,
        }));
    }

    /// Vanilla `ScheduleCommand.clear`: drops every entry for a function id.
    ///
    /// Returns how many entries were removed; vanilla reports the count and errors
    /// when it is zero. Tags cannot be cleared by `/schedule clear`, exactly like
    /// vanilla, whose `IdentifierArgument` never matches the `#id` key.
    pub(crate) fn clear(&mut self, function: &Identifier) -> usize {
        self.clear_callback(&ScheduledCallback::Function(function.clone()))
    }

    /// Removes every entry for `callback`, returning the removed count.
    fn clear_callback(&mut self, callback: &ScheduledCallback) -> usize {
        let before = self.queue.len();
        self.queue.retain(|entry| &entry.0.callback != callback);
        before - self.queue.len()
    }

    /// Pops every entry due at or before `current_tick`, in vanilla `TimerQueue` order.
    pub(crate) fn take_due(&mut self, current_tick: i64) -> Vec<ScheduledCallback> {
        let mut due = Vec::new();
        while let Some(Reverse(entry)) = self.queue.peek() {
            if entry.tick > current_tick {
                break;
            }
            let Reverse(entry) = self.queue.pop().expect("peeked entry is present");
            due.push(entry.callback);
        }
        due
    }

    /// Vanilla `TimerQueue.getEventsIds`: every pending id, for `/schedule` suggestions.
    #[must_use]
    pub(crate) fn pending_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .queue
            .iter()
            .map(|entry| entry.0.callback.key())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Every pending entry and its due tick, in due order.
    #[cfg(test)]
    pub(crate) fn pending(&self) -> Vec<(ScheduledCallback, i64)> {
        let mut entries: Vec<(ScheduledCallback, i64)> = self
            .queue
            .iter()
            .map(|entry| (entry.0.callback.clone(), entry.0.tick))
            .collect();
        entries.sort_by(|left, right| {
            left.1
                .cmp(&right.1)
                .then_with(|| left.0.key().cmp(&right.0.key()))
        });
        entries
    }

    /// Whether any function is scheduled.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// One heap entry; `Reverse` makes `BinaryHeap` a min-heap on `tick`.
#[derive(Debug, PartialEq, Eq)]
struct ScheduledEntry {
    tick: i64,
    sequential_id: u64,
    callback: ScheduledCallback,
}

impl Ord for ScheduledEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tick
            .cmp(&other.tick)
            .then_with(|| self.sequential_id.cmp(&other.sequential_id))
    }
}

impl PartialOrd for ScheduledEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Vanilla `ScheduleCommand.parseDelay`: `10t`, `5s`, `3d` -> ticks.
///
/// # Errors
/// Returns the vanilla-shaped failure reason when the argument is unusable.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "vanilla ScheduleCommand.parseDelay API surface")
)]
pub(crate) fn parse_delay(argument: &str) -> Result<i64, String> {
    let (value, unit) = match argument.chars().last() {
        Some('t' | 's' | 'd') => argument.split_at(argument.len().saturating_sub(1)),
        _ => (argument, "t"),
    };
    let value: f64 = value
        .parse()
        .map_err(|_| format!("Invalid time '{argument}'"))?;
    if value < 0.0 {
        return Err(format!("Invalid time '{argument}'"));
    }
    let ticks = match unit {
        "t" => value,
        "s" => value * 20.0,
        "d" => value * 24_000.0,
        _ => return Err(format!("Invalid time '{argument}'")),
    };
    Ok(ticks as i64)
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "these tests unwrap their own fixture data so failures point at the offending assertion"
)]
mod tests {
    use super::{ScheduleMode, ScheduleRegistry, ScheduledCallback, parse_delay};
    use steel_utils::Identifier;

    fn function(name: &'static str) -> ScheduledCallback {
        ScheduledCallback::Function(Identifier::vanilla_static(name))
    }

    fn tag(name: &'static str) -> ScheduledCallback {
        ScheduledCallback::Tag(Identifier::vanilla_static(name))
    }

    #[test]
    fn due_entries_are_popped_in_tick_order() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("b"), 40, ScheduleMode::Replace);
        registry.schedule(function("a"), 10, ScheduleMode::Replace);
        let due = registry.take_due(40);
        assert_eq!(due.len(), 2);
        assert_eq!(due[0], function("a"));
        assert_eq!(due[1], function("b"));
        assert!(registry.is_empty());
    }

    #[test]
    fn entries_not_yet_due_stay_queued() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 100, ScheduleMode::Replace);
        assert_eq!(registry.take_due(99), [] as [ScheduledCallback; 0]);
        assert_eq!(registry.pending().len(), 1);
        assert_eq!(registry.take_due(100).len(), 1);
    }

    #[test]
    fn replace_drops_the_previous_entry() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 10, ScheduleMode::Replace);
        registry.schedule(function("a"), 20, ScheduleMode::Replace);
        assert_eq!(registry.take_due(10), [] as [ScheduledCallback; 0]);
        assert_eq!(registry.take_due(20).len(), 1);
    }

    #[test]
    fn append_keeps_both_entries() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 10, ScheduleMode::Append);
        registry.schedule(function("a"), 20, ScheduleMode::Append);
        assert_eq!(registry.take_due(20).len(), 2);
    }

    #[test]
    fn append_ignores_an_identical_pair() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 10, ScheduleMode::Append);
        registry.schedule(function("a"), 10, ScheduleMode::Append);
        assert_eq!(registry.pending().len(), 1);
    }

    #[test]
    fn clear_removes_every_entry_for_a_function() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 10, ScheduleMode::Append);
        registry.schedule(function("a"), 20, ScheduleMode::Append);
        registry.schedule(function("b"), 15, ScheduleMode::Replace);
        assert_eq!(registry.clear(&Identifier::vanilla_static("a")), 2);
        assert_eq!(registry.clear(&Identifier::vanilla_static("a")), 0);
        assert_eq!(registry.pending().len(), 1);
    }

    #[test]
    fn tags_schedule_under_a_hash_key() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(tag("load"), 5, ScheduleMode::Replace);
        assert_eq!(registry.pending_ids(), vec!["#minecraft:load".to_owned()]);
        assert_eq!(registry.take_due(5)[0], tag("load"));
    }

    #[test]
    fn delays_use_vanilla_units() {
        assert_eq!(parse_delay("10").unwrap(), 10);
        assert_eq!(parse_delay("10t").unwrap(), 10);
        assert_eq!(parse_delay("5s").unwrap(), 100);
        assert_eq!(parse_delay("1d").unwrap(), 24_000);
        assert!(parse_delay("-1t").is_err());
        assert!(parse_delay("abc").is_err());
    }
}
