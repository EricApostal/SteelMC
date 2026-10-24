//! Vanilla `SlotAccess`: a live read/write handle on one item slot.
//!
//! Mirrors `net.minecraft.world.entity.SlotAccess` without its vanilla-only
//! combat hooks (`openTextEdit`, `stabAttack`, `doesEmitEquipEvent`), which
//! only exist for sign text editing and mace-style stab attacks.

use steel_registry::item_stack::ItemStack;

/// Vanilla `SlotAccess`: one live item slot.
///
/// `get`/`set` take `&self` (vanilla's no-mutability signature) because a
/// slot is a *handle*, not a value: containers, equipment racks, and component
/// views alias their backing store. Implementations therefore mutate through
/// interior state, exactly like vanilla's closures over a `Container`,
/// `LivingEntity`, or `ItemStack` component.
pub(crate) trait SlotAccess {
    /// Vanilla `SlotAccess.get`.
    fn get(&self) -> ItemStack;
    /// Vanilla `SlotAccess.set`: `false` means the slot refused the stack.
    fn set(&self, stack: ItemStack) -> bool;
}

/// Vanilla `SlotAccess.of`: builds an access from a getter/setter pair.
pub(crate) struct FnSlotAccess<G, S> {
    get: G,
    set: S,
}

impl<G, S> FnSlotAccess<G, S> {
    /// Creates an access from getter and setter closures.
    pub(crate) const fn new(get: G, set: S) -> Self {
        Self { get, set }
    }
}

impl<G, S> SlotAccess for FnSlotAccess<G, S>
where
    G: Fn() -> ItemStack,
    S: Fn(ItemStack) -> bool,
{
    fn get(&self) -> ItemStack {
        (self.get)()
    }

    fn set(&self, stack: ItemStack) -> bool {
        (self.set)(stack)
    }
}
