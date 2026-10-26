//! Vanilla `SlotCollection`: an ordered, composable set of item slots.
//!
//! Mirrors `net.minecraft.world.item.slot.SlotCollection` and every
//! implementation vanilla ships (`EMPTY`, `of`, `concat`, `Filtered`,
//! `Limited`, `FlatMapped`, plus `ContainerComponentManipulator`'s component
//! view in [`super::container_component`]).
//!
//! Vanilla streams item copies lazily and rejects an exhausted
//! `ItemProvider` by throwing; Steel materializes copies into a `Vec` (the
//! shape every vanilla call site's `toList` produces) and guards the provider
//! precondition with `debug_assert!` (see [`super::item_provider`]).

use steel_registry::item_stack::ItemStack;

use super::{
    access::SlotAccess,
    item_provider::ItemProvider,
    selector::{AnySelector, SlotSelector},
};

/// Vanilla `Consumer<? super SlotAccess>`: the per-slot callback of
/// `SlotCollection.modifySlots`.
pub(crate) type SlotModifier<'m> = dyn FnMut(&dyn SlotAccess) + 'm;

/// Vanilla `SlotCollection`.
pub(crate) trait SlotCollection {
    /// Vanilla `SlotCollection.itemCopies`.
    fn item_copies(&self) -> Vec<ItemStack>;
    /// Vanilla `SlotCollection.size`.
    fn size(&self) -> usize;
    /// Vanilla `SlotCollection.replaceSlotItems(provider, selector)`: returns
    /// how many slots accepted a new stack.
    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize;
    /// Vanilla `SlotCollection.modifySlots(modifier, selector)`.
    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector);
    /// Vanilla `SlotCollection.replaceSlotItems(provider)` (any selector).
    #[cfg(test)]
    fn replace_all_slot_items(&self, provider: &mut dyn ItemProvider) -> usize {
        self.replace_slot_items(provider, &mut AnySelector)
    }
    /// Vanilla `SlotCollection.modifySlots(modifier)` (any selector).
    fn modify_all_slots(&self, modifier: &mut SlotModifier<'_>) {
        self.modify_slots(modifier, &mut AnySelector);
    }
}

/// Vanilla `SlotCollection.EMPTY`.
pub(crate) struct EmptySlots;

impl SlotCollection for EmptySlots {
    fn item_copies(&self) -> Vec<ItemStack> {
        Vec::new()
    }

    fn size(&self) -> usize {
        0
    }

    fn replace_slot_items(
        &self,
        _provider: &mut dyn ItemProvider,
        _selector: &mut dyn SlotSelector,
    ) -> usize {
        0
    }

    fn modify_slots(&self, _modifier: &mut SlotModifier<'_>, _selector: &mut dyn SlotSelector) {}
}

/// Vanilla `SlotCollection.of(SlotAccess)`.
pub(crate) struct SingleSlot<'a> {
    access: Box<dyn SlotAccess + 'a>,
}

impl<'a> SingleSlot<'a> {
    pub(crate) const fn new(access: Box<dyn SlotAccess + 'a>) -> Self {
        Self { access }
    }
}

impl SlotCollection for SingleSlot<'_> {
    fn item_copies(&self) -> Vec<ItemStack> {
        vec![self.access.get()]
    }

    fn size(&self) -> usize {
        1
    }

    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        usize::from(
            provider.has_next()
                && selector.try_select_slot(&self.access.get())
                && self.access.set(provider.next()),
        )
    }

    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector) {
        if selector.try_select_slot(&self.access.get()) {
            modifier(self.access.as_ref());
        }
    }
}

/// Vanilla `SlotCollection.of(Collection<SlotAccess>)` with more than one slot.
pub(crate) struct SlotList<'a> {
    slots: Vec<Box<dyn SlotAccess + 'a>>,
}

impl<'a> SlotList<'a> {
    pub(crate) const fn new(slots: Vec<Box<dyn SlotAccess + 'a>>) -> Self {
        Self { slots }
    }
}

impl SlotCollection for SlotList<'_> {
    fn item_copies(&self) -> Vec<ItemStack> {
        self.slots.iter().map(|slot| slot.get()).collect()
    }

    fn size(&self) -> usize {
        self.slots.len()
    }

    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        let mut replaced = 0;
        for slot in &self.slots {
            if !provider.has_next() {
                break;
            }
            if selector.try_select_slot(&slot.get()) && slot.set(provider.next()) {
                replaced += 1;
            }
        }
        replaced
    }

    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector) {
        for slot in &self.slots {
            if selector.try_select_slot(&slot.get()) {
                modifier(slot.as_ref());
            }
        }
    }
}

