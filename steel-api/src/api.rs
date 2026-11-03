//! Annotated host API surface.
//!
//! These are the methods the generated bindings call through MethodBinds. The
//! implementations resolve handles through the shared host state; they do not
//! expose Rust borrows or lifetimes.

use std::sync::Arc;

use steel_abi::*;
use steel_api_macros::{steel_api, steel_method};
use steel_core::entity::Entity as _;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::item_stack::ItemStack;
use steel_registry::{RegistryEntry, RegistryExt};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId as UtilsBlockStateId, Identifier};
use text_components::TextComponent;

use crate::from_ref::{BlockId, BlockStateId, ItemId};
use crate::host::{HOST, scratch_view};

#[steel_api(name = "Object", handle = "none", receiver = "()")]
pub struct ObjectApi;

impl ObjectApi {}

fn world(reference: SteelWorldRef) -> Option<Arc<steel_core::world::World>> {
    HOST.worlds.read().get(reference.0)
}

fn entity(reference: SteelEntityRef) -> Option<steel_core::entity::SharedEntity> {
    let world = world(reference.world)?;
    world.get_entity_by_id(reference.entity)
}

fn identifier(view: SteelStrView) -> Option<Identifier> {
    // SAFETY: the host supplies a borrowed view for the call.
    let value = unsafe { view.as_str() }?;
    let (namespace, path) = value.split_once(':')?;
    Some(Identifier::new(namespace.to_owned(), path.to_owned()))
}

fn state_id(id: u32) -> Option<UtilsBlockStateId> {
    let state = UtilsBlockStateId(u16::try_from(id).ok()?);
    steel_registry::REGISTRY.blocks.by_state_id(state)?;
    Some(state)
}

#[steel_api(
    name = "World",
    inherits = "Object",
    handle = "world",
    receiver = "SteelWorldRef"
)]
pub struct WorldApi;

impl WorldApi {
    #[steel_method]
    #[must_use]
    pub fn get_block(world_id: SteelWorldRef, x: i32, y: i32, z: i32) -> u32 {
        world(world_id).map_or(0, |world| {
            u32::from(world.get_block_state(BlockPos::new(x, y, z)).0)
        })
    }

    #[steel_method]
    #[must_use]
    pub fn set_block(
        world_id: SteelWorldRef,
        x: i32,
        y: i32,
        z: i32,
        state: u32,
        flags: u32,
    ) -> bool {
        let Some(world) = world(world_id) else {
            return false;
        };
        let Some(state) = state_id(state) else {
            return false;
        };
        world.set_block(
            BlockPos::new(x, y, z),
            state,
            UpdateFlags::from_bits_truncate(u16::try_from(flags).unwrap_or(0)),
        )
    }

    #[steel_method]
    #[must_use]
    pub fn destroy_block(
        world_id: SteelWorldRef,
        x: i32,
        y: i32,
        z: i32,
        drop_items: bool,
    ) -> bool {
        world(world_id).is_some_and(|world| world.destroy_block(BlockPos::new(x, y, z), drop_items))
    }

    #[steel_method]
    #[must_use]
    pub fn spawn(world_id: SteelWorldRef) -> SteelVec3 {
        let Some(world) = world(world_id) else {
            return SteelVec3::default();
        };
        let spawn = world.level_data.read().data().spawn_pos();
        SteelVec3 {
            x: f64::from(spawn.x()) + 0.5,
            y: f64::from(spawn.y()),
            z: f64::from(spawn.z()) + 0.5,
        }
    }

    #[steel_method]
    #[must_use]
    pub fn player_count(world_id: SteelWorldRef) -> u32 {
        world(world_id).map_or(0, |world| u32::try_from(world.players.len()).unwrap_or(0))
    }

    #[steel_method(returns_class = "Player")]
    #[must_use]
    pub fn player_at(world_id: SteelWorldRef, index: u32) -> SteelRef {
        let Some(world) = world(world_id) else {
            return SteelRef::NONE;
        };
        let mut players = Vec::new();
        world.players.iter_players(|_uuid, player| {
            players.push(Arc::clone(player));
            true
        });
        let Some(player) = players.get(index as usize) else {
            return SteelRef::NONE;
        };
        SteelRef::owned(
            STEEL_REF_ENTITY,
            crate::host::world_id(&world),
            u32::try_from(player.id()).unwrap_or(0),
        )
    }

    #[steel_method]
    pub fn play_sound(
        world_id: SteelWorldRef,
        sound: SteelStrView,
        x: f64,
        y: f64,
        z: f64,
        volume: f32,
        pitch: f32,
    ) {
        let Some(world) = world(world_id) else {
            return;
        };
        let Some(key) = identifier(sound) else {
            return;
        };
        let Some(event) = steel_registry::REGISTRY.sound_events.by_key(&key) else {
            return;
        };
        world.play_sound_at(
            event,
            SoundSource::Master,
            glam::DVec3::new(x, y, z),
            volume,
            pitch,
            None,
        );
    }
}

