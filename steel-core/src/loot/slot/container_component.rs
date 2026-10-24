//! Vanilla `ContainerComponentManipulator`: an item's container component as slots.
//!
//! Mirrors `net.minecraft.world.level.storage.loot.{ContainerComponentManipulator,
//! ContainerComponentManipulators, ContainerComponentManipulator.ComponentSlotCollection}`
//! plus the mutable component views it drives (`ContainerComponent.Mutable`,
//! `SimpleMutableContainer`, `GrowableMutableContainer`).
//!
//! Steel's `ItemStack` keeps the same component helpers as private methods
//! inside `steel-registry` (`ItemStack::container_contents` /
//! `store_container_contents`); this module reads and writes the same three
//! components through their public component types so the slot model can expand
//! a slot into its contents.
//!
//! Intentional divergence: vanilla's bundle mutable view enforces the bundle
//! weight budget while appending. Steel exposes no bundle weight API, so the
//! appends here delegate to `BundleContents::with_contents`, which applies the
//! same weight-limited insert over the whole content list. The result matches
//! for every content list the bundle codec can store.

use std::cell::RefCell;

use steel_registry::{
    ItemStackTemplate,
    data_components::{
        components::{BundleContents, ChargedProjectiles, ItemContainerContents},
        vanilla_components::{BUNDLE_CONTENTS, CHARGED_PROJECTILES, CONTAINER},
    },
    item_stack::ItemStack,
    slot_source::ContainerComponentManipulator,
};

use super::{
    access::SlotAccess,
    collection::{FlatMapper, SlotCollection, SlotModifier},
    item_provider::ItemProvider,
    selector::SlotSelector,
};

impl FlatMapper for ContainerComponentManipulator {
    type Slots<'b> = ComponentSlots<'b>;

    fn map<'b>(&self, stack: &'b mut ItemStack) -> Self::Slots<'b> {
        ComponentSlots::new(stack, *self)
    }
}

/// Vanilla `GrowableMutableContainer.canInsertNewSlots`.
const fn maximum_size(component: ContainerComponentManipulator) -> usize {
    match component {
        ContainerComponentManipulator::Container => ItemContainerContents::MAX_SIZE,
        ContainerComponentManipulator::BundleContents => usize::MAX,
        ContainerComponentManipulator::ChargedProjectiles => ChargedProjectiles::MAX_SIZE,
    }
}

/// Vanilla `ContainerComponent.itemCopies`/`size`: the component's stored
/// stacks, or nothing when the stack has no such component.
fn read_contents(
    component: ContainerComponentManipulator,
    stack: &ItemStack,
) -> Option<Vec<ItemStack>> {
    match component {
        ContainerComponentManipulator::Container => stack.get(CONTAINER).map(|container| {
            container
                .items()
                .iter()
                .map(|slot| {
                    slot.as_ref()
                        .map_or_else(ItemStack::empty, ItemStackTemplate::create)
                })
                .collect()
        }),
        ContainerComponentManipulator::BundleContents => stack.get(BUNDLE_CONTENTS).map(|bundle| {
            bundle
                .items()
                .iter()
                .map(ItemStackTemplate::create)
                .collect()
        }),
        ContainerComponentManipulator::ChargedProjectiles => {
            stack.get(CHARGED_PROJECTILES).map(|projectiles| {
                projectiles
                    .items()
                    .iter()
                    .map(ItemStackTemplate::create)
                    .collect()
            })
        }
    }
}

/// Vanilla `ItemStack.getOrDefault(component, component.empty())`.
fn read_contents_or_empty(
    component: ContainerComponentManipulator,
    stack: &ItemStack,
) -> Vec<ItemStack> {
    read_contents(component, stack).unwrap_or_default()
}

/// Vanilla `ContainerComponentManipulator.setContents`.
fn store_contents(
    component: ContainerComponentManipulator,
    stack: &mut ItemStack,
    items: &[ItemStack],
) {
    match component {
        ContainerComponentManipulator::Container => {
            let templates: Vec<Option<ItemStackTemplate>> = items
                .iter()
                .map(|item| ItemStackTemplate::from_stack(item).ok())
                .collect();
            let last = templates.iter().rposition(Option::is_some);
            let trimmed = last.map_or_else(Vec::new, |last| templates[..=last].to_vec());
            match ItemContainerContents::new(trimmed) {
                Ok(container) => stack.set(CONTAINER, container),
                Err(error) => log::warn!("Could not set container contents: {error}"),
            }
        }
        ContainerComponentManipulator::BundleContents => {
            let bundle = stack.get(BUNDLE_CONTENTS).cloned().unwrap_or_default();
            stack.set(BUNDLE_CONTENTS, bundle.with_contents(items.iter().cloned()));
        }
        ContainerComponentManipulator::ChargedProjectiles => {
            let templates: Vec<ItemStackTemplate> = items
                .iter()
                .filter_map(|item| ItemStackTemplate::from_stack(item).ok())
                .collect();
            match ChargedProjectiles::new(templates) {
                Ok(projectiles) => stack.set(CHARGED_PROJECTILES, projectiles),
                Err(error) => log::warn!("Could not set charged projectiles: {error}"),
            }
        }
    }
}

