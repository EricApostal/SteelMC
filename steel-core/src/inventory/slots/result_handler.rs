use std::sync::Arc;

use steel_registry::item_stack::ItemStack;

use crate::{
    inventory::lock::{ContainerLockGuard, ContainerRef},
    player::Player,
};

/// A trait for recipe handlers that update slots in containers according to recipes
pub trait ResultHandler: Send + Sync {
    /// The container the result is written to and read from.
    ///
    /// [`ResultSlot::new`](crate::inventory::slots::ResultSlot::new) derives the
    /// slot's container from this. Menu builders reuse that exact reference for
    /// validation and locking, so handler writes and slot reads cannot target
    /// different containers.
    fn result_container(&self) -> ContainerRef;

    /// Auxiliary containers accessed while validating or taking the result.
    ///
    /// The result container itself is already supplied by
    /// [`result_container`](Self::result_container) and must not be repeated.
    fn dependencies(&self) -> Vec<ContainerRef>;

    /// Recalculate the result based on current inputs.
    fn update_result(&self, guard: &mut ContainerLockGuard);

    /// Recalculate the result with the acting player in scope.
    ///
    /// Vanilla's crafting menus recompute their result inside
    /// `slotChangedCraftingGrid`, which can consult the player (`limited_crafting`
    /// is refused for recipes the player has not unlocked). Handlers whose result
    /// does not depend on the player keep the default delegation to
    /// [`update_result`](Self::update_result).
    fn update_result_for(&self, guard: &mut ContainerLockGuard, _player: Option<&Player>) {
        self.update_result(guard);
    }

    /// Consume inputs when the result is taken. Return overflow remainders.
    fn on_result_taken(&self, guard: &mut ContainerLockGuard, player: &Player)
    -> Option<ItemStack>;

    /// Vanilla `ItemStack.onCraftedBy` / `Item.onCraftedPostProcess` for the
    /// stack leaving this slot.
    ///
    /// [`ResultSlot`](super::ResultSlot) calls this on the stack it is about to
    /// hand to the player, before [`on_result_taken`](Self::on_result_taken).
    /// Handlers whose result item rewrites itself when crafted (the filled
    /// map's `map_post_processing` lock/scale) override it; every other menu
    /// keeps the no-op default.
    fn on_result_crafted(&self, _stack: &mut ItemStack) {}

    /// Whether the stored result still matches the current inputs.
    fn is_result_valid(&self, guard: &ContainerLockGuard, player: &Player) -> bool;
}

impl<T: ResultHandler + ?Sized> ResultHandler for Arc<T> {
    fn result_container(&self) -> ContainerRef {
        (**self).result_container()
    }

    fn dependencies(&self) -> Vec<ContainerRef> {
        (**self).dependencies()
    }

    fn update_result(&self, guard: &mut ContainerLockGuard) {
        (**self).update_result(guard);
    }

    fn update_result_for(&self, guard: &mut ContainerLockGuard, player: Option<&Player>) {
        (**self).update_result_for(guard, player);
    }

    fn on_result_taken(
        &self,
        guard: &mut ContainerLockGuard,
        player: &Player,
    ) -> Option<ItemStack> {
        (**self).on_result_taken(guard, player)
    }

    fn on_result_crafted(&self, stack: &mut ItemStack) {
        (**self).on_result_crafted(stack);
    }

    fn is_result_valid(&self, guard: &ContainerLockGuard, player: &Player) -> bool {
        (**self).is_result_valid(guard, player)
    }
}
