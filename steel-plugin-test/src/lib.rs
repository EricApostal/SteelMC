//! Conformance extension used by `steel-plugin-host` integration tests.

#![allow(
    clippy::wildcard_imports,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    clippy::absolute_paths,
    reason = "the conformance extension intentionally exercises the whole ABI surface with static inputs"
)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use steel_abi::*;

static INTERFACE: AtomicPtr<SteelInterface> = AtomicPtr::new(std::ptr::null_mut());
static PLACE_HOOK_CALLS: AtomicU64 = AtomicU64::new(0);
static PLACE_EVENTS: AtomicU64 = AtomicU64::new(0);
static LAST_WORLD: AtomicU64 = AtomicU64::new(u64::MAX);

#[must_use]
fn interface() -> Option<&'static SteelInterface> {
    let pointer = INTERFACE.load(Ordering::Acquire);
    // SAFETY: the host keeps the interface alive for the process lifetime.
    // SAFETY: the host keeps the interface alive for the process lifetime.
    (!pointer.is_null()).then(|| unsafe { &*pointer })
}

unsafe extern "C" fn on_place(
    _user_data: *mut c_void,
    world: SteelWorldRef,
    _x: i32,
    _y: i32,
    _z: i32,
    _state: SteelBlockState,
    _old_state: SteelBlockState,
    _moved_by_piston: u8,
) {
    PLACE_HOOK_CALLS.fetch_add(1, Ordering::Relaxed);
    LAST_WORLD.store(u64::from(world.0), Ordering::Relaxed);
}

unsafe extern "C" fn random_tick(
    _user_data: *mut c_void,
    _world: SteelWorldRef,
    _x: i32,
    _y: i32,
    _z: i32,
    _state: SteelBlockState,
) {
}

unsafe extern "C" fn use_item_on(
    _user_data: *mut c_void,
    _world: SteelWorldRef,
    _x: i32,
    _y: i32,
    _z: i32,
    _state: SteelBlockState,
    _player: SteelEntityRef,
    _hand: u32,
    _clicked_face: u32,
) -> SteelInteractionResult {
    SteelInteractionResult::NOT_HANDLED
}

unsafe extern "C" fn on_place_event(_event: *mut SteelEvent, _user_data: *mut c_void) -> u32 {
    PLACE_EVENTS.fetch_add(1, Ordering::Relaxed);
    0
}

unsafe extern "C" fn initialize(level: u32, _user_data: *mut c_void) -> i32 {
    let Some(interface) = interface() else {
        return STEEL_ERR_INTERNAL;
    };
    match level {
        STEEL_LEVEL_CONTENT => {
            let name = SteelStrView::from_str("lit");
            let property = SteelPropertyDesc {
                kind: STEEL_PROPERTY_BOOL,
                name,
                min: 0,
                max: 0,
                values: std::ptr::null(),
                value_count: 0,
            };
            let properties = [property];

            let desc = SteelBlockDesc {
                key: SteelStrView::from_str("steel_test:ruby_block"),
                properties: properties.as_ptr(),
                property_count: 1,
                light: SteelLight {
                    emission: 0,
                    dampening: 15,
                    use_shape_for_occlusion: 0,
                },
                suffocating: 1,
                redstone_conductor: 1,
                ..SteelBlockDesc::default()
            };

            let hooks = SteelBlockHooks {
                on_place: Some(on_place),
                random_tick: Some(random_tick),
                use_item_on: Some(use_item_on),
                ..SteelBlockHooks::default()
            };

            let Some(register_block) = interface.register_block else {
                return STEEL_ERR_UNSUPPORTED;
            };
            // SAFETY: descriptors live for the duration of the call.
            // SAFETY: descriptors and hooks live for the duration of the call.
            let status =
                unsafe { register_block(std::ptr::from_ref(&desc), std::ptr::from_ref(&hooks)) };
            if status != STEEL_OK {
                return status;
            }

            let Some(block_by_key) = interface.block_by_key else {
                return STEEL_ERR_UNSUPPORTED;
            };
            // SAFETY: the view points at a static string.
            let block = unsafe { block_by_key(SteelStrView::from_str("steel_test:ruby_block")) };

            let item = SteelItemDesc {
                key: SteelStrView::from_str("steel_test:ruby_block"),
                block,
                max_stack_size: 64,
                ..SteelItemDesc::default()
            };
            let Some(register_item) = interface.register_item else {
                return STEEL_ERR_UNSUPPORTED;
            };
            // SAFETY: descriptors live for the duration of the call.
            let item_hooks = SteelItemHooks::default();
            // SAFETY: descriptors and hooks live for the duration of the call.
            unsafe { register_item(std::ptr::from_ref(&item), std::ptr::from_ref(&item_hooks)) }
        }
        STEEL_LEVEL_BEHAVIOR => {
            let Some(register_listener) = interface.register_event_listener else {
                return STEEL_ERR_UNSUPPORTED;
            };
            // SAFETY: the callback and user data live for the process lifetime.
            unsafe {
                register_listener(
                    STEEL_EVENT_BLOCK_PLACE,
                    0,
                    Some(on_place_event),
                    std::ptr::null_mut(),
                )
            }
        }
        STEEL_LEVEL_RUNTIME => STEEL_OK,
        _ => STEEL_ERR_INVALID_ARGUMENT,
    }
}

