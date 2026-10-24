//! Vanilla `SlotSource` evaluation: turning a slot source into slots.
//!
//! Mirrors `net.minecraft.world.item.slot.{SlotSource, SlotSources,
//! EmptySlotSource, RangeSlotSource, TransformedSlotSource, CompositeSlotSource,
//! GroupSlotSource, FilteredSlotSource, LimitSlotSource, ContentsSlotSource}`.
//!
//! Vanilla resolves every slot source through a `LootContext`, reading the
//! `minecraft:container` param (`LootContextParams.CONTAINER`) and the entity
//! targets. Steel's registry-side `LootContext` carries `EntityRef` views with
//! no live slot access, so [`SlotSourceContext`] carries the objects a slot
//! source may read directly. Vanilla also falls back to no slots whenever the
//! referenced context object is absent or is not a `SlotProvider`, which is
//! exactly what an unresolvable target does here.

use steel_registry::{
    REGISTRY, RegistryExt as _,
    item_stack::ItemStack,
    loot_table::ItemPredicate,
    slot_source::{SlotSource, SlotSourceContextArg, SlotSourceHolder},
};

use super::{
    collection::{EmptySlots, SlotCollection, concat, filter, flat_map, limit},
    provider::SlotProvider,
    range::{SlotRange, name_to_range},
};

/// The context objects a slot source can read slots from, mirroring vanilla's
/// `LootContextArg` resolved against a `LootContext`.
#[derive(Clone, Copy, Default)]
pub(crate) struct SlotSourceContext<'a> {
    /// Vanilla `LootContextParams.CONTAINER`.
    pub(crate) container: Option<&'a dyn SlotProvider>,
    /// Vanilla `LootContextParams.THIS_ENTITY`, when it provides slots.
    pub(crate) this_entity: Option<&'a dyn SlotProvider>,
}

impl<'a> SlotSourceContext<'a> {
    /// Vanilla `LootContextArg.get`, restricted to the targets Steel can
    /// resolve live slot access for.
    fn provider(self, arg: SlotSourceContextArg) -> Option<&'a dyn SlotProvider> {
        match arg {
            SlotSourceContextArg::Container => self.container,
            SlotSourceContextArg::This => self.this_entity,
            // The remaining entity/block-entity targets read params that
            // Steel's `LootContext` keeps as registry-side views without live
            // slots; vanilla yields no slots for a target that is not a
            // `SlotProvider`, which is the same fallback.
            SlotSourceContextArg::Attacker
            | SlotSourceContextArg::DirectAttacker
            | SlotSourceContextArg::AttackingPlayer
            | SlotSourceContextArg::TargetEntity
            | SlotSourceContextArg::InteractingEntity
            | SlotSourceContextArg::BlockEntity => None,
        }
    }

    /// Vanilla `RangeSlotSource.provide`.
    pub(crate) fn range(
        self,
        range: &SlotRange,
        source: SlotSourceContextArg,
    ) -> Box<dyn SlotCollection + 'a> {
        match self.provider(source) {
            Some(provider) => provider.get_slots_from_range(range.slots()),
            None => Box::new(EmptySlots),
        }
    }
}

/// Vanilla `RangeSlotSource.slotRange(range)`, the slot source
/// `SlotSourceArgument.LiteralResult` produces: a slot range read from the
/// `container` param.
pub(crate) fn provide_slot_range<'a>(
    range: &'static SlotRange,
    context: &SlotSourceContext<'a>,
) -> Box<dyn SlotCollection + 'a> {
    context.range(range, SlotSourceContextArg::Container)
}

/// Vanilla `SlotSource.provide`.
pub(crate) fn provide_slot_source<'a>(
    source: &'static SlotSource,
    context: &SlotSourceContext<'a>,
) -> Box<dyn SlotCollection + 'a> {
    match source {
        SlotSource::Group(group) => concat(
            group
                .terms
                .iter()
                .map(|term| provide_holder(term, context))
                .collect(),
        ),
        SlotSource::Filtered(filtered) => {
            let predicate: &'static ItemPredicate = &filtered.item_filter;
            filter(
                provide_holder(&filtered.slot_source, context),
                Box::new(move |stack: &ItemStack| predicate.test(stack)),
            )
        }
        SlotSource::Limit(limited) => limit(
            provide_holder(&limited.slot_source, context),
            usize::try_from(limited.limit).unwrap_or(0),
        ),
        SlotSource::SlotRange(range_source) => match name_to_range(range_source.slots) {
            Some(range) => context.range(range, range_source.source),
            // Vanilla rejects an unknown range name while validating a data
            // pack; Steel's registry has no such validation pass.
            None => Box::new(EmptySlots),
        },
        SlotSource::Contents(contents) => flat_map(
            provide_holder(&contents.slot_source, context),
            contents.component,
        ),
        SlotSource::Empty(_) => Box::new(EmptySlots),
    }
}

