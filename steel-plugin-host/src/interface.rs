//! Implementation of the host function table exported to extensions.

#![allow(
    clippy::wildcard_imports,
    reason = "the interface wires the complete ABI surface"
)]

use std::cell::Cell;
use std::ffi::c_void;
use std::slice;
use std::sync::Arc;

use simdnbt::owned::NbtCompound;
use steel_abi::*;
use steel_core::command::{
    CommandArgument, CommandContext, CommandError, CommandRegistration, argument, literal,
    register_plugin_command,
};
use steel_core::entity::{ENTITIES, Entity, next_entity_id};
use steel_core::events::{self, Event, EventKind, EventOutcome};
use steel_core::permission::{PermissionExpr, PermissionKey};
use steel_core::world::World;
use steel_protocol::packets::common::CCustomPayload;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::BlockRef;
use steel_registry::{Registry, RegistryEntry, RegistryExt};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Identifier};
use text_components::TextComponent;

use crate::convert;
use crate::state::{HOST, OwnedObject, last_error_view, scratch_view, set_last_error, world_id};

thread_local! {
    static CURRENT_REGISTRY: Cell<*mut Registry> = const { Cell::new(std::ptr::null_mut()) };
}

/// Runs `f` with the registry that is currently accepting extension entries.
pub fn with_registry(registry: &mut Registry, f: impl FnOnce()) {
    CURRENT_REGISTRY.with(|slot| slot.set(registry));
    f();
    CURRENT_REGISTRY.with(|slot| slot.set(std::ptr::null_mut()));
}

fn current_registry() -> Option<&'static mut Registry> {
    CURRENT_REGISTRY.with(|slot| {
        let pointer = slot.get();
        // SAFETY: `with_registry` keeps the pointer valid for the duration of
        // the content phase and clears it afterwards.
        (!pointer.is_null()).then(|| unsafe { &mut *pointer })
    })
}

fn block_registry<R>(f: impl FnOnce(&steel_registry::blocks::BlockRegistry) -> R) -> Option<R> {
    if let Some(registry) = current_registry() {
        return Some(f(&registry.blocks));
    }
    steel_registry::REGISTRY
        .try_get()
        .map(|registry| f(&registry.blocks))
}

fn item_registry<R>(f: impl FnOnce(&steel_registry::items::ItemRegistry) -> R) -> Option<R> {
    if let Some(registry) = current_registry() {
        return Some(f(&registry.items));
    }
    steel_registry::REGISTRY
        .try_get()
        .map(|registry| f(&registry.items))
}

fn block(reference: SteelRef) -> Option<BlockRef> {
    if reference.kind != STEEL_REF_BLOCK {
        return None;
    }
    block_registry(|blocks| blocks.by_id(reference.index as usize)).flatten()
}

fn item(reference: SteelRef) -> Option<steel_registry::items::ItemRef> {
    if reference.kind != STEEL_REF_ITEM {
        return None;
    }
    item_registry(|items| items.by_id(reference.index as usize)).flatten()
}

fn state(value: SteelBlockState) -> Option<BlockStateId> {
    let id = u16::try_from(value.0).ok()?;
    block_registry(|blocks| blocks.by_state_id(BlockStateId(id)))
        .flatten()
        .map(|_| BlockStateId(id))
}

fn world(reference: SteelWorldRef) -> Option<Arc<World>> {
    HOST.worlds.read().get(reference.0)
}

unsafe extern "C" fn host_log(level: u32, message: SteelStrView) {
    // SAFETY: the extension supplies a valid view for this call.
    let message = unsafe { message.as_str() }.unwrap_or("<invalid utf-8>");
    match level {
        0 => log::error!("{message}"),
        1 => log::warn!("{message}"),
        2 => log::info!("{message}"),
        3 => log::debug!("{message}"),
        _ => log::trace!("{message}"),
    }
}

unsafe extern "C" fn host_last_error() -> SteelStrView {
    last_error_view()
}

unsafe extern "C" fn host_block_by_key(key: SteelStrView) -> SteelRef {
    let Ok(key) = convert::identifier(key) else {
        set_last_error("invalid block identifier");
        return SteelRef::NONE;
    };
    let id = if let Some(registry) = current_registry() {
        registry.blocks.id_from_key(&key)
    } else {
        steel_registry::REGISTRY.blocks.id_from_key(&key)
    };
    id.map_or(SteelRef::NONE, |id| {
        SteelRef::registry(STEEL_REF_BLOCK, id as u32)
    })
}

unsafe extern "C" fn host_item_by_key(key: SteelStrView) -> SteelRef {
    let Ok(key) = convert::identifier(key) else {
        set_last_error("invalid item identifier");
        return SteelRef::NONE;
    };
    let id = if let Some(registry) = current_registry() {
        registry.items.id_from_key(&key)
    } else {
        steel_registry::REGISTRY.items.id_from_key(&key)
    };
    id.map_or(SteelRef::NONE, |id| {
        SteelRef::registry(STEEL_REF_ITEM, id as u32)
    })
}

