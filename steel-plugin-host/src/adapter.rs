//! Foreign entity and block-entity implementations plus factory trampolines.

use rustc_hash::FxHashMap;
use std::sync::{Arc, LazyLock, Weak};

use simdnbt::owned::NbtCompound;
use steel_abi::{SteelBlockEntityHooks, SteelEntityHooks, SteelEntityRef, SteelWorldRef};
use steel_core::block_entity::{BlockEntity, BlockEntityBase, SharedBlockEntity};
use steel_core::entity::{Entity, EntityBase, EntityBaseLoad, SharedEntity};
use steel_core::world::World;
use steel_registry::RegistryEntry;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::entity_type::EntityTypeRef;
use steel_utils::locks::{SyncMutex, SyncRwLock};
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::state::{HOST, OwnedObject, world_id};

struct HookCell<T>(T);

// SAFETY: hook tables are immutable and the extension contract guarantees
// thread-safe user data for the lifetime of the loaded library.
unsafe impl<T> Send for HookCell<T> {}

// SAFETY: see the `Send` implementation above.
unsafe impl<T> Sync for HookCell<T> {}

static BLOCK_ENTITY_HOOKS: LazyLock<SyncRwLock<FxHashMap<usize, HookCell<SteelBlockEntityHooks>>>> =
    LazyLock::new(|| SyncRwLock::new(FxHashMap::default()));
static ENTITY_HOOKS: LazyLock<SyncRwLock<FxHashMap<usize, HookCell<SteelEntityHooks>>>> =
    LazyLock::new(|| SyncRwLock::new(FxHashMap::default()));
static BLOCK_ENTITY_BY_BLOCK: LazyLock<SyncRwLock<FxHashMap<usize, usize>>> =
    LazyLock::new(|| SyncRwLock::new(FxHashMap::default()));

pub fn bind_block_entity_hooks(type_id: usize, hooks: SteelBlockEntityHooks) {
    BLOCK_ENTITY_HOOKS.write().insert(type_id, HookCell(hooks));
}

pub fn bind_entity_hooks(type_id: usize, hooks: SteelEntityHooks) {
    ENTITY_HOOKS.write().insert(type_id, HookCell(hooks));
}

pub fn bind_block_entity_type(block_id: usize, type_id: usize) {
    BLOCK_ENTITY_BY_BLOCK.write().insert(block_id, type_id);
}

fn block_entity_hooks(type_id: usize) -> Option<SteelBlockEntityHooks> {
    BLOCK_ENTITY_HOOKS.read().get(&type_id).map(|hooks| hooks.0)
}

fn entity_hooks(type_id: usize) -> Option<SteelEntityHooks> {
    ENTITY_HOOKS.read().get(&type_id).map(|hooks| hooks.0)
}

pub fn block_entity_type_for_block(block: BlockRef) -> Option<usize> {
    BLOCK_ENTITY_BY_BLOCK.read().get(&block.id()).copied()
}

/// Block entity that forwards lifecycle hooks to an extension.
pub struct ForeignBlockEntity {
    base: BlockEntityBase,
    hooks: SteelBlockEntityHooks,
    data: SyncMutex<NbtCompound>,
}

// SAFETY: this key uniquely identifies the Steel-owned foreign block-entity
// Rust type. All extension block entities share it by construction.
unsafe impl DowncastType for ForeignBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:plugin/block_entity");
}

impl ForeignBlockEntity {
    fn new(
        kind: BlockEntityTypeRef,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
        hooks: SteelBlockEntityHooks,
    ) -> Self {
        Self {
            base: BlockEntityBase::new(kind, level, pos, state),
            hooks,
            data: SyncMutex::new(NbtCompound::new()),
        }
    }

    fn world_ref(&self) -> Option<(Arc<World>, SteelWorldRef)> {
        let world = self.get_level()?;
        let id = world_id(&world);
        Some((world, SteelWorldRef(id)))
    }
}

impl BlockEntity for ForeignBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn tick(&self, _world: &Arc<World>) {
        let Some(hook) = self.hooks.tick else {
            return;
        };
        let Some((world, world_ref)) = self.world_ref() else {
            return;
        };
        let pos = self.get_block_pos();
        // SAFETY: the hook table belongs to a loaded extension.
        unsafe {
            hook(
                self.hooks.user_data,
                world_ref,
                pos.x(),
                pos.y(),
                pos.z(),
                steel_abi::SteelBlockState(u32::from(self.get_block_state().0)),
            );
        }
        let _ = world;
    }

    fn on_block_state_changed(&self, state: BlockStateId) {
        let Some(hook) = self.hooks.state_changed else {
            return;
        };
        let Some((_world, world_ref)) = self.world_ref() else {
            return;
        };
        let pos = self.get_block_pos();
        // SAFETY: see `tick`.
        unsafe {
            hook(
                self.hooks.user_data,
                world_ref,
                pos.x(),
                pos.y(),
                pos.z(),
                steel_abi::SteelBlockState(u32::from(state.0)),
            );
        }
    }

    fn load_additional(&self, nbt: &simdnbt::borrow::BaseNbtCompound<'_>) {
        if self.hooks.load.is_none() {
            let view: simdnbt::borrow::NbtCompound<'_, '_> = nbt.into();
            *self.data.lock() = view.to_owned();
            return;
        }
        let Some(hook) = self.hooks.load else {
            return;
        };
        let Some((_world, world_ref)) = self.world_ref() else {
            return;
        };
        let view: simdnbt::borrow::NbtCompound<'_, '_> = nbt.into();
        let handle = HOST.owned.lock().insert_nbt(view.to_owned());
        let pos = self.get_block_pos();
        // SAFETY: see `tick`. The handle remains owned by the extension.
        unsafe {
            hook(
                self.hooks.user_data,
                world_ref,
                pos.x(),
                pos.y(),
                pos.z(),
                handle,
            );
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        if self.hooks.save.is_none() {
            nbt.extend(
                self.data
                    .lock()
                    .iter()
                    .map(|(key, value)| (key.to_str().into_owned(), value.clone())),
            );
            return;
        }
        let Some(hook) = self.hooks.save else {
            return;
        };
        let Some((_world, world_ref)) = self.world_ref() else {
            return;
        };
        let handle = HOST.owned.lock().insert_nbt(NbtCompound::new());
        let pos = self.get_block_pos();
        // SAFETY: see `tick`.
        unsafe {
            hook(
                self.hooks.user_data,
                world_ref,
                pos.x(),
                pos.y(),
                pos.z(),
                handle,
            );
        }
        let mut owned = HOST.owned.lock();
        if let Some(OwnedObject::Nbt(produced)) = owned.get_mut(handle) {
            nbt.extend(
                produced
                    .iter()
                    .map(|(key, value)| (key.to_str().into_owned(), value.clone())),
            );
        }
        owned.remove(handle);
    }
}