/// Vanilla `SlotCollection.concat`.
pub(crate) struct ConcatSlots<'a> {
    terms: Vec<Box<dyn SlotCollection + 'a>>,
}

impl<'a> ConcatSlots<'a> {
    pub(crate) const fn new(terms: Vec<Box<dyn SlotCollection + 'a>>) -> Self {
        Self { terms }
    }
}

impl SlotCollection for ConcatSlots<'_> {
    fn item_copies(&self) -> Vec<ItemStack> {
        self.terms
            .iter()
            .flat_map(|term| term.item_copies())
            .collect()
    }

    fn size(&self) -> usize {
        self.terms.iter().map(|term| term.size()).sum()
    }

    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        self.terms
            .iter()
            .map(|term| term.replace_slot_items(provider, selector))
            .sum()
    }

    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector) {
        for term in &self.terms {
            term.modify_slots(modifier, selector);
        }
    }
}

/// Vanilla `SlotCollection.Filtered`: keeps only slots whose stack matches.
pub(crate) struct FilteredSlots<'a> {
    slots: Box<dyn SlotCollection + 'a>,
    filter: Box<dyn Fn(&ItemStack) -> bool + 'a>,
}

impl<'a> FilteredSlots<'a> {
    pub(crate) const fn new(
        slots: Box<dyn SlotCollection + 'a>,
        filter: Box<dyn Fn(&ItemStack) -> bool + 'a>,
    ) -> Self {
        Self { slots, filter }
    }
}

/// Vanilla `SlotSelector.filter`: narrows a selector with an extra predicate.
///
/// Vanilla implements this on the selector itself; Steel applies it where
/// vanilla calls it (`SlotCollection.Filtered` and `.Limited`), which are its
/// only call sites.
struct FilteringSelector<'s> {
    inner: &'s mut dyn SlotSelector,
    filter: &'s dyn Fn(&ItemStack) -> bool,
}

impl SlotSelector for FilteringSelector<'_> {
    fn try_select_slot(&mut self, stack: &ItemStack) -> bool {
        (self.filter)(stack) && self.inner.try_select_slot(stack)
    }
}

/// Vanilla `SlotSelector.limit`: stops selecting after `limit` slots.
struct LimitingSelector<'s> {
    inner: &'s mut dyn SlotSelector,
    limit: usize,
    selected: usize,
}

impl SlotSelector for LimitingSelector<'_> {
    fn try_select_slot(&mut self, stack: &ItemStack) -> bool {
        if self.selected < self.limit && self.inner.try_select_slot(stack) {
            self.selected += 1;
            true
        } else {
            false
        }
    }
}

impl SlotCollection for FilteredSlots<'_> {
    fn item_copies(&self) -> Vec<ItemStack> {
        self.slots
            .item_copies()
            .into_iter()
            .filter(|stack| (self.filter)(stack))
            .collect()
    }

    fn size(&self) -> usize {
        self.item_copies().len()
    }

    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        let mut filtering = FilteringSelector {
            inner: selector,
            filter: &*self.filter,
        };
        self.slots.replace_slot_items(provider, &mut filtering)
    }

    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector) {
        let mut filtering = FilteringSelector {
            inner: selector,
            filter: &*self.filter,
        };
        self.slots.modify_slots(modifier, &mut filtering);
    }
}

/// Vanilla `SlotCollection.Limited`: keeps at most `limit` slots.
pub(crate) struct LimitedSlots<'a> {
    slots: Box<dyn SlotCollection + 'a>,
    limit: usize,
}

impl<'a> LimitedSlots<'a> {
    pub(crate) const fn new(slots: Box<dyn SlotCollection + 'a>, limit: usize) -> Self {
        Self { slots, limit }
    }
}

impl SlotCollection for LimitedSlots<'_> {
    fn item_copies(&self) -> Vec<ItemStack> {
        let mut copies = self.slots.item_copies();
        copies.truncate(self.limit);
        copies
    }

    fn size(&self) -> usize {
        self.slots.size().min(self.limit)
    }

    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        let mut limiting = LimitingSelector {
            inner: selector,
            limit: self.limit,
            selected: 0,
        };
        self.slots.replace_slot_items(provider, &mut limiting)
    }

    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector) {
        let mut limiting = LimitingSelector {
            inner: selector,
            limit: self.limit,
            selected: 0,
        };
        self.slots.modify_slots(modifier, &mut limiting);
    }
}