unsafe extern "C" fn host_block_default_state(reference: SteelRef) -> SteelBlockState {
    block(reference).map_or(SteelBlockState(0), |block| {
        SteelBlockState(u32::from(block.default_state().0))
    })
}

unsafe extern "C" fn host_state_block(value: SteelBlockState) -> SteelRef {
    let Some(state) = state(value) else {
        return SteelRef::NONE;
    };
    block_registry(|blocks| blocks.by_state_id(state))
        .flatten()
        .map_or(SteelRef::NONE, |block| {
            SteelRef::registry(STEEL_REF_BLOCK, block.id() as u32)
        })
}

unsafe extern "C" fn host_state_with_bool(
    value: SteelBlockState,
    name: SteelStrView,
    bool_value: u8,
) -> SteelBlockState {
    state_with(value, name, if bool_value != 0 { "true" } else { "false" })
}

unsafe extern "C" fn host_state_with_int(
    value: SteelBlockState,
    name: SteelStrView,
    int_value: i32,
) -> SteelBlockState {
    state_with(value, name, &int_value.to_string())
}

unsafe extern "C" fn host_state_with_enum(
    value: SteelBlockState,
    name: SteelStrView,
    enum_value: SteelStrView,
) -> SteelBlockState {
    // SAFETY: the extension supplies the view for this call.
    let Some(enum_value) = (unsafe { enum_value.as_str() }) else {
        return value;
    };
    state_with(value, name, enum_value)
}

fn state_with(value: SteelBlockState, name: SteelStrView, property_value: &str) -> SteelBlockState {
    let Some(state) = state(value) else {
        return value;
    };
    // SAFETY: the extension supplies the view for this call.
    let Some(name) = (unsafe { name.as_str() }) else {
        return value;
    };
    block_registry(|blocks| blocks.try_set_property_by_name(state, name, property_value))
        .flatten()
        .map_or(value, |state| SteelBlockState(u32::from(state.0)))
}

unsafe extern "C" fn host_state_get_bool(
    value: SteelBlockState,
    name: SteelStrView,
    out: *mut u8,
) -> i32 {
    let Some(state) = state(value) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    // SAFETY: the extension supplies the view for this call.
    let Some(name) = (unsafe { name.as_str() }) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(Some(value)) = block_registry(|blocks| blocks.get_property_str(state, name)) else {
        return STEEL_ERR_NOT_FOUND;
    };
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe { *out = u8::from(value == "true") };
    STEEL_OK
}

unsafe extern "C" fn host_state_get_int(
    value: SteelBlockState,
    name: SteelStrView,
    out: *mut i32,
) -> i32 {
    let Some(state) = state(value) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    // SAFETY: the extension supplies the view for this call.
    let Some(name) = (unsafe { name.as_str() }) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(Some(value)) = block_registry(|blocks| blocks.get_property_str(state, name)) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Ok(value) = value.parse::<i32>() else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe { *out = value };
    STEEL_OK
}

unsafe extern "C" fn host_state_get_enum(
    value: SteelBlockState,
    name: SteelStrView,
    out: *mut SteelStrView,
) -> i32 {
    let Some(state) = state(value) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    // SAFETY: the extension supplies the view for this call.
    let Some(name) = (unsafe { name.as_str() }) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(Some(value)) = block_registry(|blocks| blocks.get_property_str(state, name)) else {
        return STEEL_ERR_NOT_FOUND;
    };
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the extension supplied a writable out-pointer. The view points at
    // the registry's leaked property name table.
    unsafe { *out = SteelStrView::from_str(value) };
    STEEL_OK
}

unsafe extern "C" fn host_world_get_block(
    world_ref: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
) -> SteelBlockState {
    world(world_ref).map_or(SteelBlockState(0), |world| {
        SteelBlockState(u32::from(world.get_block_state(BlockPos::new(x, y, z)).0))
    })
}

unsafe extern "C" fn host_world_set_block(
    world_ref: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    value: SteelBlockState,
    flags: u32,
) -> u8 {
    let (Some(world), Some(state)) = (world(world_ref), state(value)) else {
        return 0;
    };
    u8::from(world.set_block(
        BlockPos::new(x, y, z),
        state,
        UpdateFlags::from_bits_truncate(u16::try_from(flags).unwrap_or(0)),
    ))
}

unsafe extern "C" fn host_world_destroy_block(
    world_ref: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
    drop_items: u8,
) -> u8 {
    world(world_ref).map_or(0, |world| {
        u8::from(world.destroy_block(BlockPos::new(x, y, z), drop_items != 0))
    })
}

unsafe extern "C" fn host_world_get_entity(
    world_ref: SteelWorldRef,
    entity_id: i32,
) -> SteelOptEntityRef {
    let Some(world) = world(world_ref) else {
        return SteelOptEntityRef::default();
    };
    world
        .get_entity_by_id(entity_id)
        .map_or_else(SteelOptEntityRef::default, |_| SteelOptEntityRef {
            present: 1,
            value: SteelEntityRef {
                world: world_ref,
                entity: entity_id,
            },
        })
}