#[steel_api(
    name = "Entity",
    inherits = "Object",
    handle = "entity",
    receiver = "SteelEntityRef"
)]
pub struct EntityApi;

impl EntityApi {
    #[steel_method]
    #[must_use]
    pub fn position(reference: SteelEntityRef) -> SteelVec3 {
        entity(reference).map_or_else(SteelVec3::default, |entity| {
            let position = entity.position();
            SteelVec3 {
                x: position.x,
                y: position.y,
                z: position.z,
            }
        })
    }

    #[steel_method]
    #[must_use]
    pub fn teleport(reference: SteelEntityRef, position: SteelVec3, yaw: f32, pitch: f32) -> bool {
        let Some(entity) = entity(reference) else {
            return false;
        };
        entity
            .teleport_to(glam::DVec3::new(position.x, position.y, position.z))
            .is_ok()
            && {
                entity.set_rotation((yaw, pitch));
                true
            }
    }

    #[steel_method]
    #[must_use]
    pub fn health(reference: SteelEntityRef) -> f32 {
        entity(reference)
            .and_then(|entity| entity.as_living_entity().map(|living| living.get_health()))
            .unwrap_or(0.0)
    }

    #[steel_method]
    #[must_use]
    pub fn set_health(reference: SteelEntityRef, health: f32) -> bool {
        let Some(entity) = entity(reference) else {
            return false;
        };
        let Some(living) = entity.as_living_entity() else {
            return false;
        };
        living.set_health(health);
        true
    }
}

#[steel_api(
    name = "Player",
    inherits = "Entity",
    handle = "entity",
    receiver = "SteelEntityRef"
)]
pub struct PlayerApi;

impl PlayerApi {
    #[steel_method]
    #[must_use]
    pub fn name(reference: SteelEntityRef) -> SteelStrView {
        entity(reference)
            .and_then(|entity| {
                entity
                    .as_player()
                    .map(|player| player.gameprofile.name().to_owned())
            })
            .map_or(SteelStrView::EMPTY, |name| scratch_view(&name))
    }

    #[steel_method]
    #[must_use]
    pub fn uuid(reference: SteelEntityRef) -> SteelStrView {
        entity(reference)
            .and_then(|entity| {
                entity
                    .as_player()
                    .map(|player| player.gameprofile.id().to_string())
            })
            .map_or(SteelStrView::EMPTY, |uuid| scratch_view(&uuid))
    }

    #[steel_method]
    #[must_use]
    pub fn send_message(reference: SteelEntityRef, message: SteelStrView) -> bool {
        let Some(entity) = entity(reference) else {
            return false;
        };
        let Some(player) = entity.as_player() else {
            return false;
        };
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(message) = (unsafe { message.as_str() }) else {
            return false;
        };
        player.send_message(&TextComponent::plain(message));
        true
    }
}

#[steel_api(
    name = "Server",
    inherits = "Object",
    handle = "server",
    receiver = "()"
)]
pub struct ServerApi;

impl ServerApi {
    #[steel_method(static, returns_class = "Player")]
    #[must_use]
    pub fn find_player(world_id: u32, name: SteelStrView) -> SteelRef {
        let Some(world) = HOST.worlds.read().get(world_id) else {
            return SteelRef::NONE;
        };
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(name) = (unsafe { name.as_str() }) else {
            return SteelRef::NONE;
        };
        let mut found = None;
        world.players.iter_players(|_uuid, player| {
            if player.gameprofile.name() == name {
                found = Some(player.id());
                false
            } else {
                true
            }
        });
        found.map_or(SteelRef::NONE, |entity| {
            SteelRef::owned(
                STEEL_REF_ENTITY,
                world_id,
                u32::try_from(entity).unwrap_or(0),
            )
        })
    }
}

#[steel_api(
    name = "Block",
    inherits = "Object",
    handle = "block",
    receiver = "BlockId"
)]
pub struct BlockApi;

impl BlockApi {
    #[steel_method(static, returns_class = "Block")]
    #[must_use]
    pub fn by_key(key: SteelStrView) -> SteelRef {
        let Some(key) = identifier(key) else {
            return SteelRef::NONE;
        };
        steel_registry::REGISTRY
            .blocks
            .id_from_key(&key)
            .map_or(SteelRef::NONE, |id| {
                SteelRef::registry(STEEL_REF_BLOCK, u32::try_from(id).unwrap_or(0))
            })
    }

    #[steel_method]
    #[must_use]
    pub fn default_state(block: BlockId) -> u32 {
        steel_registry::REGISTRY
            .blocks
            .by_id(block.0 as usize)
            .map_or(0, |block| u32::from(block.default_state().0))
    }
}

