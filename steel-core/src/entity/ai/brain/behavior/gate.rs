//! Vanilla `GateBehavior` / `RunOne` / `ShufflingList`.

use super::super::memory::MemoryStatus;
use super::BehaviorContext;
use super::BehaviorControl;
use super::BehaviorState;
use super::BehaviorStatus;
use super::has_required_memories;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;

/// Vanilla `ShufflingList.WeightedEntry`.
struct WeightedEntry<T> {
    data: T,
    weight: i32,
    rand_weight: f64,
}

/// Vanilla `ShufflingList`.
pub struct ShufflingList<T> {
    entries: Vec<WeightedEntry<T>>,
}

impl<T> Default for ShufflingList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ShufflingList<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn add(&mut self, data: T, weight: i32) {
        self.entries.push(WeightedEntry {
            data,
            weight,
            rand_weight: 0.0,
        });
    }

    /// Vanilla `ShufflingList.shuffle`: weighted exponential jitter, sorted ascending.
    pub fn shuffle(&mut self) {
        for entry in &mut self.entries {
            let random = rand::random_range(0.0_f32..1.0);
            let weight = entry.weight.max(1);
            entry.rand_weight = -f64::from(random).powf(1.0 / f64::from(weight));
        }

        self.entries
            .sort_by(|a, b| a.rand_weight.total_cmp(&b.rand_weight));
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.entries.iter_mut().map(|entry| &mut entry.data)
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.entries.iter().map(|entry| &entry.data)
    }
}

/// Vanilla `GateBehavior.OrderPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderPolicy {
    Ordered,
    Shuffled,
}

/// Vanilla `GateBehavior.RunningPolicy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningPolicy {
    RunOne,
    TryAll,
}

/// Vanilla `GateBehavior<E>`.
pub struct GateBehavior {
    state: BehaviorState,
    exit_erased_memories: Vec<MemoryModuleTypeRef>,
    order_policy: OrderPolicy,
    running_policy: RunningPolicy,
    behaviors: ShufflingList<Box<dyn BehaviorControl>>,
}

impl GateBehavior {
    #[must_use]
    pub fn new(
        entry_condition: Vec<(MemoryModuleTypeRef, MemoryStatus)>,
        exit_erased_memories: Vec<MemoryModuleTypeRef>,
        order_policy: OrderPolicy,
        running_policy: RunningPolicy,
        behaviors: Vec<(Box<dyn BehaviorControl>, i32)>,
    ) -> Self {
        let mut list = ShufflingList::new();
        for (behavior, weight) in behaviors {
            list.add(behavior, weight);
        }

        Self {
            state: BehaviorState::with_entry_condition(entry_condition),
            exit_erased_memories,
            order_policy,
            running_policy,
            behaviors: list,
        }
    }
}

/// Vanilla `RunOne<E>`: a shuffled gate that starts at most one behavior.
pub struct RunOne(GateBehavior);

impl RunOne {
    /// Vanilla `RunOne(weightedBehaviors)`.
    #[must_use]
    pub fn new(weighted_behaviors: Vec<(Box<dyn BehaviorControl>, i32)>) -> Self {
        Self::with_entry_condition(Vec::new(), weighted_behaviors)
    }

    /// Vanilla `RunOne(entryCondition, weightedBehaviors)`.
    #[must_use]
    pub fn with_entry_condition(
        entry_condition: Vec<(MemoryModuleTypeRef, MemoryStatus)>,
        weighted_behaviors: Vec<(Box<dyn BehaviorControl>, i32)>,
    ) -> Self {
        Self(GateBehavior::new(
            entry_condition,
            Vec::new(),
            OrderPolicy::Shuffled,
            RunningPolicy::RunOne,
            weighted_behaviors,
        ))
    }
}

impl BehaviorControl for RunOne {
    fn status(&self) -> BehaviorStatus {
        self.0.status()
    }

    fn required_memories(&self) -> Vec<MemoryModuleTypeRef> {
        self.0.required_memories()
    }

    fn try_start(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.0.try_start(ctx)
    }

    fn tick_or_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.0.tick_or_stop(ctx);
    }

    fn do_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.0.do_stop(ctx);
    }

    fn debug_string(&self) -> String {
        self.0.debug_string()
    }
}

impl BehaviorControl for GateBehavior {
    fn status(&self) -> BehaviorStatus {
        self.state.status()
    }

    fn required_memories(&self) -> Vec<MemoryModuleTypeRef> {
        let mut memories: Vec<MemoryModuleTypeRef> = self
            .state
            .entry_condition
            .iter()
            .map(|(ty, _)| *ty)
            .collect();
        for behavior in self.behaviors.iter() {
            for ty in behavior.required_memories() {
                if !memories.iter().any(|existing| existing.key() == ty.key()) {
                    memories.push(ty);
                }
            }
        }
        memories
    }

    fn try_start(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !has_required_memories(ctx, &self.state.entry_condition) {
            return false;
        }

        self.state.set_status(BehaviorStatus::Running);

        if self.order_policy == OrderPolicy::Shuffled {
            self.behaviors.shuffle();
        }

        match self.running_policy {
            RunningPolicy::RunOne => {
                for behavior in self.behaviors.iter_mut() {
                    if behavior.status() == BehaviorStatus::Stopped && behavior.try_start(ctx) {
                        break;
                    }
                }
            }
            RunningPolicy::TryAll => {
                for behavior in self.behaviors.iter_mut() {
                    if behavior.status() == BehaviorStatus::Stopped {
                        behavior.try_start(ctx);
                    }
                }
            }
        }

        true
    }

    fn tick_or_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        for behavior in self.behaviors.iter_mut() {
            if behavior.status() == BehaviorStatus::Running {
                behavior.tick_or_stop(ctx);
            }
        }

        let still_running = self
            .behaviors
            .iter()
            .any(|behavior| behavior.status() == BehaviorStatus::Running);
        if !still_running {
            self.do_stop(ctx);
        }
    }

    fn do_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.state.set_status(BehaviorStatus::Stopped);

        for behavior in self.behaviors.iter_mut() {
            if behavior.status() == BehaviorStatus::Running {
                behavior.do_stop(ctx);
            }
        }

        for ty in &self.exit_erased_memories {
            ctx.memories_mut().erase_dyn(*ty);
        }
    }

    fn debug_string(&self) -> String {
        let running: Vec<String> = self
            .behaviors
            .iter()
            .filter(|behavior| behavior.status() == BehaviorStatus::Running)
            .map(|behavior| behavior.debug_string())
            .collect();
        format!("GateBehavior: {running:?}")
    }
}
