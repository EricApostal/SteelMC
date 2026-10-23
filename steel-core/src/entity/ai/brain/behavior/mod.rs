//! Vanilla behavior tree control.

pub mod builder;
pub mod gate;
pub mod trigger;

pub use trigger::{OneShot, Trigger, TriggerIf, TriggerOneShuffled, TriggerSequence};

use std::any::type_name;

use crate::entity::SharedEntity;
use crate::world::World;

use super::memory::{
    MemoryModuleType, MemoryModuleTypeRef, MemoryStatus, MemoryStore, MemoryValueType,
};

/// Vanilla `Behavior.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BehaviorStatus {
    Stopped,
    Running,
}

/// Vanilla `Brain` state a behavior may touch while it runs.
///
/// Vanilla behaviors reach the brain through `body.getBrain()`. Steel passes the
/// memory store explicitly so a behavior can never re-enter a locked brain.
pub struct BehaviorContext<'a> {
    memories: &'a mut MemoryStore,
    pub level: &'a World,
    pub body: &'a SharedEntity,
    pub timestamp: i64,
}

impl<'a> BehaviorContext<'a> {
    #[must_use]
    pub fn new(
        memories: &'a mut MemoryStore,
        level: &'a World,
        body: &'a SharedEntity,
        timestamp: i64,
    ) -> Self {
        Self {
            memories,
            level,
            body,
            timestamp,
        }
    }

    #[must_use]
    pub fn memories(&self) -> &MemoryStore {
        self.memories
    }

    pub fn memories_mut(&mut self) -> &mut MemoryStore {
        self.memories
    }

    #[must_use]
    pub fn check_memory(&self, ty: MemoryModuleTypeRef, status: MemoryStatus) -> bool {
        self.memories.check(ty, status)
    }

    #[must_use]
    pub fn has_memory_value<T>(&self, ty: &MemoryModuleType<T>) -> bool {
        self.memories
            .check_key(ty.key(), MemoryStatus::ValuePresent)
    }

    #[must_use]
    pub fn get_memory<T: MemoryValueType>(&self, ty: &MemoryModuleType<T>) -> Option<T> {
        self.memories.get(ty)
    }

    pub fn set_memory<T: MemoryValueType>(&mut self, ty: &MemoryModuleType<T>, value: T) {
        self.memories.set(ty, value);
    }

    pub fn set_memory_with_expiry<T: MemoryValueType>(
        &mut self,
        ty: &MemoryModuleType<T>,
        value: T,
        time_to_live: i64,
    ) {
        self.memories.set_with_expiry(ty, value, time_to_live);
    }

    /// Vanilla `MemoryAccessor.setOrErase`.
    pub fn set_memory_or_erase<T: MemoryValueType>(
        &mut self,
        ty: &MemoryModuleType<T>,
        value: Option<T>,
    ) {
        match value {
            Some(value) => self.memories.set(ty, value),
            None => self.memories.erase(ty),
        }
    }

    pub fn erase_memory<T>(&mut self, ty: &MemoryModuleType<T>) {
        self.memories.erase(ty);
    }

    #[must_use]
    pub fn is_memory_value<T: MemoryValueType + PartialEq>(
        &self,
        ty: &MemoryModuleType<T>,
        value: &T,
    ) -> bool {
        self.memories.is_value(ty, value)
    }
}

/// Vanilla `BehaviorControl<E>`.
pub trait BehaviorControl: Send + Sync {
    fn status(&self) -> BehaviorStatus;

    /// Vanilla `getRequiredMemories`: memories that must be registered.
    fn required_memories(&self) -> Vec<MemoryModuleTypeRef>;