/// Entity that forwards lifecycle hooks to an extension.
pub struct ForeignEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    hooks: SteelEntityHooks,
    data: SyncMutex<NbtCompound>,
}

// SAFETY: this key uniquely identifies the Steel-owned foreign entity Rust type.
unsafe impl DowncastType for ForeignEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:plugin/entity");
}

impl ForeignEntity {
    fn entity_ref(&self) -> SteelEntityRef {
        let world = self.level();
        SteelEntityRef {
            world: world.as_ref().map_or(SteelWorldRef::INVALID, |world| {
                SteelWorldRef(world_id(world))
            }),
            entity: self.id(),
        }
    }
}

impl Entity for ForeignEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        self.base_tick();
        if let Some(hook) = self.hooks.tick {
            // SAFETY: the hook table belongs to a loaded extension.
            unsafe { hook(self.hooks.user_data, self.entity_ref()) };
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        if self.hooks.save.is_none() {
            nbt.extend(
                self.data
                    .lock()
                    .iter()
                    .map(|(key, value)| (key.to_str().into_owned(), value.clone())),
            );
            return;
        }
        let Some(hook) = self.hooks.save else {
            return;
        };
        let handle = HOST.owned.lock().insert_nbt(NbtCompound::new());
        // SAFETY: see `tick`.
        unsafe { hook(self.hooks.user_data, self.entity_ref(), handle) };
        let mut owned = HOST.owned.lock();
        if let Some(OwnedObject::Nbt(produced)) = owned.get_mut(handle) {
            nbt.extend(
                produced
                    .iter()
                    .map(|(key, value)| (key.to_str().into_owned(), value.clone())),
            );
        }
        owned.remove(handle);
    }

    fn load_additional(&self, nbt: simdnbt::borrow::NbtCompound<'_, '_>) {
        if self.hooks.load.is_none() {
            *self.data.lock() = nbt.to_owned();
            return;
        }
        let Some(hook) = self.hooks.load else {
            return;
        };
        let handle = HOST.owned.lock().insert_nbt(nbt.to_owned());
        // SAFETY: see `tick`.
        unsafe { hook(self.hooks.user_data, self.entity_ref(), handle) };
    }
}

/// Factory trampoline registered for extension block-entity types.
#[must_use]
pub fn foreign_block_entity_factory(
    level: Weak<World>,
    pos: BlockPos,
    state: BlockStateId,
) -> SharedBlockEntity {
    let block = state.get_block();
    let type_id = block_entity_type_for_block(block).unwrap_or(usize::MAX);
    let Some(kind) = steel_registry::REGISTRY
        .block_entity_types
        .iter()
        .find_map(|(id, kind)| (id == type_id).then_some(kind))
    else {
        unreachable!("block-entity registration validated the block set")
    };
    let hooks = block_entity_hooks(kind.id()).unwrap_or_default();
    Arc::new(ForeignBlockEntity::new(kind, level, pos, state, hooks))
}

/// Factory trampoline registered for extension entity types.
#[must_use]
pub fn foreign_entity_factory(
    entity_type: EntityTypeRef,
    id: i32,
    position: glam::DVec3,
    world: Weak<World>,
) -> SharedEntity {
    let hooks = entity_hooks(entity_type.id()).unwrap_or_default();
    Arc::new(ForeignEntity {
        base: EntityBase::new(id, position, entity_type.dimensions, world),
        entity_type,
        hooks,
        data: SyncMutex::new(NbtCompound::new()),
    })
}

/// Load factory trampoline registered for extension entity types.
#[must_use]
pub fn foreign_entity_load_factory(
    entity_type: EntityTypeRef,
    load: EntityBaseLoad,
) -> SharedEntity {
    let hooks = entity_hooks(entity_type.id()).unwrap_or_default();
    Arc::new(ForeignEntity {
        base: EntityBase::from_load(load, entity_type.dimensions),
        entity_type,
        hooks,
        data: SyncMutex::new(NbtCompound::new()),
    })
}
