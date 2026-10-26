//! Vanilla `ItemProvider`: the stack sequence a slot replacement consumes.
//!
//! Mirrors `net.minecraft.world.item.{ItemProvider, ArrayItemProvider}`, whose
//! two concrete shapes `/item` uses are the plain array walk (`replace` and
//! `override`) and the cycling walk (`fill`).

use steel_registry::item_stack::ItemStack;

/// Vanilla `ItemProvider`.
pub(crate) trait ItemProvider {
    /// Vanilla `ItemProvider.hasNext`.
    fn has_next(&self) -> bool;
    /// Vanilla `ItemProvider.restart`.
    fn restart(&mut self);
    /// Vanilla `ItemProvider.next`. Callers check [`Self::has_next`] first; the
    /// debug assertion marks that precondition instead of panicking in release.
    fn next(&mut self) -> ItemStack;
    /// Vanilla `ItemProvider.peek`.
    fn peek(&mut self) -> ItemStack;
    /// Vanilla `ItemProvider.findNext`.
    fn find_next(&mut self, predicate: &mut dyn FnMut(&ItemStack) -> bool) -> bool;

    /// Vanilla `ItemProvider.findNextNonEmpty`.
    fn find_next_non_empty(&mut self) -> bool {
        self.find_next(&mut |stack| !stack.is_empty())
    }
}

impl dyn ItemProvider {
    /// Vanilla `ItemProvider.orElseProvide`.
    pub(crate) fn or_else_provide(self: Box<Self>, fallback: ItemStack) -> Box<dyn ItemProvider> {
        Box::new(FallbackItemProvider {
            provider: self,
            fallback,
            using_fallback: false,
        })
    }
}

/// Vanilla `ArrayItemProvider`: walks the stacks once.
pub(crate) struct ArrayItemProvider {
    items: Vec<ItemStack>,
    position: usize,
    peeked_item: Option<ItemStack>,
}

impl ArrayItemProvider {
    /// Vanilla `ItemProvider.of`.
    pub(crate) const fn new(items: Vec<ItemStack>) -> Self {
        Self {
            items,
            position: 0,
            peeked_item: None,
        }
    }

    fn set_position(&mut self, position: usize) {
        if self.position != position {
            self.position = position;
            self.peeked_item = None;
        }
    }

    fn compute_next_item(&self) -> ItemStack {
        self.items[self.position].clone()
    }
}

impl ItemProvider for ArrayItemProvider {
    fn has_next(&self) -> bool {
        self.position < self.items.len()
    }

    fn restart(&mut self) {
        self.set_position(0);
    }

    fn next(&mut self) -> ItemStack {
        debug_assert!(self.has_next(), "ArrayItemProvider exhausted");
        let stack = self
            .peeked_item
            .take()
            .unwrap_or_else(|| self.compute_next_item());
        self.position += 1;
        stack
    }

    fn peek(&mut self) -> ItemStack {
        debug_assert!(self.has_next(), "ArrayItemProvider exhausted");
        if self.peeked_item.is_none() {
            self.peeked_item = Some(self.compute_next_item());
        }
        self.peeked_item.clone().expect("peeked item")
    }

    fn find_next(&mut self, predicate: &mut dyn FnMut(&ItemStack) -> bool) -> bool {
        let mut index = self.position;
        while index < self.items.len() {
            if predicate(&self.items[index]) {
                self.set_position(index);
                return true;
            }
            index += 1;
        }
        false
    }
}

/// Vanilla `ArrayItemProvider.Cycled`: walks the stacks repeatedly.
pub(crate) struct CycledItemProvider {
    inner: ArrayItemProvider,
}

impl CycledItemProvider {
    /// Vanilla `ItemProvider.cycle`.
    pub(crate) const fn new(items: Vec<ItemStack>) -> Self {
        Self {
            inner: ArrayItemProvider::new(items),
        }
    }

    const fn len(&self) -> usize {
        self.inner.items.len()
    }
}

impl ItemProvider for CycledItemProvider {
    fn has_next(&self) -> bool {
        self.len() > 0
    }

    fn restart(&mut self) {
        self.inner.set_position(0);
    }

    fn next(&mut self) -> ItemStack {
        debug_assert!(self.has_next(), "CycledItemProvider exhausted");
        let index = self.inner.position % self.len();
        let stack = self.inner.items[index].clone();
        self.inner.position += 1;
        stack
    }

    fn peek(&mut self) -> ItemStack {
        debug_assert!(self.has_next(), "CycledItemProvider exhausted");
        self.inner.items[self.inner.position % self.len()].clone()
    }

    fn find_next(&mut self, predicate: &mut dyn FnMut(&ItemStack) -> bool) -> bool {
        let len = self.len();
        for offset in 0..len {
            let index = (self.inner.position + offset) % len;
            if predicate(&self.inner.items[index]) {
                self.inner.set_position(self.inner.position + offset);
                return true;
            }
        }
        false
    }
}

/// Vanilla `ItemProvider.FallbackItemProvider`.
struct FallbackItemProvider {
    provider: Box<dyn ItemProvider>,
    fallback: ItemStack,
    using_fallback: bool,
}

impl ItemProvider for FallbackItemProvider {
    fn has_next(&self) -> bool {
        true
    }

    fn restart(&mut self) {
        self.provider.restart();
        self.using_fallback = false;
    }

    fn next(&mut self) -> ItemStack {
        if !self.using_fallback && self.provider.has_next() {
            return self.provider.next();
        }
        self.using_fallback = true;
        self.fallback.clone()
    }

    fn peek(&mut self) -> ItemStack {
        if !self.using_fallback && self.provider.has_next() {
            return self.provider.peek();
        }
        self.using_fallback = true;
        self.fallback.clone()
    }

    fn find_next(&mut self, predicate: &mut dyn FnMut(&ItemStack) -> bool) -> bool {
        if self.using_fallback {
            return predicate(&self.fallback);
        }
        if self.provider.find_next(predicate) {
            return true;
        }
        if predicate(&self.fallback) {
            self.using_fallback = true;
            return true;
        }
        false
    }
}