#[steel_api(
    name = "BlockState",
    inherits = "Object",
    handle = "block_state",
    receiver = "BlockStateId"
)]
pub struct BlockStateApi;

impl BlockStateApi {
    #[steel_method]
    #[must_use]
    pub fn with_bool(state: BlockStateId, name: SteelStrView, value: bool) -> u32 {
        set_property(state, name, if value { "true" } else { "false" })
    }

    #[steel_method]
    #[must_use]
    pub fn with_int(state: BlockStateId, name: SteelStrView, value: i32) -> u32 {
        set_property(state, name, &value.to_string())
    }

    #[steel_method]
    #[must_use]
    pub fn with_enum(state: BlockStateId, name: SteelStrView, value: SteelStrView) -> u32 {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(value) = (unsafe { value.as_str() }) else {
            return state.0;
        };
        set_property(state, name, value)
    }

    #[steel_method]
    #[must_use]
    pub fn get_bool(state: BlockStateId, name: SteelStrView) -> bool {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(name) = (unsafe { name.as_str() }) else {
            return false;
        };
        steel_registry::REGISTRY
            .blocks
            .get_property_str(UtilsBlockStateId(u16::try_from(state.0).unwrap_or(0)), name)
            .is_some_and(|value| value == "true")
    }

    #[steel_method]
    #[must_use]
    pub fn get_int(state: BlockStateId, name: SteelStrView) -> i32 {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(name) = (unsafe { name.as_str() }) else {
            return 0;
        };
        steel_registry::REGISTRY
            .blocks
            .get_property_str(UtilsBlockStateId(u16::try_from(state.0).unwrap_or(0)), name)
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    }

    #[steel_method]
    #[must_use]
    pub fn get_enum(state: BlockStateId, name: SteelStrView) -> SteelStrView {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(name) = (unsafe { name.as_str() }) else {
            return SteelStrView::EMPTY;
        };
        steel_registry::REGISTRY
            .blocks
            .get_property_str(UtilsBlockStateId(u16::try_from(state.0).unwrap_or(0)), name)
            .map_or(SteelStrView::EMPTY, SteelStrView::from_str)
    }

    #[steel_method(returns_class = "Block")]
    #[must_use]
    pub fn block(state: BlockStateId) -> SteelRef {
        steel_registry::REGISTRY
            .blocks
            .by_state_id(UtilsBlockStateId(u16::try_from(state.0).unwrap_or(0)))
            .map_or(SteelRef::NONE, |block| {
                SteelRef::registry(STEEL_REF_BLOCK, u32::try_from(block.id()).unwrap_or(0))
            })
    }
}

fn set_property(state: BlockStateId, name: SteelStrView, value: &str) -> u32 {
    let original = state.0;
    // SAFETY: the host supplies a borrowed view for the call.
    let Some(name) = (unsafe { name.as_str() }) else {
        return original;
    };
    let Some(state) = state_id(original) else {
        return original;
    };
    steel_registry::REGISTRY
        .blocks
        .try_set_property_by_name(state, name, value)
        .map_or(original, |state| u32::from(state.0))
}

#[steel_api(
    name = "Item",
    inherits = "Object",
    handle = "item",
    receiver = "ItemId"
)]
pub struct ItemApi;

impl ItemApi {
    #[steel_method(static, returns_class = "Item")]
    #[must_use]
    pub fn by_key(key: SteelStrView) -> SteelRef {
        let Some(key) = identifier(key) else {
            return SteelRef::NONE;
        };
        steel_registry::REGISTRY
            .items
            .id_from_key(&key)
            .map_or(SteelRef::NONE, |id| {
                SteelRef::registry(STEEL_REF_ITEM, u32::try_from(id).unwrap_or(0))
            })
    }
}

#[steel_api(
    name = "ItemStack",
    inherits = "Object",
    handle = "item_stack",
    receiver = "SteelRef"
)]
pub struct ItemStackApi;

impl ItemStackApi {
    #[steel_method(static, returns_class = "ItemStack")]
    #[must_use]
    pub fn create(item: SteelRef, count: i32) -> SteelRef {
        if item.kind != STEEL_REF_ITEM {
            return SteelRef::NONE;
        }
        let Some(item) = steel_registry::REGISTRY.items.by_id(item.index as usize) else {
            return SteelRef::NONE;
        };
        let mut stack = ItemStack::new(item);
        stack.count = count;
        HOST.owned.lock().insert_item_stack(stack)
    }

    #[steel_method]
    #[must_use]
    pub fn count(stack: SteelRef) -> i32 {
        match HOST.owned.lock().get(stack) {
            Some(crate::host::OwnedObject::ItemStack(stack)) => stack.count,
            _ => 0,
        }
    }