/// Vanilla `SlotCollection.FlatMapped`'s mapper: `Function<ItemStack,
/// SlotCollection>` over the *live* stack.
///
/// Vanilla passes the shared mutable `ItemStack` reference so the produced
/// collection writes through to the containing slot; Rust expresses the same
/// contract with a mutable borrow and a generic associated lifetime.
pub(crate) trait FlatMapper {
    /// The collection type produced for one stack.
    type Slots<'b>: SlotCollection;

    /// Vanilla `Function.apply`.
    fn map<'b>(&self, stack: &'b mut ItemStack) -> Self::Slots<'b>;
}

/// Vanilla `SlotCollection.FlatMapped`: expands every slot into more slots.
pub(crate) struct FlatMappedSlots<'a, M: FlatMapper> {
    slots: Box<dyn SlotCollection + 'a>,
    mapper: M,
}

impl<'a, M: FlatMapper> FlatMappedSlots<'a, M> {
    pub(crate) const fn new(slots: Box<dyn SlotCollection + 'a>, mapper: M) -> Self {
        Self { slots, mapper }
    }
}

impl<M: FlatMapper> SlotCollection for FlatMappedSlots<'_, M> {
    fn item_copies(&self) -> Vec<ItemStack> {
        let mut copies = Vec::new();
        for mut stack in self.slots.item_copies() {
            let mapped = self.mapper.map(&mut stack);
            copies.extend(mapped.item_copies());
        }
        copies
    }

    fn size(&self) -> usize {
        self.item_copies().len()
    }

    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        let mut replaced = 0usize;
        self.slots.modify_all_slots(&mut |access| {
            let mut stack = access.get();
            let count = {
                let mapped = self.mapper.map(&mut stack);
                mapped.replace_slot_items(&mut *provider, &mut *selector)
            };
            access.set(stack);
            replaced += count;
        });
        replaced
    }

    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector) {
        self.slots.modify_all_slots(&mut |access| {
            let mut stack = access.get();
            {
                let mapped = self.mapper.map(&mut stack);
                mapped.modify_slots(&mut *modifier, &mut *selector);
            }
            access.set(stack);
        });
    }
}

/// Vanilla `SlotCollection.of(Collection<SlotAccess>)`, which returns `EMPTY`
/// for no slots and a single-slot collection for one.
pub(crate) fn of_many<'a>(
    mut slots: Vec<Box<dyn SlotAccess + 'a>>,
) -> Box<dyn SlotCollection + 'a> {
    match slots.len() {
        0 => Box::new(EmptySlots),
        1 => Box::new(SingleSlot::new(slots.remove(0))),
        _ => Box::new(SlotList::new(slots)),
    }
}

/// Vanilla `SlotCollection.concat(List<SlotCollection>)`.
pub(crate) fn concat<'a>(terms: Vec<Box<dyn SlotCollection + 'a>>) -> Box<dyn SlotCollection + 'a> {
    match terms.len() {
        0 => Box::new(EmptySlots),
        1 => terms.into_iter().next().expect("one term"),
        _ => Box::new(ConcatSlots::new(terms)),
    }
}

/// Vanilla `SlotCollection.filter`.
pub(crate) fn filter<'a>(
    slots: Box<dyn SlotCollection + 'a>,
    predicate: Box<dyn Fn(&ItemStack) -> bool + 'a>,
) -> Box<dyn SlotCollection + 'a> {
    Box::new(FilteredSlots::new(slots, predicate))
}

/// Vanilla `SlotCollection.limit`.
pub(crate) fn limit<'a>(
    slots: Box<dyn SlotCollection + 'a>,
    limit: usize,
) -> Box<dyn SlotCollection + 'a> {
    Box::new(LimitedSlots::new(slots, limit))
}

/// Vanilla `SlotCollection.flatMap`.
pub(crate) fn flat_map<'a, M: FlatMapper + 'a>(
    slots: Box<dyn SlotCollection + 'a>,
    mapper: M,
) -> Box<dyn SlotCollection + 'a> {
    Box::new(FlatMappedSlots::new(slots, mapper))
}