unsafe extern "C" fn host_world_get_block_entity(
    world_ref: SteelWorldRef,
    x: i32,
    y: i32,
    z: i32,
) -> SteelOptBlockEntityRef {
    let Some(world) = world(world_ref) else {
        return SteelOptBlockEntityRef::default();
    };
    world.get_block_entity(BlockPos::new(x, y, z)).map_or_else(
        SteelOptBlockEntityRef::default,
        |_| SteelOptBlockEntityRef {
            present: 1,
            value: SteelBlockEntityRef {
                world: world_ref,
                x,
                y,
                z,
            },
        },
    )
}

unsafe extern "C" fn host_world_spawn_entity(
    world_ref: SteelWorldRef,
    entity_type: SteelRef,
    position: SteelVec3,
    yaw: f32,
    pitch: f32,
    out: *mut SteelOptEntityRef,
) -> i32 {
    let Some(world) = world(world_ref) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    if entity_type.kind != STEEL_REF_ENTITY_TYPE {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let Some(entity_type) = steel_registry::REGISTRY
        .entity_types
        .iter()
        .find_map(|(id, kind)| (id == entity_type.index as usize).then_some(kind))
    else {
        return STEEL_ERR_NOT_FOUND;
    };
    let entity_id = next_entity_id();
    let Some(entity) = ENTITIES.create(
        entity_type,
        entity_id,
        glam::DVec3::new(position.x, position.y, position.z),
        Arc::downgrade(&world),
    ) else {
        return STEEL_ERR_UNSUPPORTED;
    };
    entity.set_rotation((yaw, pitch));
    if world.try_add_entity(entity).is_err() {
        return STEEL_ERR_INTERNAL;
    }
    if !out.is_null() {
        // SAFETY: the extension supplied a writable out-pointer.
        unsafe {
            *out = SteelOptEntityRef {
                present: 1,
                value: SteelEntityRef {
                    world: world_ref,
                    entity: entity_id,
                },
            }
        };
    }
    STEEL_OK
}

unsafe extern "C" fn host_world_play_sound(
    world_ref: SteelWorldRef,
    sound_key: SteelStrView,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
) {
    let Some(world) = world(world_ref) else {
        return;
    };
    let Ok(key) = convert::identifier(sound_key) else {
        return;
    };
    let Some(sound) = steel_registry::REGISTRY.sound_events.by_key(&key) else {
        set_last_error(format!("unknown sound event `{key}`"));
        return;
    };
    world.play_sound_at(
        sound,
        SoundSource::Master,
        glam::DVec3::new(x, y, z),
        volume,
        pitch,
        None,
    );
}

unsafe extern "C" fn host_world_broadcast_payload(
    world_ref: SteelWorldRef,
    channel: SteelStrView,
    data: *const u8,
    len: usize,
) -> i32 {
    let Some(world) = world(world_ref) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Ok(channel) = convert::identifier(channel) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let payload = if len == 0 {
        Vec::new()
    } else if data.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    } else {
        // SAFETY: the extension supplied a readable buffer for this call.
        unsafe { slice::from_raw_parts(data, len) }.to_vec()
    };
    world.broadcast_to_all(CCustomPayload::new(channel, payload.into_boxed_slice()));
    STEEL_OK
}

