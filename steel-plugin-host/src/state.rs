//! Process-wide host state: loaded worlds, owned object handles, pending
//! registrations, and the last error message.

use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};

use simdnbt::owned::NbtCompound;
use steel_abi::{
    STEEL_REF_ITEM_STACK, STEEL_REF_NBT, SteelBlockEntityHooks, SteelBlockHooks, SteelEntityHooks,
    SteelItemHooks, SteelRef, SteelStrView,
};
use steel_core::world::World;
use steel_registry::item_stack::ItemStack;
use steel_utils::Identifier;
use steel_utils::locks::{SyncMutex, SyncRwLock};

thread_local! {
    static LAST_ERROR: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Stores a human-readable error for the current thread and mirrors it to the log.
pub fn set_last_error(message: impl Into<String>) {
    let message = message.into();
    log::error!("extension ABI error: {message}");
    LAST_ERROR.with(|slot| *slot.borrow_mut() = message.into_bytes());
}

/// Returns the last error message for this thread. The view is valid until the
/// next error on the same thread.
#[must_use]
pub fn last_error_view() -> SteelStrView {
    LAST_ERROR.with(|slot| {
        let bytes = slot.borrow();
        SteelStrView {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    })
}

/// Copies a string into a per-thread scratch buffer and returns a borrowed view.
/// The view is invalidated by the next call to `scratch_view` on the same thread.
#[must_use]
pub fn scratch_view(value: &str) -> SteelStrView {
    SCRATCH.with(|slot| {
        let mut bytes = slot.borrow_mut();
        bytes.clear();
        bytes.extend_from_slice(value.as_bytes());
        SteelStrView {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    })
}

/// Bidirectional world table. World handles are indices; the pointer map keeps
/// the same world stable across callbacks.
#[derive(Default)]
pub struct WorldTable {
    worlds: Vec<Arc<World>>,
    by_ptr: FxHashMap<usize, u32>,
}

impl WorldTable {
    pub fn id_for(&mut self, world: &Arc<World>) -> u32 {
        let key = Arc::as_ptr(world) as usize;
        if let Some(id) = self.by_ptr.get(&key) {
            return *id;
        }
        let id = u32::try_from(self.worlds.len()).unwrap_or(u32::MAX);
        self.worlds.push(Arc::clone(world));
        self.by_ptr.insert(key, id);
        id
    }

    #[must_use]
    pub fn get(&self, id: u32) -> Option<Arc<World>> {
        self.worlds.get(id as usize).cloned()
    }
}

/// An owned object addressed by a generation-tagged handle.
pub enum OwnedObject {
    ItemStack(ItemStack),
    Nbt(NbtCompound),
}

struct OwnedSlot {
    generation: u32,
    value: Option<OwnedObject>,
}

/// Slab of owned objects handed to extensions.
#[derive(Default)]
pub struct OwnedTable {
    slots: Vec<OwnedSlot>,
    free: Vec<u32>,
}

impl OwnedTable {
    pub fn insert(&mut self, kind: u32, value: OwnedObject) -> SteelRef {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            SteelRef::owned(kind, index, slot.generation)
        } else {
            let index = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);
            self.slots.push(OwnedSlot {
                generation: 1,
                value: Some(value),
            });
            SteelRef::owned(kind, index, 1)
        }
    }

    pub fn insert_item_stack(&mut self, stack: ItemStack) -> SteelRef {
        self.insert(STEEL_REF_ITEM_STACK, OwnedObject::ItemStack(stack))
    }

    pub fn insert_nbt(&mut self, nbt: NbtCompound) -> SteelRef {
        self.insert(STEEL_REF_NBT, OwnedObject::Nbt(nbt))
    }

    fn slot(&self, reference: SteelRef) -> Option<&OwnedSlot> {
        let slot = self.slots.get(reference.index as usize)?;
        (slot.generation == reference.generation)
            .then_some(slot)
            .filter(|slot| slot.value.is_some())
    }

    #[must_use]
    pub fn get(&self, reference: SteelRef) -> Option<&OwnedObject> {
        self.slot(reference)?.value.as_ref()
    }

    pub fn get_mut(&mut self, reference: SteelRef) -> Option<&mut OwnedObject> {
        self.slots
            .get_mut(reference.index as usize)
            .filter(|slot| slot.generation == reference.generation && slot.value.is_some())
            .and_then(|slot| slot.value.as_mut())
    }

    pub fn clone_ref(&mut self, reference: SteelRef) -> Option<SteelRef> {
        match self.get(reference)? {
            OwnedObject::ItemStack(stack) => Some(self.insert_item_stack(stack.clone())),
            OwnedObject::Nbt(nbt) => Some(self.insert_nbt(nbt.clone())),
        }
    }

    pub fn remove(&mut self, reference: SteelRef) -> bool {
        let Some(slot) = self.slots.get_mut(reference.index as usize) else {
            return false;
        };
        if slot.generation != reference.generation || slot.value.is_none() {
            return false;
        }
        slot.value = None;
        slot.generation = slot.generation.wrapping_add(1).max(1);
        self.free.push(reference.index);
        true
    }
}

/// A block registered by an extension, retained until the behavior phase.
pub struct BlockRegistration {
    pub key: Identifier,
    pub hooks: SteelBlockHooks,
}

/// An item registered by an extension, retained until the behavior phase.
pub struct ItemRegistration {
    pub key: Identifier,
    pub hooks: SteelItemHooks,
}

/// A block-entity type registered by an extension.
pub struct BlockEntityRegistration {
    pub key: Identifier,
    pub hooks: SteelBlockEntityHooks,
}

/// An entity type registered by an extension.
pub struct EntityRegistration {
    pub key: Identifier,
    pub hooks: SteelEntityHooks,
}

// SAFETY: the hook tables are immutable after registration. The extension
// contract requires `user_data` to remain valid and thread-safe for the
// lifetime of the loaded library.
unsafe impl Send for BlockRegistration {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for BlockRegistration {}
// SAFETY: see the `BlockRegistration` comment above.
unsafe impl Send for ItemRegistration {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for ItemRegistration {}
// SAFETY: see the `BlockRegistration` comment above.
unsafe impl Send for BlockEntityRegistration {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for BlockEntityRegistration {}
// SAFETY: see the `BlockRegistration` comment above.
unsafe impl Send for EntityRegistration {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for EntityRegistration {}

#[derive(Default)]
pub struct Registrations {
    pub blocks: Vec<BlockRegistration>,
    pub items: Vec<ItemRegistration>,
    pub block_entities: Vec<BlockEntityRegistration>,
    pub entities: Vec<EntityRegistration>,
}

/// Process-wide host state.
pub struct HostState {
    pub worlds: SyncRwLock<WorldTable>,
    pub owned: SyncMutex<OwnedTable>,
    pub registrations: SyncMutex<Registrations>,
    phase: AtomicU32,
}

impl HostState {
    fn new() -> Self {
        Self {
            worlds: SyncRwLock::new(WorldTable::default()),
            owned: SyncMutex::new(OwnedTable::default()),
            registrations: SyncMutex::new(Registrations::default()),
            phase: AtomicU32::new(steel_abi::STEEL_LEVEL_CONTENT),
        }
    }

    pub fn set_phase(&self, phase: u32) {
        self.phase.store(phase, Ordering::Release);
    }

    #[must_use]
    pub fn phase(&self) -> u32 {
        self.phase.load(Ordering::Acquire)
    }
}

pub static HOST: LazyLock<HostState> = LazyLock::new(HostState::new);

#[must_use]
pub fn world_id(world: &Arc<World>) -> u32 {
    HOST.worlds.write().id_for(world)
}
