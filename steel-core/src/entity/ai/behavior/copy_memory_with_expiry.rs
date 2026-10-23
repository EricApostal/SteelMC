//! Vanilla `CopyMemoryWithExpiry`: copy a memory into another one with a TTL.

use steel_utils::value_providers::UniformIntProvider;
use uuid::Uuid;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::{
    MemoryModuleType, MemoryModuleTypeRef, MemoryStatus, MemoryValueType,
};

/// Vanilla `CopyMemoryWithExpiry.create(Predicate, MemoryModuleType source,
/// MemoryModuleType target, UniformInt durationOfCopy)`.
///
/// Vanilla erases the module type's value type behind a generic `T`; Steel keeps a
/// closed [`MemoryValue`](crate::entity::ai::brain::memory::MemoryValue) enum, so the
/// supported source/target pairs are spelled out by [`CopyValue`]. Every pair vanilla
/// actually copies (`bool`, `i32`, `i64`, `Uuid`, `SharedEntity`) is covered.
#[derive(Clone, Copy)]
enum CopyValue {
    /// `MemoryModuleType<Boolean>` to `MemoryModuleType<Boolean>`.
    Bool {
        source: &'static MemoryModuleType<bool>,
        target: &'static MemoryModuleType<bool>,
    },
    /// `MemoryModuleType<Integer>` to `MemoryModuleType<Integer>`.
    Int {
        source: &'static MemoryModuleType<i32>,
        target: &'static MemoryModuleType<i32>,
    },
    /// `MemoryModuleType<Long>` to `MemoryModuleType<Long>`.
    Long {
        source: &'static MemoryModuleType<i64>,
        target: &'static MemoryModuleType<i64>,
    },
    /// `MemoryModuleType<UUID>` to `MemoryModuleType<UUID>`.
    Uuid {
        source: &'static MemoryModuleType<Uuid>,
        target: &'static MemoryModuleType<Uuid>,
    },
    /// `MemoryModuleType<LivingEntity>` to `MemoryModuleType<LivingEntity>`.
    Entity {
        source: &'static MemoryModuleType<SharedEntity>,
        target: &'static MemoryModuleType<SharedEntity>,
    },
}

impl CopyValue {
    fn source(self) -> MemoryModuleTypeRef {
        match self {
            Self::Bool { source, .. } => source,
            Self::Int { source, .. } => source,
            Self::Long { source, .. } => source,
            Self::Uuid { source, .. } => source,
            Self::Entity { source, .. } => source,
        }
    }

    fn target(self) -> MemoryModuleTypeRef {
        match self {
            Self::Bool { target, .. } => target,
            Self::Int { target, .. } => target,
            Self::Long { target, .. } => target,
            Self::Uuid { target, .. } => target,
            Self::Entity { target, .. } => target,
        }
    }

    fn copy(self, ctx: &mut BehaviorContext<'_>, time_to_live: i64) {
        match self {
            Self::Bool { source, target } => {
                copy_value(ctx, source, target, time_to_live);
            }
            Self::Int { source, target } => {
                copy_value(ctx, source, target, time_to_live);
            }
            Self::Long { source, target } => {
                copy_value(ctx, source, target, time_to_live);
            }
            Self::Uuid { source, target } => {
                copy_value(ctx, source, target, time_to_live);
            }
            Self::Entity { source, target } => {
                copy_value(ctx, source, target, time_to_live);
            }
        }
    }
}

/// `target.setWithExpiry(source.get(), ttl)` for one concrete module type pair.
fn copy_value<T: MemoryValueType>(
    ctx: &mut BehaviorContext<'_>,
    source: &'static MemoryModuleType<T>,
    target: &'static MemoryModuleType<T>,
    time_to_live: i64,
) {
    if let Some(value) = ctx.get_memory(source) {
        ctx.set_memory_with_expiry(target, value, time_to_live);
    }
}

