//! `/schedule` bookkeeping, mirroring vanilla
//! `net.minecraft.server.ServerFunctionManager` and `net.minecraft.util.TimerQueue`.
//!
//! Functions are scheduled onto absolute game ticks and popped when the tick
//! advances past them, exactly like vanilla's `TimerQueue`. Steel keeps the queue
//! free of world types so `/schedule` and the reload path can share it.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap},
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

/// One function scheduled for an absolute tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledFunction {
    /// Function to run.
    pub(crate) function: Identifier,
    /// Absolute game tick it becomes due.
    pub(crate) tick: i64,
}

/// Vanilla `TimerQueue<QualifiedName>` equivalent for scheduled functions.
#[derive(Debug, Default)]
pub(crate) struct ScheduleRegistry {
    queue: BinaryHeap<Reverse<ScheduledEntry>>,
    /// Index of the next due tick per function, so `replace` and `clear` are O(1).
    next_tick: HashMap<Identifier, i64>,
}

impl ScheduleRegistry {
    /// Vanilla `ScheduleCommand.schedule`: queues `function` for `at_tick`.
    ///
    /// Returns `false` when the entry already existed and the mode is `Replace`
    /// only in the sense of replacing it — the return value mirrors vanilla's
    /// "was a new entry added" signal used for the command's feedback text.
    pub(crate) fn schedule(
        &mut self,
        function: Identifier,
        at_tick: i64,
        mode: ScheduleMode,
    ) -> bool {
        let existing = self.next_tick.get(&function).copied();
        if existing.is_some() && mode == ScheduleMode::Append {
            // Vanilla `append` still adds a second entry at the same tick.
            self.queue.push(Reverse(ScheduledEntry {
                tick: at_tick,
                function,
            }));
            return true;
        }
        self.remove(&function);
        self.queue.push(Reverse(ScheduledEntry {
            tick: at_tick,
            function: function.clone(),
        }));
        self.next_tick.insert(function, at_tick);
        true
    }

    /// Vanilla `ScheduleCommand.clear`: drops every entry for `function`.
    ///
    /// Returns whether anything was removed. The heap is rebuilt because vanilla's
    /// `TimerQueue.remove` is also linear in the number of pending entries.
    pub(crate) fn clear(&mut self, function: &Identifier) -> bool {
        if self.next_tick.remove(function).is_none() {
            return false;
        }
        self.queue.retain(|entry| &entry.0.function != function);
        true
    }

    /// Drops every scheduled function, used when a reload replaces the functions.
    pub(crate) fn clear_all(&mut self) {
        self.queue.clear();
        self.next_tick.clear();
    }

    /// Pops every entry due at or before `current_tick`.
    pub(crate) fn take_due(&mut self, current_tick: i64) -> Vec<ScheduledFunction> {
        let mut due = Vec::new();
        while let Some(Reverse(entry)) = self.queue.peek() {
            if entry.tick > current_tick {
                break;
            }
            let Reverse(entry) = self.queue.pop().expect("peeked entry is present");
            if self.next_tick.get(&entry.function) == Some(&entry.tick) {
                self.next_tick.remove(&entry.function);
            }
            due.push(ScheduledFunction {
                function: entry.function,
                tick: entry.tick,
            });
        }
        due
    }

    /// Vanilla `/schedule` feedback: every pending function and its due tick.
    #[must_use]
    pub(crate) fn pending(&self) -> Vec<ScheduledFunction> {
        let mut entries: Vec<ScheduledFunction> = self
            .next_tick
            .iter()
            .map(|(function, &tick)| ScheduledFunction {
                function: function.clone(),
                tick,
            })
            .collect();
        entries.sort_by(|left, right| left.tick.cmp(&right.tick));
        entries
    }

    /// Whether any function is scheduled.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Removes the indexed entry for `function`, leaving heap entries to be skipped.
    fn remove(&mut self, function: &Identifier) {
        self.next_tick.remove(function);
        // Vanilla `TimerQueue.remove` also drops the heap entry, so a replaced schedule
        // can never fire at its old tick.
        self.queue.retain(|entry| &entry.0.function != function);
    }
}

/// One heap entry; `Reverse` makes `BinaryHeap` a min-heap on `tick`.
#[derive(Debug, PartialEq, Eq)]
struct ScheduledEntry {
    tick: i64,
    function: Identifier,
}

impl Ord for ScheduledEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.tick
            .cmp(&other.tick)
            .then_with(|| self.function.cmp(&other.function))
    }
}

impl PartialOrd for ScheduledEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Vanilla `ScheduleCommand.parseDelay`: `10t`, `5s`, `3d` -> ticks.
///
/// # Errors
/// Returns the vanilla-shaped failure reason when the argument is unusable.
pub(crate) fn parse_delay(argument: &str) -> Result<i64, String> {
    let (value, unit) = match argument.chars().last() {
        Some('t') | Some('s') | Some('d') => argument.split_at(argument.len().saturating_sub(1)),
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
mod tests {
    use super::{ScheduleMode, ScheduleRegistry, parse_delay};
    use steel_utils::Identifier;

    fn function(name: &'static str) -> Identifier {
        Identifier::vanilla_static(name)
    }

    #[test]
    fn due_entries_are_popped_in_tick_order() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("b"), 40, ScheduleMode::Replace);
        registry.schedule(function("a"), 10, ScheduleMode::Replace);
        let due = registry.take_due(40);
        assert_eq!(due.len(), 2);
        assert_eq!(due[0].function, function("a"));
        assert_eq!(due[1].function, function("b"));
        assert!(registry.is_empty());
    }

    #[test]
    fn entries_not_yet_due_stay_queued() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 100, ScheduleMode::Replace);
        assert!(registry.take_due(99).is_empty());
        assert_eq!(registry.pending().len(), 1);
        assert_eq!(registry.take_due(100).len(), 1);
    }

    #[test]
    fn replace_drops_the_previous_entry() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 10, ScheduleMode::Replace);
        registry.schedule(function("a"), 20, ScheduleMode::Replace);
        assert!(registry.take_due(10).is_empty());
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
    fn clear_removes_every_entry_for_a_function() {
        let mut registry = ScheduleRegistry::default();
        registry.schedule(function("a"), 10, ScheduleMode::Append);
        registry.schedule(function("a"), 20, ScheduleMode::Append);
        registry.schedule(function("b"), 15, ScheduleMode::Replace);
        assert!(registry.clear(&function("a")));
        assert!(!registry.clear(&function("a")));
        assert_eq!(registry.pending().len(), 1);
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