    fn try_start(&mut self, ctx: &mut BehaviorContext<'_>) -> bool;

    fn tick_or_stop(&mut self, ctx: &mut BehaviorContext<'_>);

    fn do_stop(&mut self, ctx: &mut BehaviorContext<'_>);

    fn debug_string(&self) -> String;
}

/// Vanilla `Behavior<E>` common state.
pub struct BehaviorState {
    pub entry_condition: Vec<(MemoryModuleTypeRef, MemoryStatus)>,
    status: BehaviorStatus,
    end_timestamp: i64,
    min_duration: i32,
    max_duration: i32,
}

impl BehaviorState {
    #[must_use]
    pub fn new(
        entry_condition: Vec<(MemoryModuleTypeRef, MemoryStatus)>,
        min_duration: i32,
        max_duration: i32,
    ) -> Self {
        Self {
            entry_condition,
            status: BehaviorStatus::Stopped,
            end_timestamp: 0,
            min_duration,
            max_duration,
        }
    }

    /// Vanilla `Behavior(entryCondition)` / `Behavior(entryCondition, timeOutDuration)`
    /// with the default 60-tick duration.
    #[must_use]
    pub fn with_entry_condition(entry_condition: Vec<(MemoryModuleTypeRef, MemoryStatus)>) -> Self {
        Self::new(entry_condition, DEFAULT_DURATION, DEFAULT_DURATION)
    }

    #[must_use]
    pub const fn status(&self) -> BehaviorStatus {
        self.status
    }

    pub(super) fn set_status(&mut self, status: BehaviorStatus) {
        self.status = status;
    }

    #[must_use]
    pub const fn end_timestamp(&self) -> i64 {
        self.end_timestamp
    }

    /// Vanilla `Behavior.timedOut`.
    #[must_use]
    pub const fn timed_out(&self, timestamp: i64) -> bool {
        timestamp > self.end_timestamp
    }
}

/// Vanilla `Behavior.DEFAULT_DURATION`.
pub const DEFAULT_DURATION: i32 = 60;

/// Vanilla `Behavior<E>`: concrete behaviors implement this and automatically
/// become [`BehaviorControl`]s, exactly like `Behavior implements BehaviorControl`.
pub trait Behavior: Send + Sync {
    fn state(&self) -> &BehaviorState;

    fn state_mut(&mut self) -> &mut BehaviorState;

    /// Vanilla `checkExtraStartConditions`.
    fn check_extra_start_conditions(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }

    fn start(&mut self, _ctx: &mut BehaviorContext<'_>) {}

    /// Vanilla `canStillUse`.
    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        false
    }

    fn tick(&mut self, _ctx: &mut BehaviorContext<'_>) {}

    fn stop(&mut self, _ctx: &mut BehaviorContext<'_>) {}
}

/// Vanilla `Behavior.hasRequiredMemories`.
#[must_use]
pub fn has_required_memories(
    ctx: &BehaviorContext<'_>,
    entry_condition: &[(MemoryModuleTypeRef, MemoryStatus)],
) -> bool {
    entry_condition
        .iter()
        .all(|(ty, status)| ctx.check_memory(*ty, *status))
}

impl<B: Behavior + ?Sized> BehaviorControl for B {
    fn status(&self) -> BehaviorStatus {
        self.state().status
    }

    fn required_memories(&self) -> Vec<MemoryModuleTypeRef> {
        self.state()
            .entry_condition
            .iter()
            .map(|(ty, _)| *ty)
            .collect()
    }

    fn try_start(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !has_required_memories(ctx, &self.state().entry_condition) {
            return false;
        }

        if !self.check_extra_start_conditions(ctx) {
            return false;
        }

        let state = self.state();
        let min_duration = state.min_duration;
        let max_duration = state.max_duration;
        let duration = min_duration + rand::random_range(0..=(max_duration - min_duration).max(0));
        let timestamp = ctx.timestamp;

        let state = self.state_mut();
        state.status = BehaviorStatus::Running;
        state.end_timestamp = timestamp + i64::from(duration);

        self.start(ctx);
        true
    }

    fn tick_or_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if !self.state().timed_out(ctx.timestamp) && self.can_still_use(ctx) {
            self.tick(ctx);
        } else {
            self.do_stop(ctx);
        }
    }

    fn do_stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        self.state_mut().status = BehaviorStatus::Stopped;
        self.stop(ctx);
    }

    fn debug_string(&self) -> String {
        type_name::<B>().to_string()
    }
}