unsafe extern "C" fn host_player_send_payload(
    player: SteelEntityRef,
    channel: SteelStrView,
    data: *const u8,
    len: usize,
) -> i32 {
    let Some(world) = world(player.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(entity) = world.get_entity_by_id(player.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Some(player) = entity.as_player() else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Ok(channel) = convert::identifier(channel) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let payload = if len == 0 {
        Vec::new()
    } else if data.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    } else {
        // SAFETY: the extension supplied a readable buffer for this call.
        unsafe { slice::from_raw_parts(data, len) }.to_vec()
    };
    player.send_packet(CCustomPayload::new(channel, payload.into_boxed_slice()));
    STEEL_OK
}

unsafe extern "C" fn host_entity_get_position(entity: SteelEntityRef, out: *mut SteelVec3) -> i32 {
    let Some(world) = world(entity.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(entity) = world.get_entity_by_id(entity.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let position = entity.position();
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe {
        *out = SteelVec3 {
            x: position.x,
            y: position.y,
            z: position.z,
        }
    };
    STEEL_OK
}

unsafe extern "C" fn host_entity_teleport(
    entity: SteelEntityRef,
    position: SteelVec3,
    yaw: f32,
    pitch: f32,
) -> i32 {
    let Some(world) = world(entity.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(entity) = world.get_entity_by_id(entity.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let _ = entity.teleport_to(glam::DVec3::new(position.x, position.y, position.z));
    entity.set_rotation((yaw, pitch));
    STEEL_OK
}

unsafe extern "C" fn host_item_stack_new(item_ref: SteelRef, count: i32) -> SteelRef {
    let Some(item) = item(item_ref) else {
        set_last_error("item_stack_new: invalid item reference");
        return SteelRef::NONE;
    };
    let stack = steel_registry::item_stack::ItemStack::new(item);
    let mut stack = stack;
    stack.count = count;
    HOST.owned.lock().insert_item_stack(stack)
}

unsafe extern "C" fn host_item_stack_clone(stack: SteelRef) -> SteelRef {
    HOST.owned.lock().clone_ref(stack).unwrap_or(SteelRef::NONE)
}

unsafe extern "C" fn host_item_stack_free(stack: SteelRef) {
    HOST.owned.lock().remove(stack);
}

unsafe extern "C" fn host_item_stack_count(stack: SteelRef) -> i32 {
    match HOST.owned.lock().get(stack) {
        Some(OwnedObject::ItemStack(stack)) => stack.count,
        _ => 0,
    }
}

unsafe extern "C" fn host_item_stack_set_count(stack: SteelRef, count: i32) -> i32 {
    match HOST.owned.lock().get_mut(stack) {
        Some(OwnedObject::ItemStack(stack)) => {
            stack.count = count;
            STEEL_OK
        }
        _ => STEEL_ERR_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn host_item_stack_item(stack: SteelRef) -> SteelRef {
    match HOST.owned.lock().get(stack) {
        Some(OwnedObject::ItemStack(stack)) => {
            SteelRef::registry(STEEL_REF_ITEM, stack.item().id() as u32)
        }
        _ => SteelRef::NONE,
    }
}

unsafe extern "C" fn host_nbt_new() -> SteelRef {
    HOST.owned.lock().insert_nbt(NbtCompound::new())
}

unsafe extern "C" fn host_nbt_clone(nbt: SteelRef) -> SteelRef {
    HOST.owned.lock().clone_ref(nbt).unwrap_or(SteelRef::NONE)
}

unsafe extern "C" fn host_nbt_free(nbt: SteelRef) {
    HOST.owned.lock().remove(nbt);
}

unsafe extern "C" fn host_nbt_set_int(nbt: SteelRef, key: SteelStrView, value: i64) -> i32 {
    let Ok(key) = convert::string(key) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    match HOST.owned.lock().get_mut(nbt) {
        Some(OwnedObject::Nbt(compound)) => {
            compound.insert(key, value);
            STEEL_OK
        }
        _ => STEEL_ERR_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn host_nbt_get_int(nbt: SteelRef, key: SteelStrView, default_value: i64) -> i64 {
    let Ok(key) = convert::string(key) else {
        return default_value;
    };
    match HOST.owned.lock().get(nbt) {
        Some(OwnedObject::Nbt(compound)) => compound.long(&key).unwrap_or(default_value),
        _ => default_value,
    }
}

unsafe extern "C" fn host_nbt_set_string(
    nbt: SteelRef,
    key: SteelStrView,
    value: SteelStrView,
) -> i32 {
    let Ok(key) = convert::string(key) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Ok(value) = convert::string(value) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    match HOST.owned.lock().get_mut(nbt) {
        Some(OwnedObject::Nbt(compound)) => {
            compound.insert(key, value);
            STEEL_OK
        }
        _ => STEEL_ERR_INVALID_ARGUMENT,
    }
}

unsafe extern "C" fn host_nbt_get_string(nbt: SteelRef, key: SteelStrView) -> SteelStrView {
    let Ok(key) = convert::string(key) else {
        return SteelStrView::EMPTY;
    };
    let value = match HOST.owned.lock().get(nbt) {
        Some(OwnedObject::Nbt(compound)) => compound.string(&key).map(|value| value.to_string()),
        _ => None,
    };
    value.map_or(SteelStrView::EMPTY, |value| scratch_view(&value))
}

unsafe extern "C" fn host_nbt_contains(nbt: SteelRef, key: SteelStrView) -> u8 {
    let Ok(key) = convert::string(key) else {
        return 0;
    };
    match HOST.owned.lock().get(nbt) {
        Some(OwnedObject::Nbt(compound)) => u8::from(compound.contains(&key)),
        _ => 0,
    }
}

unsafe extern "C" fn host_nbt_remove(nbt: SteelRef, key: SteelStrView) -> i32 {
    let Ok(key) = convert::string(key) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    match HOST.owned.lock().get_mut(nbt) {
        Some(OwnedObject::Nbt(compound)) => {
            let removed = compound.remove(&key).is_some();
            if removed {
                STEEL_OK
            } else {
                STEEL_ERR_NOT_FOUND
            }
        }
        _ => STEEL_ERR_INVALID_ARGUMENT,
    }
}

fn has_block_hook(hooks: &SteelBlockHooks) -> bool {
    hooks.get_state_for_placement.is_some()
        || hooks.on_place.is_some()
        || hooks.use_item_on.is_some()
        || hooks.player_destroy.is_some()
        || hooks.random_tick.is_some()
        || hooks.scheduled_tick.is_some()
        || hooks.neighbor_changed.is_some()
        || hooks.can_be_replaced.is_some()
}

unsafe extern "C" fn host_register_block(
    desc: *const SteelBlockDesc,
    hooks: *const SteelBlockHooks,
) -> i32 {
    if desc.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    if HOST.phase() != STEEL_LEVEL_CONTENT {
        return STEEL_ERR_WRONG_PHASE;
    }
    let Some(registry) = current_registry() else {
        return STEEL_ERR_WRONG_PHASE;
    };
    // SAFETY: the extension supplies the descriptor for this call.
    let desc = unsafe { &*desc };
    let hooks = if hooks.is_null() {
        SteelBlockHooks::default()
    } else {
        // SAFETY: the extension supplies the hook table for this call.
        unsafe { *hooks }
    };
    match convert::register_block(desc, registry) {
        Ok(block) => {
            if has_block_hook(&hooks) {
                let key = block.key.clone();
                HOST.registrations
                    .lock()
                    .blocks
                    .push(crate::state::BlockRegistration { key, hooks });
            }
            STEEL_OK
        }
        Err(error) => {
            set_last_error(error);
            STEEL_ERR_INVALID_ARGUMENT
        }
    }
}

unsafe extern "C" fn host_register_item(
    desc: *const SteelItemDesc,
    hooks: *const SteelItemHooks,
) -> i32 {
    if desc.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    if HOST.phase() != STEEL_LEVEL_CONTENT {
        return STEEL_ERR_WRONG_PHASE;
    }
    let Some(registry) = current_registry() else {
        return STEEL_ERR_WRONG_PHASE;
    };
    // SAFETY: the extension supplies the descriptor for this call.
    let desc = unsafe { &*desc };
    let hooks = if hooks.is_null() {
        SteelItemHooks::default()
    } else {
        // SAFETY: the extension supplies the hook table for this call.
        unsafe { *hooks }
    };
    match convert::register_item(desc, registry) {
        Ok(item) => {
            if hooks.use_item.is_some() || hooks.use_item_on.is_some() {
                let key = item.key.clone();
                HOST.registrations
                    .lock()
                    .items
                    .push(crate::state::ItemRegistration { key, hooks });
            }
            STEEL_OK
        }
        Err(error) => {
            set_last_error(error);
            STEEL_ERR_INVALID_ARGUMENT
        }
    }
}

unsafe extern "C" fn host_register_block_entity_type(
    desc: *const SteelBlockEntityDesc,
    hooks: *const SteelBlockEntityHooks,
) -> i32 {
    if desc.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    if HOST.phase() != STEEL_LEVEL_CONTENT {
        return STEEL_ERR_WRONG_PHASE;
    }
    let Some(registry) = current_registry() else {
        return STEEL_ERR_WRONG_PHASE;
    };
    // SAFETY: the extension supplies the descriptor for this call.
    let desc = unsafe { &*desc };
    let hooks = if hooks.is_null() {
        SteelBlockEntityHooks::default()
    } else {
        // SAFETY: the extension supplies the hook table for this call.
        unsafe { *hooks }
    };
    match convert::register_block_entity_type(desc, registry) {
        Ok(()) => {
            let key = match convert::identifier(desc.key) {
                Ok(key) => key,
                Err(error) => {
                    set_last_error(error);
                    return STEEL_ERR_INVALID_ARGUMENT;
                }
            };
            HOST.registrations
                .lock()
                .block_entities
                .push(crate::state::BlockEntityRegistration { key, hooks });
            STEEL_OK
        }
        Err(error) => {
            set_last_error(error);
            STEEL_ERR_INVALID_ARGUMENT
        }
    }
}

unsafe extern "C" fn host_register_entity_type(
    desc: *const SteelEntityDesc,
    hooks: *const SteelEntityHooks,
) -> i32 {
    if desc.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    if HOST.phase() != STEEL_LEVEL_CONTENT {
        return STEEL_ERR_WRONG_PHASE;
    }
    let Some(registry) = current_registry() else {
        return STEEL_ERR_WRONG_PHASE;
    };
    // SAFETY: the extension supplies the descriptor for this call.
    let desc = unsafe { &*desc };
    let hooks = if hooks.is_null() {
        SteelEntityHooks::default()
    } else {
        // SAFETY: the extension supplies the hook table for this call.
        unsafe { *hooks }
    };
    match convert::register_entity_type(desc, registry) {
        Ok(()) => {
            let key = match convert::identifier(desc.key) {
                Ok(key) => key,
                Err(error) => {
                    set_last_error(error);
                    return STEEL_ERR_INVALID_ARGUMENT;
                }
            };
            HOST.registrations
                .lock()
                .entities
                .push(crate::state::EntityRegistration { key, hooks });
            STEEL_OK
        }
        Err(error) => {
            set_last_error(error);
            STEEL_ERR_INVALID_ARGUMENT
        }
    }
}

struct PluginListener {
    callback: SteelEventListenerFn,
    user_data: *mut c_void,
}

impl PluginListener {
    fn call(&self, event: &mut SteelEvent) -> u32 {
        // SAFETY: the extension supplied this callback and user data.
        unsafe { (self.callback)(std::ptr::from_mut(event), self.user_data) }
    }
}

// SAFETY: extension listeners are registered once and the extension contract
// requires thread-safe user data.
unsafe impl Send for PluginListener {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for PluginListener {}

fn to_abi(event: &Event) -> SteelEvent {
    let mut abi = SteelEvent {
        kind: 0,
        flags: 0,
        world: SteelWorldRef::INVALID,
        x: 0,
        y: 0,
        z: 0,
        block_state: SteelBlockState(0),
        entity: SteelEntityRef::INVALID,
        text: SteelStrView::EMPTY,
    };
    match event {
        Event::BlockBreak(event) => {
            abi.kind = steel_abi::STEEL_EVENT_BLOCK_BREAK;
            abi.world = SteelWorldRef(world_id(&event.world));
            abi.x = event.pos.x();
            abi.y = event.pos.y();
            abi.z = event.pos.z();
            abi.block_state = SteelBlockState(u32::from(event.state.0));
            if let Some(id) = event.player_id {
                abi.entity = SteelEntityRef {
                    world: abi.world,
                    entity: id,
                };
            }
        }
        Event::BlockPlace(event) => {
            abi.kind = steel_abi::STEEL_EVENT_BLOCK_PLACE;
            abi.world = SteelWorldRef(world_id(&event.world));
            abi.x = event.pos.x();
            abi.y = event.pos.y();
            abi.z = event.pos.z();
            abi.block_state = SteelBlockState(u32::from(event.state.0));
            if let Some(id) = event.player_id {
                abi.entity = SteelEntityRef {
                    world: abi.world,
                    entity: id,
                };
            }
        }
        Event::PlayerJoin(event) => {
            abi.kind = steel_abi::STEEL_EVENT_PLAYER_JOIN;
            abi.world = SteelWorldRef(world_id(&event.world));
            abi.entity = SteelEntityRef {
                world: abi.world,
                entity: event.player_id,
            };
        }
        Event::PlayerLeave(event) => {
            abi.kind = steel_abi::STEEL_EVENT_PLAYER_LEAVE;
            abi.world = SteelWorldRef(world_id(&event.world));
            abi.entity = SteelEntityRef {
                world: abi.world,
                entity: event.player_id,
            };
        }
        Event::Chat(event) => {
            abi.kind = steel_abi::STEEL_EVENT_CHAT;
            abi.world = SteelWorldRef(world_id(&event.world));
            abi.entity = SteelEntityRef {
                world: abi.world,
                entity: event.player_id,
            };
            abi.text = SteelStrView::from_str(&event.message);
        }
    }
    if event.is_cancelled() {
        abi.flags |= STEEL_EVENT_CANCELLED;
    }
    abi
}

unsafe extern "C" fn host_register_event_listener(
    kind: u32,
    priority: i32,
    callback: Option<SteelEventListenerFn>,
    user_data: *mut c_void,
) -> i32 {
    if HOST.phase() == STEEL_LEVEL_CONTENT {
        return STEEL_ERR_WRONG_PHASE;
    }
    let Some(kind) = EventKind::from_abi_kind(kind) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(callback) = callback else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let listener = PluginListener {
        callback,
        user_data,
    };
    events::register_listener(kind, priority, move |event| {
        let mut abi = to_abi(event);
        let result = listener.call(&mut abi);
        if abi.flags & STEEL_EVENT_CANCELLED != 0 || result != 0 {
            event.cancel();
            EventOutcome::Cancel
        } else {
            EventOutcome::Continue
        }
    });
    STEEL_OK
}

/// A command callback registered by an extension plus its opaque user data.
struct CommandHandler {
    callback: SteelCommandHandlerFn,
    user_data: *mut c_void,
}

impl CommandHandler {
    fn execute(&self, context: &CommandContext<'_>, command: &str) -> Result<i32, CommandError> {
        let source = context.source();
        let player = source.player();
        let entity = player.map_or(SteelEntityRef::INVALID, |player| SteelEntityRef {
            world: SteelWorldRef(world_id(source.world())),
            entity: player.id(),
        });
        let args = context.string("args").unwrap_or("");
        // SAFETY: the callback and its user data stay valid for the lifetime of
        // the loaded extension library, and both views are valid for this call.
        let status = unsafe {
            (self.callback)(
                self.user_data,
                entity,
                u8::from(player.is_some()),
                SteelStrView::from_str(command),
                SteelStrView::from_str(args),
            )
        };
        Ok(i32::from(status != 0))
    }
}

// SAFETY: extension command handlers are registered once and the extension
// contract requires `user_data` to remain valid and thread-safe for the
// lifetime of the loaded library.
unsafe impl Send for CommandHandler {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for CommandHandler {}

/// Namespace used for extension commands when the extension manifest id cannot
/// be resolved.
const FALLBACK_PLUGIN_NAMESPACE: &str = "steel_plugin";

/// Derives an identifier namespace from the currently initializing extension.
fn plugin_namespace() -> String {
    let id = HOST.extension_id.read();
    let Some(id) = id.as_deref() else {
        return FALLBACK_PLUGIN_NAMESPACE.to_owned();
    };
    let namespace: String = id
        .chars()
        .map(|character| {
            let character = character.to_ascii_lowercase();
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if namespace.is_empty() {
        FALLBACK_PLUGIN_NAMESPACE.to_owned()
    } else {
        namespace
    }
}

unsafe extern "C" fn host_extension_path() -> SteelStrView {
    let path = HOST.extension_path.read();
    path.as_deref()
        .map_or(SteelStrView::EMPTY, |path| scratch_view(path))
}

unsafe extern "C" fn host_register_command(
    name: SteelStrView,
    description: SteelStrView,
    permission: SteelStrView,
    handler: Option<SteelCommandHandlerFn>,
    user_data: *mut c_void,
) -> i32 {
    if HOST.phase() == STEEL_LEVEL_CONTENT {
        return STEEL_ERR_WRONG_PHASE;
    }
    let Ok(name) = convert::string(name) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let name = name.trim_start_matches('/').to_owned();
    if name.is_empty() || name.contains(char::is_whitespace) || name.contains(':') {
        set_last_error(format!("invalid plugin command name `{name}`"));
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // Descriptions are accepted for ABI parity; the command API has no
    // description field, so the value is validated and dropped.
    if convert::string(description).is_err() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let Ok(permission) = convert::string(permission) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let permission = if permission.is_empty() {
        None
    } else {
        match PermissionKey::parse(permission) {
            Ok(key) => Some(PermissionExpr::key(key)),
            Err(error) => {
                set_last_error(format!("invalid command permission: {error}"));
                return STEEL_ERR_INVALID_ARGUMENT;
            }
        }
    };
    let Some(callback) = handler else {
        set_last_error(format!("command `{name}` has no handler"));
        return STEEL_ERR_INVALID_ARGUMENT;
    };

    let mut registration =
        CommandRegistration::new(Identifier::new(plugin_namespace(), name.clone()), {
            let handler = Arc::new(CommandHandler {
                callback,
                user_data,
            });
            let command: Arc<str> = Arc::from(name.as_str());
            move || {
                let literal_name: Box<str> = Box::from(command.as_ref());
                let root_handler = Arc::clone(&handler);
                let root_command = Arc::clone(&command);
                let argument_handler = Arc::clone(&handler);
                let argument_command = Arc::clone(&command);
                literal(literal_name)
                    .executes(move |context| root_handler.execute(context, root_command.as_ref()))
                    .then(argument("args", CommandArgument::greedy_string()).executes(
                        move |context| argument_handler.execute(context, argument_command.as_ref()),
                    ))
            }
        });
    registration = if let Some(permission) = permission {
        registration.permission(permission)
    } else {
        registration.default_access()
    };

    {
        let mut commands = HOST.commands.lock();
        if commands.iter().any(|registered| registered == &name) {
            set_last_error(format!("command `{name}` is already registered"));
            return STEEL_ERR_ALREADY_EXISTS;
        }
        commands.push(name);
    }
    register_plugin_command(registration);
    STEEL_OK
}

unsafe extern "C" fn host_entity_get_name(entity: SteelEntityRef, out: *mut SteelStrView) -> i32 {
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let Some(world) = world(entity.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(shared) = world.get_entity_by_id(entity.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Some(player) = shared.as_player() else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe { *out = scratch_view(player.gameprofile.name()) };
    STEEL_OK
}

unsafe extern "C" fn host_entity_get_uuid(entity: SteelEntityRef, out: *mut SteelStrView) -> i32 {
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let Some(world) = world(entity.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(shared) = world.get_entity_by_id(entity.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Some(player) = shared.as_player() else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let uuid = player.gameprofile.id().to_string();
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe { *out = scratch_view(&uuid) };
    STEEL_OK
}

unsafe extern "C" fn host_player_send_message(
    entity: SteelEntityRef,
    message: SteelStrView,
) -> i32 {
    let Some(world) = world(entity.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(shared) = world.get_entity_by_id(entity.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Some(player) = shared.as_player() else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Ok(message) = convert::string(message) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    player.send_message(&TextComponent::plain(message));
    STEEL_OK
}

unsafe extern "C" fn host_entity_get_health(entity: SteelEntityRef, out: *mut f32) -> i32 {
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let Some(world) = world(entity.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(shared) = world.get_entity_by_id(entity.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Some(living) = shared.as_living_entity() else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe { *out = living.get_health() };
    STEEL_OK
}

unsafe extern "C" fn host_entity_set_health(entity: SteelEntityRef, health: f32) -> i32 {
    let Some(world) = world(entity.world) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(shared) = world.get_entity_by_id(entity.entity) else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Some(living) = shared.as_living_entity() else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    living.set_health(health);
    STEEL_OK
}

unsafe extern "C" fn host_world_get_spawn(world_ref: SteelWorldRef, out: *mut SteelVec3) -> i32 {
    let Some(world) = world(world_ref) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let spawn = world.level_data.read().data().spawn_pos();
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe {
        *out = SteelVec3 {
            x: f64::from(spawn.x()) + 0.5,
            y: f64::from(spawn.y()),
            z: f64::from(spawn.z()) + 0.5,
        }
    };
    STEEL_OK
}

unsafe extern "C" fn host_world_player_count(world_ref: SteelWorldRef) -> u32 {
    world(world_ref).map_or(0, |world| {
        u32::try_from(world.players.len()).unwrap_or(u32::MAX)
    })
}

fn player_at_index(world_ref: SteelWorldRef, index: u32) -> Option<SteelEntityRef> {
    let world = world(world_ref)?;
    let mut current = 0u32;
    let mut found = None;
    world.players.iter_players(|_uuid, player| {
        if current == index {
            found = Some(SteelEntityRef {
                world: world_ref,
                entity: player.id(),
            });
            false
        } else {
            current += 1;
            true
        }
    });
    found
}

unsafe extern "C" fn host_world_player_at(
    world_ref: SteelWorldRef,
    index: u32,
    out: *mut SteelOptEntityRef,
) -> i32 {
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let found = player_at_index(world_ref, index);
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe {
        *out = found.map_or(SteelOptEntityRef::default(), |value| SteelOptEntityRef {
            present: 1,
            value,
        })
    };
    if found.is_some() {
        STEEL_OK
    } else {
        STEEL_ERR_NOT_FOUND
    }
}

unsafe extern "C" fn host_world_find_player(
    world_ref: SteelWorldRef,
    name: SteelStrView,
    out: *mut SteelOptEntityRef,
) -> i32 {
    let Ok(name) = convert::string(name) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let Some(world) = world(world_ref) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    if out.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let mut found = None;
    world.players.iter_players(|_uuid, player| {
        if player.gameprofile.name() == name.as_str() {
            found = Some(SteelEntityRef {
                world: world_ref,
                entity: player.id(),
            });
            false
        } else {
            true
        }
    });
    // SAFETY: the extension supplied a writable out-pointer.
    unsafe {
        *out = found.map_or(SteelOptEntityRef::default(), |value| SteelOptEntityRef {
            present: 1,
            value,
        })
    };
    if found.is_some() {
        STEEL_OK
    } else {
        STEEL_ERR_NOT_FOUND
    }
}

/// Host function table published to every extension.
pub static INTERFACE: SteelInterface = SteelInterface {
    struct_size: std::mem::size_of::<SteelInterface>() as u32,
    api_version: STEEL_ABI_VERSION,
    log: Some(host_log),
    last_error: Some(host_last_error),
    block_by_key: Some(host_block_by_key),
    item_by_key: Some(host_item_by_key),
    block_default_state: Some(host_block_default_state),
    state_block: Some(host_state_block),
    state_with_bool: Some(host_state_with_bool),
    state_with_int: Some(host_state_with_int),
    state_with_enum: Some(host_state_with_enum),
    state_get_bool: Some(host_state_get_bool),
    state_get_int: Some(host_state_get_int),
    state_get_enum: Some(host_state_get_enum),
    world_get_block: Some(host_world_get_block),
    world_set_block: Some(host_world_set_block),
    world_destroy_block: Some(host_world_destroy_block),
    world_get_entity: Some(host_world_get_entity),
    world_get_block_entity: Some(host_world_get_block_entity),
    world_spawn_entity: Some(host_world_spawn_entity),
    world_play_sound: Some(host_world_play_sound),
    world_broadcast_payload: Some(host_world_broadcast_payload),
    player_send_payload: Some(host_player_send_payload),
    entity_get_position: Some(host_entity_get_position),
    entity_teleport: Some(host_entity_teleport),
    item_stack_new: Some(host_item_stack_new),
    item_stack_clone: Some(host_item_stack_clone),
    item_stack_free: Some(host_item_stack_free),
    item_stack_count: Some(host_item_stack_count),
    item_stack_set_count: Some(host_item_stack_set_count),
    item_stack_item: Some(host_item_stack_item),
    nbt_new: Some(host_nbt_new),
    nbt_clone: Some(host_nbt_clone),
    nbt_free: Some(host_nbt_free),
    nbt_set_int: Some(host_nbt_set_int),
    nbt_get_int: Some(host_nbt_get_int),
    nbt_set_string: Some(host_nbt_set_string),
    nbt_get_string: Some(host_nbt_get_string),
    nbt_contains: Some(host_nbt_contains),
    nbt_remove: Some(host_nbt_remove),
    register_block: Some(host_register_block),
    register_item: Some(host_register_item),
    register_block_entity_type: Some(host_register_block_entity_type),
    register_entity_type: Some(host_register_entity_type),
    register_event_listener: Some(host_register_event_listener),
    extension_path: Some(host_extension_path),
    register_command: Some(host_register_command),
    entity_get_name: Some(host_entity_get_name),
    entity_get_uuid: Some(host_entity_get_uuid),
    player_send_message: Some(host_player_send_message),
    entity_get_health: Some(host_entity_get_health),
    entity_set_health: Some(host_entity_set_health),
    world_get_spawn: Some(host_world_get_spawn),
    world_player_count: Some(host_world_player_count),
    world_player_at: Some(host_world_player_at),
    world_find_player: Some(host_world_find_player),
    classdb_get_method_bind: Some(steel_api::classdb::classdb_get_method_bind),
    method_bind_ptrcall: Some(steel_api::classdb::method_bind_ptrcall),
    register_extension_class: Some(steel_api::classdb::register_extension_class),
    classdb_construct: Some(steel_api::classdb::classdb_construct),
    classdb_method_bind_is_extension: Some(steel_api::classdb::method_bind_is_extension),
};