/// Vanilla `ContainerComponentManipulator.ComponentSlotCollection`.
pub(crate) struct ComponentSlots<'a> {
    stack: RefCell<&'a mut ItemStack>,
    component: ContainerComponentManipulator,
}

impl<'a> ComponentSlots<'a> {
    /// Vanilla `ContainerComponentManipulator.getSlots`.
    pub(crate) const fn new(
        stack: &'a mut ItemStack,
        component: ContainerComponentManipulator,
    ) -> Self {
        Self {
            stack: RefCell::new(stack),
            component,
        }
    }

    fn item_copies_of(&self) -> Vec<ItemStack> {
        read_contents(self.component, &self.stack.borrow()).unwrap_or_default()
    }

    /// Vanilla `Mutable.setItem`.
    fn set_item(&self, index: usize, stack: ItemStack) {
        let mut items = read_contents_or_empty(self.component, &self.stack.borrow());
        let Some(target) = items.get_mut(index) else {
            return;
        };
        *target = stack;
        store_contents(self.component, &mut self.stack.borrow_mut(), &items);
    }

    /// Vanilla `GrowableMutableContainer.replaceSlotItems` with each
    /// component's `addSlotWithItem` rule.
    fn replace_items(
        &self,
        items: &mut Vec<ItemStack>,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        let mut replaced = 0;
        for index in 0..items.len() {
            if !provider.has_next() {
                break;
            }
            if selector.try_select_slot(&items[index]) {
                items[index] = provider.next();
                replaced += 1;
            }
        }
        while items.len() < maximum_size(self.component)
            && provider.has_next()
            && selector.try_select_slot(&ItemStack::empty())
        {
            match self.component {
                ContainerComponentManipulator::Container => items.push(provider.next()),
                ContainerComponentManipulator::ChargedProjectiles => {
                    if !provider.find_next_non_empty() {
                        break;
                    }
                    items.push(provider.next());
                }
                ContainerComponentManipulator::BundleContents => {
                    if !provider.find_next_non_empty() {
                        break;
                    }
                    // Steel's bundle insert applies vanilla's weight budget.
                    let mut candidate = items.clone();
                    candidate.push(provider.peek());
                    let rebuilt = BundleContents::default().with_contents(candidate);
                    if rebuilt.items().len() <= items.len() {
                        break;
                    }
                    items.extend(
                        rebuilt
                            .items()
                            .iter()
                            .map(ItemStackTemplate::create)
                            .skip(items.len()),
                    );
                }
            }
            replaced += 1;
        }
        replaced
    }
}

impl SlotCollection for ComponentSlots<'_> {
    fn item_copies(&self) -> Vec<ItemStack> {
        self.item_copies_of()
    }

    fn size(&self) -> usize {
        self.item_copies_of().len()
    }

    fn replace_slot_items(
        &self,
        provider: &mut dyn ItemProvider,
        selector: &mut dyn SlotSelector,
    ) -> usize {
        let mut items = read_contents_or_empty(self.component, &self.stack.borrow());
        let replaced = self.replace_items(&mut items, provider, selector);
        store_contents(self.component, &mut self.stack.borrow_mut(), &items);
        replaced
    }

    fn modify_slots(&self, modifier: &mut SlotModifier<'_>, selector: &mut dyn SlotSelector) {
        let items = read_contents_or_empty(self.component, &self.stack.borrow());
        for index in 0..items.len() {
            if !selector.try_select_slot(&items[index]) {
                continue;
            }
            let access = ComponentSlotAccess { slots: self, index };
            modifier(&access);
        }
    }
}

/// Vanilla `SimpleMutableContainer`'s per-slot `SlotAccess.of(get, set)`.
struct ComponentSlotAccess<'a, 'b> {
    slots: &'a ComponentSlots<'b>,
    index: usize,
}

impl SlotAccess for ComponentSlotAccess<'_, '_> {
    fn get(&self) -> ItemStack {
        self.slots
            .item_copies_of()
            .get(self.index)
            .cloned()
            .unwrap_or_else(ItemStack::empty)
    }

    fn set(&self, stack: ItemStack) -> bool {
        if self.index >= self.slots.size() {
            return false;
        }
        self.slots.set_item(self.index, stack);
        true
    }
}