static EXTENSION: SteelExtension = SteelExtension {
    struct_size: std::mem::size_of::<SteelExtension>() as u32,
    api_version: STEEL_ABI_VERSION,
    id: SteelStrView::from_str("steel_test"),
    version: SteelStrView::from_str("0.1.0"),
    api_min: 1,
    api_max: 1,
    initialize: Some(initialize),
    deinitialize: None,
    user_data: std::ptr::null_mut(),
};

/// Extension entry point.
///
/// # Safety
///
/// `interface` must point to a host interface table valid for the process
/// lifetime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn steel_extension_entry(
    interface: *const SteelInterface,
) -> *const SteelExtension {
    INTERFACE.store(interface.cast_mut(), Ordering::Release);
    &EXTENSION
}

/// Returns 1 when the test block and item were registered.
#[unsafe(no_mangle)]
pub extern "C" fn steel_test_registered() -> i32 {
    let Some(interface) = interface() else {
        return 0;
    };
    let Some(block_by_key) = interface.block_by_key else {
        return 0;
    };
    let Some(item_by_key) = interface.item_by_key else {
        return 0;
    };
    // SAFETY: static string views.
    let block = unsafe { block_by_key(SteelStrView::from_str("steel_test:ruby_block")) };
    let item = unsafe { item_by_key(SteelStrView::from_str("steel_test:ruby_block")) };
    i32::from(!block.is_none() && !item.is_none())
}

/// Exercises state lookup, mutation, and readback through the host table.
#[unsafe(no_mangle)]
pub extern "C" fn steel_test_state_roundtrip() -> i32 {
    let Some(interface) = interface() else {
        return 0;
    };
    let (Some(block_by_key), Some(default_state), Some(with_bool), Some(get_bool)) = (
        interface.block_by_key,
        interface.block_default_state,
        interface.state_with_bool,
        interface.state_get_bool,
    ) else {
        return 0;
    };
    // SAFETY: static string view; all calls are on the host table.
    let block = unsafe { block_by_key(SteelStrView::from_str("steel_test:ruby_block")) };
    let default = unsafe { default_state(block) };
    let lit = unsafe { with_bool(default, SteelStrView::from_str("lit"), 1) };
    let mut value = 0u8;
    let status = unsafe { get_bool(lit, SteelStrView::from_str("lit"), &mut value) };
    i32::from(status == STEEL_OK && value == 1)
}

/// Number of times the test block's `on_place` hook ran.
#[unsafe(no_mangle)]
pub extern "C" fn steel_test_place_hook_calls() -> u64 {
    PLACE_HOOK_CALLS.load(Ordering::Relaxed)
}

/// Number of block-place events delivered to the extension.
#[unsafe(no_mangle)]
pub extern "C" fn steel_test_place_events() -> u64 {
    PLACE_EVENTS.load(Ordering::Relaxed)
}

/// World id observed by the last `on_place` hook.
#[unsafe(no_mangle)]
pub extern "C" fn steel_test_last_world() -> u64 {
    LAST_WORLD.load(Ordering::Relaxed)
}
