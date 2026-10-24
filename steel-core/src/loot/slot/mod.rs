//! Vanilla slot model for commands and loot functions.
//!
//! Mirrors `net.minecraft.world.item.slot.*` (plus `SlotSelector` and its
//! selectors), `net.minecraft.world.entity.{SlotAccess, SlotProvider}`,
//! `net.minecraft.world.item.ItemProvider`, `net.minecraft.world.inventory.
//! SlotRanges`, and `net.minecraft.world.level.storage.loot.
//! ContainerComponentManipulator`.
//!
//! Two rules keep Steel's port honest while borrowing through Rust:
//!
//! * vanilla `SlotAccess.get`/`set` are live views into game state. Steel's
//!   [`SlotAccess`] keeps that shape (`get(&self)`/`set(&self)`) with interior
//!   mutation, so every `&dyn SlotAccess` stays a live handle exactly like
//!   vanilla's immutable-by-reference accessors.
//! * vanilla `SlotCollection` operations stream lazily; Steel materializes
//!   [`SlotCollection::item_copies`] into a `Vec` because that is also what
//!   vanilla's `toList` call sites consume (e.g. `/item`'s source items).

mod access;
mod collection;
mod container_component;
pub(crate) mod entity;
mod item_provider;
mod provider;
pub(crate) mod range;
mod selector;
mod source;

pub(crate) use access::{FnSlotAccess, SlotAccess};
pub(crate) use collection::SlotCollection;
pub(crate) use entity::EntitySlots;
pub(crate) use item_provider::{ArrayItemProvider, CycledItemProvider, ItemProvider};
pub(crate) use provider::{ContainerSlots, SlotProvider};
pub(crate) use selector::{AnySelector, NonEmptySelector, TrackingSelector};
pub(crate) use source::{SlotSourceContext, provide_slot_range, provide_slot_source};
