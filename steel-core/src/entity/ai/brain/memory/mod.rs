//! Vanilla `MemoryModuleType` / `MemorySlot` / `MemoryMap` storage.
//!
//! Vanilla keys memories by a generic `MemoryModuleType<?>` whose value type is
//! erased by the JVM. Steel keeps the same typed handle ([`MemoryModuleType<T>`])
//! and stores values as a closed [`MemoryValue`] enum. A closed enum keeps the
//! foundation free of `Any`/`TypeId` downcasting while still letting the storage
//! tick expiry and clear erased slots.

use std::marker::PhantomData;

use glam::DVec3;
use rustc_hash::FxHashMap;
use steel_utils::{BlockPos, GlobalPos, Identifier};
use uuid::Uuid;

use crate::entity::SharedEntity;
use crate::entity::ai::path::Path;
use crate::entity::damage::DamageSource;

pub mod module_type;
pub mod nearest_visible;

use nearest_visible::NearestVisibleLivingEntities;

use super::tracker::WalkTarget;

/// Vanilla `MemoryStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryStatus {
    ValuePresent,
    ValueAbsent,
    Registered,
}

/// A memory module type with the value type erased, so it can be named where the
/// value type is irrelevant (activity requirements, behavior entry conditions).
pub trait MemoryModuleTypeHandle: Send + Sync {
    fn key(&self) -> &Identifier;
}

/// A borrowed erased memory module type.
pub type MemoryModuleTypeRef = &'static dyn MemoryModuleTypeHandle;

/// Vanilla `MemoryModuleType<U>`.
pub struct MemoryModuleType<T> {
    key: Identifier,
    _marker: PhantomData<fn() -> T>,
}

impl<T> MemoryModuleType<T> {
    #[must_use]
    pub const fn new(key: Identifier) -> Self {
        Self {
            key,
            _marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn key(&self) -> &Identifier {
        &self.key
    }
}

impl<T: 'static> MemoryModuleTypeHandle for MemoryModuleType<T> {
    fn key(&self) -> &Identifier {
        &self.key
    }
}

/// A stored memory value, erased over the module type's generic parameter.
#[derive(Clone)]
pub enum MemoryValue {
    Unit,
    Bool(bool),
    Int(i32),
    Long(i64),
    GlobalPos(GlobalPos),
    GlobalPosList(Vec<GlobalPos>),
    BlockPos(BlockPos),
    Vec3(DVec3),
    Uuid(Uuid),
    Entity(SharedEntity),
    EntityList(Vec<SharedEntity>),
    DamageSource(DamageSource),
    Path(Path),
    WalkTarget(WalkTarget),
    PositionTracker(std::sync::Arc<dyn super::tracker::PositionTracker>),
    NearestVisibleLivingEntities(NearestVisibleLivingEntities),
}

impl MemoryValue {
    /// Vanilla `Brain.isEmptyCollection`: empty collections are stored as absent.
    #[must_use]
    pub fn is_empty_collection(&self) -> bool {
        match self {
            Self::GlobalPosList(values) => values.is_empty(),
            Self::EntityList(values) => values.is_empty(),
            _ => false,
        }
    }
}

/// Conversion between a module type's Rust value type and [`MemoryValue`].
pub trait MemoryValueType: Clone + Send + Sync + 'static {
    fn into_memory_value(self) -> MemoryValue;

    fn from_memory_value(value: &MemoryValue) -> Option<Self>;
}

macro_rules! simple_memory_value {
    ($ty:ty, $variant:ident) => {
        impl MemoryValueType for $ty {
            fn into_memory_value(self) -> MemoryValue {
                MemoryValue::$variant(self)
            }

            fn from_memory_value(value: &MemoryValue) -> Option<Self> {
                match value {
                    MemoryValue::$variant(inner) => Some(inner.clone()),
                    _ => None,
                }
            }
        }
    };
}

impl MemoryValueType for () {
    fn into_memory_value(self) -> MemoryValue {
        MemoryValue::Unit
    }

    fn from_memory_value(value: &MemoryValue) -> Option<Self> {
        match value {
            MemoryValue::Unit => Some(()),
            _ => None,
        }
    }
}

simple_memory_value!(bool, Bool);
simple_memory_value!(i32, Int);
simple_memory_value!(i64, Long);
simple_memory_value!(GlobalPos, GlobalPos);
simple_memory_value!(Vec<GlobalPos>, GlobalPosList);
simple_memory_value!(BlockPos, BlockPos);
simple_memory_value!(DVec3, Vec3);
simple_memory_value!(Uuid, Uuid);
simple_memory_value!(Path, Path);
simple_memory_value!(WalkTarget, WalkTarget);
simple_memory_value!(NearestVisibleLivingEntities, NearestVisibleLivingEntities);

impl MemoryValueType for SharedEntity {
    fn into_memory_value(self) -> MemoryValue {
        MemoryValue::Entity(self)
    }

    fn from_memory_value(value: &MemoryValue) -> Option<Self> {
        match value {
            MemoryValue::Entity(entity) => Some(entity.clone()),
            _ => None,
        }
    }
}

impl MemoryValueType for Vec<SharedEntity> {
    fn into_memory_value(self) -> MemoryValue {
        MemoryValue::EntityList(self)
    }

    fn from_memory_value(value: &MemoryValue) -> Option<Self> {
        match value {
            MemoryValue::EntityList(entities) => Some(entities.clone()),
            _ => None,
        }
    }
}

