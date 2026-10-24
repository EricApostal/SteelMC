//! Runtime loot machinery.
//!
//! The slot model shared by `/item`, `execute if items`, and slot-bearing loot
//! functions. Vanilla shape: `net.minecraft.world.item.slot.*`,
//! `net.minecraft.world.entity.{SlotAccess, SlotProvider}`, and
//! `net.minecraft.world.item.ItemProvider`.

pub mod slot;