/// Vanilla `Holder<SlotSource>.value().provide(context)` for a term.
fn provide_holder<'a>(
    holder: &'static SlotSourceHolder,
    context: &SlotSourceContext<'a>,
) -> Box<dyn SlotCollection + 'a> {
    match holder {
        SlotSourceHolder::Inline(source) => provide_slot_source(source, context),
        SlotSourceHolder::Reference(key) => match REGISTRY.slot_sources.by_key(key) {
            Some(entry) => provide_slot_source(&entry.source, context),
            // Vanilla cannot reference an unregistered `slot_source` at
            // runtime: the id fails validation while loading the data pack.
            None => Box::new(EmptySlots),
        },
    }
}
#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use steel_registry::item_stack::ItemStack;

    use super::*;
    use crate::loot::slot::{ArrayItemProvider, FnSlotAccess, SlotAccess, SlotProvider};

    /// A `SlotProvider` over a fixed slot vector, mirroring vanilla's
    /// `Container` slot access.
    struct FakeSlots {
        items: RefCell<Vec<ItemStack>>,
    }

    impl FakeSlots {
        fn new(len: usize) -> Self {
            Self {
                items: RefCell::new(vec![ItemStack::empty(); len]),
            }
        }

        fn with(self, index: usize, stack: ItemStack) -> Self {
            self.items.borrow_mut()[index] = stack;
            self
        }
    }

    impl SlotProvider for FakeSlots {
        fn get_slot(&self, slot: i32) -> Option<Box<dyn SlotAccess + '_>> {
            let index = usize::try_from(slot).ok()?;
            if index >= self.items.borrow().len() {
                return None;
            }
            Some(Box::new(FnSlotAccess::new(
                move || self.items.borrow()[index].clone(),
                move |stack| {
                    self.items.borrow_mut()[index] = stack;
                    true
                },
            )))
        }
    }

    fn stack(count: i32) -> ItemStack {
        ItemStack::with_count(&steel_registry::vanilla_items::STONE, count)
    }

    /// Vanilla `RangeSlotSource.slotRange(range).provide`: a literal slot
    /// source reads the named range from the `container` param and yields
    /// exactly those slots, in vanilla order.
    #[test]
    fn literal_slot_source_reads_named_range() {
        let slots = FakeSlots::new(4).with(0, stack(1)).with(2, stack(2));
        let context = SlotSourceContext {
            container: Some(&slots),
            this_entity: None,
        };
        let range = name_to_range("container.*").expect("container.* range");
        let collection = provide_slot_range(range, &context);
        assert_eq!(collection.size(), 4);
        let copies = collection.item_copies();
        assert_eq!(copies.len(), 4);
        assert_eq!(copies[0].count(), 1);
        assert_eq!(copies[1].count(), 0);
        assert_eq!(copies[2].count(), 2);

        // A single-slot range yields exactly one slot.
        let single = provide_slot_range(name_to_range("container.2").expect("single"), &context);
        assert_eq!(single.size(), 1);
        assert_eq!(single.item_copies()[0].count(), 2);
    }

    /// Vanilla `SlotCollection.replaceSlotItems`: writes walk the collection
    /// in order and stop when the provider is exhausted.
    #[test]
    fn replacing_writes_through_the_slot_range() {
        let slots = FakeSlots::new(3);
        let context = SlotSourceContext {
            container: Some(&slots),
            this_entity: None,
        };
        let collection = provide_slot_range(name_to_range("container.*").expect("range"), &context);
        let mut provider = ArrayItemProvider::new(vec![stack(5)]);
        assert_eq!(collection.replace_all_slot_items(&mut provider), 1);
        let copies = slots.items.borrow().clone();
        assert_eq!(copies[0].count(), 5);
        assert_eq!(copies[1].count(), 0);
    }

    /// Vanilla `SlotCollection.EMPTY`: a slot source with no container param
    /// provides no slots instead of failing.
    #[test]
    fn missing_container_provides_no_slots() {
        let context = SlotSourceContext::default();
        let collection =
            provide_slot_range(name_to_range("weapon.mainhand").expect("range"), &context);
        assert_eq!(collection.size(), 0);
        assert!(collection.item_copies().is_empty());
    }
}