    #[steel_method]
    #[must_use]
    pub fn set_count(stack: SteelRef, count: i32) -> bool {
        match HOST.owned.lock().get_mut(stack) {
            Some(crate::host::OwnedObject::ItemStack(stack)) => {
                stack.count = count;
                true
            }
            _ => false,
        }
    }

    #[steel_method(returns_class = "Item")]
    #[must_use]
    pub fn item(stack: SteelRef) -> SteelRef {
        match HOST.owned.lock().get(stack) {
            Some(crate::host::OwnedObject::ItemStack(stack)) => SteelRef::registry(
                STEEL_REF_ITEM,
                u32::try_from(stack.item().id()).unwrap_or(0),
            ),
            _ => SteelRef::NONE,
        }
    }

    #[steel_method(returns_class = "ItemStack")]
    #[must_use]
    pub fn clone_stack(stack: SteelRef) -> SteelRef {
        HOST.owned.lock().clone_ref(stack).unwrap_or(SteelRef::NONE)
    }

    #[steel_method]
    pub fn free(stack: SteelRef) {
        HOST.owned.lock().remove(stack);
    }
}

#[steel_api(
    name = "Nbt",
    inherits = "Object",
    handle = "nbt",
    receiver = "SteelRef"
)]
pub struct NbtApi;

impl NbtApi {
    #[steel_method(static, returns_class = "Nbt")]
    #[must_use]
    pub fn create() -> SteelRef {
        HOST.owned
            .lock()
            .insert_nbt(simdnbt::owned::NbtCompound::new())
    }

    #[steel_method]
    #[must_use]
    pub fn set_int(nbt: SteelRef, key: SteelStrView, value: i64) -> bool {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(key) = (unsafe { key.as_str() }) else {
            return false;
        };
        match HOST.owned.lock().get_mut(nbt) {
            Some(crate::host::OwnedObject::Nbt(compound)) => {
                compound.insert(key, value);
                true
            }
            _ => false,
        }
    }

    #[steel_method]
    #[must_use]
    pub fn get_int(nbt: SteelRef, key: SteelStrView) -> i64 {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(key) = (unsafe { key.as_str() }) else {
            return 0;
        };
        match HOST.owned.lock().get(nbt) {
            Some(crate::host::OwnedObject::Nbt(compound)) => compound.long(key).unwrap_or(0),
            _ => 0,
        }
    }

    #[steel_method]
    #[must_use]
    pub fn set_string(nbt: SteelRef, key: SteelStrView, value: SteelStrView) -> bool {
        // SAFETY: the host supplies borrowed views for the call.
        let (Some(key), Some(value)) = (unsafe { key.as_str() }, unsafe { value.as_str() }) else {
            return false;
        };
        match HOST.owned.lock().get_mut(nbt) {
            Some(crate::host::OwnedObject::Nbt(compound)) => {
                compound.insert(key, value);
                true
            }
            _ => false,
        }
    }

    #[steel_method]
    #[must_use]
    pub fn get_string(nbt: SteelRef, key: SteelStrView) -> SteelStrView {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(key) = (unsafe { key.as_str() }) else {
            return SteelStrView::EMPTY;
        };
        match HOST.owned.lock().get(nbt) {
            Some(crate::host::OwnedObject::Nbt(compound)) => {
                compound.string(key).map_or(SteelStrView::EMPTY, |value| {
                    scratch_view(&value.to_string())
                })
            }
            _ => SteelStrView::EMPTY,
        }
    }

    #[steel_method]
    #[must_use]
    pub fn contains(nbt: SteelRef, key: SteelStrView) -> bool {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(key) = (unsafe { key.as_str() }) else {
            return false;
        };
        match HOST.owned.lock().get(nbt) {
            Some(crate::host::OwnedObject::Nbt(compound)) => compound.contains(key),
            _ => false,
        }
    }

    #[steel_method]
    #[must_use]
    pub fn remove(nbt: SteelRef, key: SteelStrView) -> bool {
        // SAFETY: the host supplies a borrowed view for the call.
        let Some(key) = (unsafe { key.as_str() }) else {
            return false;
        };
        match HOST.owned.lock().get_mut(nbt) {
            Some(crate::host::OwnedObject::Nbt(compound)) => compound.remove(key).is_some(),
            _ => false,
        }
    }

    #[steel_method(returns_class = "Nbt")]
    #[must_use]
    pub fn clone_nbt(nbt: SteelRef) -> SteelRef {
        HOST.owned.lock().clone_ref(nbt).unwrap_or(SteelRef::NONE)
    }

    #[steel_method]
    pub fn free(nbt: SteelRef) {
        HOST.owned.lock().remove(nbt);
    }
}