/// Vanilla `CopyMemoryWithExpiry`: while `copy_if_true` accepts the body, copies the
/// source memory into the (absent) target memory with a sampled time to live.
pub struct CopyMemoryWithExpiry {
    copy_if_true: fn(&BehaviorContext<'_>) -> bool,
    copy: CopyValue,
    duration_of_copy: UniformIntProvider,
}

impl CopyMemoryWithExpiry {
    /// Vanilla `CopyMemoryWithExpiry.create(copyIfTrue, sourceMemory, targetMemory,
    /// durationOfCopy)` for an entity-valued memory pair.
    #[must_use]
    pub fn create(
        copy_if_true: fn(&BehaviorContext<'_>) -> bool,
        source: &'static MemoryModuleType<SharedEntity>,
        target: &'static MemoryModuleType<SharedEntity>,
        duration_of_copy: UniformIntProvider,
    ) -> OneShot<Self> {
        Self::build(
            copy_if_true,
            CopyValue::Entity { source, target },
            duration_of_copy,
        )
    }

    /// Vanilla `CopyMemoryWithExpiry.create` for a `boolean` memory pair.
    #[must_use]
    pub fn create_bool(
        copy_if_true: fn(&BehaviorContext<'_>) -> bool,
        source: &'static MemoryModuleType<bool>,
        target: &'static MemoryModuleType<bool>,
        duration_of_copy: UniformIntProvider,
    ) -> OneShot<Self> {
        Self::build(
            copy_if_true,
            CopyValue::Bool { source, target },
            duration_of_copy,
        )
    }

    /// Vanilla `CopyMemoryWithExpiry.create` for an `int` memory pair.
    #[must_use]
    pub fn create_int(
        copy_if_true: fn(&BehaviorContext<'_>) -> bool,
        source: &'static MemoryModuleType<i32>,
        target: &'static MemoryModuleType<i32>,
        duration_of_copy: UniformIntProvider,
    ) -> OneShot<Self> {
        Self::build(
            copy_if_true,
            CopyValue::Int { source, target },
            duration_of_copy,
        )
    }

    /// Vanilla `CopyMemoryWithExpiry.create` for a `long` memory pair.
    #[must_use]
    pub fn create_long(
        copy_if_true: fn(&BehaviorContext<'_>) -> bool,
        source: &'static MemoryModuleType<i64>,
        target: &'static MemoryModuleType<i64>,
        duration_of_copy: UniformIntProvider,
    ) -> OneShot<Self> {
        Self::build(
            copy_if_true,
            CopyValue::Long { source, target },
            duration_of_copy,
        )
    }

    /// Vanilla `CopyMemoryWithExpiry.create` for a `UUID` memory pair.
    #[must_use]
    pub fn create_uuid(
        copy_if_true: fn(&BehaviorContext<'_>) -> bool,
        source: &'static MemoryModuleType<Uuid>,
        target: &'static MemoryModuleType<Uuid>,
        duration_of_copy: UniformIntProvider,
    ) -> OneShot<Self> {
        Self::build(
            copy_if_true,
            CopyValue::Uuid { source, target },
            duration_of_copy,
        )
    }

    fn build(
        copy_if_true: fn(&BehaviorContext<'_>) -> bool,
        copy: CopyValue,
        duration_of_copy: UniformIntProvider,
    ) -> OneShot<Self> {
        let source = copy.source();
        let target = copy.target();
        OneShot::new(Self {
            copy_if_true,
            copy,
            duration_of_copy,
        })
        .with_required_memories(vec![source, target])
    }
}

impl Trigger for CopyMemoryWithExpiry {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let source = self.copy.source();
        let target = self.copy.target();

        // Vanilla entry conditions: `present(sourceMemory)`, `absent(targetMemory)`.
        if !ctx.check_memory(source, MemoryStatus::ValuePresent)
            || !ctx.check_memory(target, MemoryStatus::ValueAbsent)
        {
            return false;
        }

        if !(self.copy_if_true)(ctx) {
            return false;
        }

        let duration = self.duration_of_copy;
        let time_to_live = i64::from(rand::random_range(
            duration.min_inclusive..=duration.max_inclusive,
        ));
        self.copy.copy(ctx, time_to_live);
        true
    }
}