impl MemoryValueType for DamageSource {
    fn into_memory_value(self) -> MemoryValue {
        MemoryValue::DamageSource(self)
    }

    fn from_memory_value(value: &MemoryValue) -> Option<Self> {
        match value {
            MemoryValue::DamageSource(source) => Some(source.clone()),
            _ => None,
        }
    }
}

impl MemoryValueType for std::sync::Arc<dyn super::tracker::PositionTracker> {
    fn into_memory_value(self) -> MemoryValue {
        MemoryValue::PositionTracker(self)
    }

    fn from_memory_value(value: &MemoryValue) -> Option<Self> {
        match value {
            MemoryValue::PositionTracker(tracker) => Some(tracker.clone()),
            _ => None,
        }
    }
}

/// Vanilla `MemorySlot`: a value plus its remaining time to live.
#[derive(Clone)]
struct MemorySlot {
    value: Option<MemoryValue>,
    time_to_live: i64,
}

impl MemorySlot {
    const NEVER_EXPIRE: i64 = i64::MAX;

    fn create() -> Self {
        Self {
            value: None,
            time_to_live: Self::NEVER_EXPIRE,
        }
    }

    fn tick(&mut self) {
        if self.value.is_some() && self.time_to_live != Self::NEVER_EXPIRE {
            if self.time_to_live <= 0 {
                self.clear();
            } else {
                self.time_to_live -= 1;
            }
        }
    }

    fn set(&mut self, value: MemoryValue, time_to_live: i64) {
        self.value = Some(value);
        self.time_to_live = time_to_live;
    }

    fn set_unexpiring(&mut self, value: MemoryValue) {
        self.set(value, Self::NEVER_EXPIRE);
    }

    fn clear(&mut self) {
        self.value = None;
        self.time_to_live = Self::NEVER_EXPIRE;
    }

    fn has_value(&self) -> bool {
        self.value.is_some()
    }
}

/// Vanilla `Brain.memories`: the registered memory map of a single brain.
#[derive(Clone, Default)]
pub struct MemoryStore {
    slots: FxHashMap<Identifier, MemorySlot>,
}

impl MemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a memory slot if it is not present yet.
    pub fn register(&mut self, ty: MemoryModuleTypeRef) {
        self.slots
            .entry(ty.key().clone())
            .or_insert_with(MemorySlot::create);
    }

    pub fn clear(&mut self) {
        for slot in self.slots.values_mut() {
            slot.clear();
        }
    }

    #[must_use]
    pub fn check(&self, ty: MemoryModuleTypeRef, status: MemoryStatus) -> bool {
        self.check_key(ty.key(), status)
    }

    #[must_use]
    pub fn check_key(&self, key: &Identifier, status: MemoryStatus) -> bool {
        let Some(slot) = self.slots.get(key) else {
            return false;
        };

        match status {
            MemoryStatus::Registered => true,
            MemoryStatus::ValuePresent => slot.has_value(),
            MemoryStatus::ValueAbsent => !slot.has_value(),
        }
    }

    #[must_use]
    pub fn has_value(&self, ty: MemoryModuleTypeRef) -> bool {
        self.check(ty, MemoryStatus::ValuePresent)
    }

    #[must_use]
    pub fn get<T: MemoryValueType>(&self, ty: &MemoryModuleType<T>) -> Option<T> {
        self.slots
            .get(ty.key())
            .and_then(|slot| slot.value.as_ref())
            .and_then(T::from_memory_value)
    }

    pub fn set<T: MemoryValueType>(&mut self, ty: &MemoryModuleType<T>, value: T) {
        self.set_value(ty.key(), value.into_memory_value(), None);
    }

    pub fn set_with_expiry<T: MemoryValueType>(
        &mut self,
        ty: &MemoryModuleType<T>,
        value: T,
        time_to_live: i64,
    ) {
        self.set_value(ty.key(), value.into_memory_value(), Some(time_to_live));
    }

    pub fn set_value(&mut self, key: &Identifier, value: MemoryValue, time_to_live: Option<i64>) {
        if value.is_empty_collection() {
            self.erase_key(key);
            return;
        }

        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };

        match time_to_live {
            Some(time_to_live) => slot.set(value, time_to_live),
            None => slot.set_unexpiring(value),
        }
    }

    pub fn erase<T>(&mut self, ty: &MemoryModuleType<T>) {
        self.erase_key(ty.key());
    }

    pub fn erase_key(&mut self, key: &Identifier) {
        if let Some(slot) = self.slots.get_mut(key) {
            slot.clear();
        }
    }

    pub fn erase_dyn(&mut self, ty: MemoryModuleTypeRef) {
        self.erase_key(ty.key());
    }

    #[must_use]
    pub fn is_value<T: MemoryValueType + PartialEq>(
        &self,
        ty: &MemoryModuleType<T>,
        value: &T,
    ) -> bool {
        self.get(ty).is_some_and(|current| &current == value)
    }

    #[must_use]
    pub fn time_to_live<T>(&self, ty: &MemoryModuleType<T>) -> i64 {
        self.slots
            .get(ty.key())
            .map_or(MemorySlot::NEVER_EXPIRE, |slot| slot.time_to_live)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub(super) fn forget_outdated(&mut self) {
        for slot in self.slots.values_mut() {
            slot.tick();
        }
    }
}
