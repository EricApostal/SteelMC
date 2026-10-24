//! Vanilla `SlotSelector`: which slots a `SlotCollection` operation touches.
//!
//! Mirrors `net.minecraft.world.item.slot.SlotSelector` exactly: the static
//! instances `ANY_SLOT`/`EMPTY_SLOTS`/`NON_EMPTY_SLOTS`, selecting,
//! tracking, and the `filter`/`limit` combinators.

use std::{cell::Cell, rc::Rc};

use steel_registry::item_stack::ItemStack;

/// Vanilla `SlotSelector`: a predicate run against each slot's stack.
pub(crate) trait SlotSelector {
    /// Vanilla `SlotSelector.trySelectSlot`.
    fn try_select_slot(&mut self, stack: &ItemStack) -> bool;
}

/// Vanilla `SlotSelector.ANY_SLOT`.
#[derive(Debug, Default)]
pub(crate) struct AnySelector;

impl SlotSelector for AnySelector {
    fn try_select_slot(&mut self, _stack: &ItemStack) -> bool {
        true
    }
}

/// Vanilla `SlotSelector.EMPTY_SLOTS` (`ItemStack::isEmpty`).
#[allow(dead_code, reason = "vanilla SlotSelector API surface")]
#[derive(Debug, Default)]
pub(crate) struct EmptySelector;

impl SlotSelector for EmptySelector {
    fn try_select_slot(&mut self, stack: &ItemStack) -> bool {
        stack.is_empty()
    }
}

/// Vanilla `SlotSelector.NON_EMPTY_SLOTS` (`!ItemStack::isEmpty`).
#[derive(Debug, Default)]
pub(crate) struct NonEmptySelector;

impl SlotSelector for NonEmptySelector {
    fn try_select_slot(&mut self, stack: &ItemStack) -> bool {
        !stack.is_empty()
    }
}

/// Vanilla `SlotSelector.selecting(predicate)` / `FilteringSelector`.
#[allow(dead_code, reason = "vanilla SlotSelector API surface")]
pub(crate) struct SelectingSelector<F> {
    filter: F,
}

impl<F> SelectingSelector<F> {
    /// Mirrors `SlotSelector.selecting`.
    #[allow(dead_code, reason = "vanilla SlotSelector API surface")]
    pub(crate) const fn new(filter: F) -> Self {
        Self { filter }
    }
}

impl<F> SlotSelector for SelectingSelector<F>
where
    F: FnMut(&ItemStack) -> bool,
{
    fn try_select_slot(&mut self, stack: &ItemStack) -> bool {
        (self.filter)(stack)
    }
}

/// Vanilla `SlotSelector.TrackingSelector`: selects at most `limit` slots and
/// counts selections into a shared counter.
pub(crate) struct TrackingSelector {
    selector: Box<dyn SlotSelector>,
    limit: usize,
    selected: Rc<Cell<usize>>,
}

impl TrackingSelector {
    /// Mirrors `SlotSelector.tracking(selector, count)`.
    pub(crate) fn tracking(
        selector: impl SlotSelector + 'static,
        selected: &Rc<Cell<usize>>,
    ) -> Self {
        Self {
            selector: Box::new(selector),
            limit: usize::MAX,
            selected: Rc::clone(selected),
        }
    }

    /// Mirrors `SlotSelector.limit(int)` on the tracking selector.
    #[allow(dead_code, reason = "vanilla SlotSelector API surface")]
    pub(crate) fn limit(self, limit: usize) -> Self {
        if limit >= self.limit {
            return self;
        }
        Self {
            selector: self.selector,
            limit,
            selected: self.selected,
        }
    }

    /// Returns how many slots this tracker has selected.
    #[allow(dead_code, reason = "vanilla SlotSelector API surface")]
    pub(crate) fn selected_count(&self) -> usize {
        self.selected.get()
    }
}

impl SlotSelector for TrackingSelector {
    fn try_select_slot(&mut self, stack: &ItemStack) -> bool {
        if self.selected.get() < self.limit && self.selector.try_select_slot(stack) {
            self.selected.set(self.selected.get() + 1);
            true
        } else {
            false
        }
    }
}
