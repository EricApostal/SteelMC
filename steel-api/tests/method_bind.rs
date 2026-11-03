//! MethodBind/ClassDB conformance tests.

use std::ffi::c_void;

use steel_abi::*;
use steel_api::classdb::{
    classdb_construct, classdb_get_method_bind, method_bind_ptrcall, register_extension_class,
};
use steel_registry::{RegistryEntry, init_vanilla_registry, vanilla_blocks};

fn hash_of(class: &str, method: &str) -> u32 {
    steel_api::METHOD_BINDS
        .iter()
        .find(|entry| {
            // SAFETY: generated entries point at static strings.
            (unsafe { entry.class_name.as_str() }) == Some(class)
                && (unsafe { entry.method_name.as_str() }) == Some(method)
        })
        .map_or(0, |entry| entry.hash)
}

#[test]
fn block_default_state_and_property_roundtrip() {
    init_vanilla_registry();
    let stone = &vanilla_blocks::STONE;
    let block = SteelRef::registry(STEEL_REF_BLOCK, u32::try_from(stone.id()).unwrap());

    let bind = unsafe {
        classdb_get_method_bind(
            SteelStrView::from_str("Block"),
            SteelStrView::from_str("default_state"),
            hash_of("Block", "default_state"),
        )
    };
    assert!(!bind.is_null());
    let mut state = 0u32;
    assert_eq!(
        unsafe {
            method_bind_ptrcall(
                bind,
                block,
                std::ptr::null(),
                std::ptr::from_mut(&mut state).cast(),
            )
        },
        STEEL_OK
    );
    assert_eq!(state, u32::from(stone.default_state().0));

    let stairs = &vanilla_blocks::OAK_STAIRS;
    let default = u32::from(stairs.default_state().0);
    let instance = SteelRef::registry(STEEL_REF_BLOCK_STATE, default);
    let name = SteelStrView::from_str("waterlogged");
    let value = true;
    let args = [
        std::ptr::from_ref(&name).cast::<c_void>(),
        std::ptr::from_ref(&value).cast::<c_void>(),
    ];
    let bind = unsafe {
        classdb_get_method_bind(
            SteelStrView::from_str("BlockState"),
            SteelStrView::from_str("with_bool"),
            hash_of("BlockState", "with_bool"),
        )
    };
    assert!(!bind.is_null());
    let mut changed = 0u32;
    assert_eq!(
        unsafe {
            method_bind_ptrcall(
                bind,
                instance,
                args.as_ptr(),
                std::ptr::from_mut(&mut changed).cast(),
            )
        },
        STEEL_OK
    );
    assert_ne!(changed, default);
}

unsafe extern "C" fn greet_thunk(
    _user_data: *mut c_void,
    _instance: SteelRef,
    _args: *const *const c_void,
    ret: *mut c_void,
) {
    assert!(!ret.is_null());
    // SAFETY: the caller supplied an i32 return slot.
    unsafe { *ret.cast::<i32>() = 7 };
}

unsafe extern "C" fn construct(
    _user_data: *mut c_void,
    _args: *const *const c_void,
    ret: *mut SteelRef,
) -> i32 {
    assert!(!ret.is_null());
    // SAFETY: the caller supplied a SteelRef return slot.
    unsafe { *ret = SteelRef::owned(STEEL_REF_EXTENSION, 1, 1) };
    STEEL_OK
}

#[test]
fn extension_class_registers_and_calls() {
    let methods = [SteelExtensionMethodDesc {
        method_name: SteelStrView::from_str("greet"),
        hash: 42,
        thunk: Some(greet_thunk),
        user_data: std::ptr::null_mut(),
    }];
    let desc = SteelExtensionClassDesc {
        struct_size: std::mem::size_of::<SteelExtensionClassDesc>() as u32,
        class_name: SteelStrView::from_str("Greeter"),
        inherits: SteelStrView::from_str("Object"),
        construct: Some(construct),
        construct_user_data: std::ptr::null_mut(),
        methods: methods.as_ptr(),
        method_count: methods.len(),
    };
    assert_eq!(unsafe { register_extension_class(&desc) }, STEEL_OK);
    let mut instance = SteelRef::NONE;
    assert_eq!(
        unsafe {
            classdb_construct(
                SteelStrView::from_str("Greeter"),
                std::ptr::null(),
                std::ptr::from_mut(&mut instance),
            )
        },
        STEEL_OK
    );
    assert_eq!(instance.kind, STEEL_REF_EXTENSION);
    let bind = unsafe {
        classdb_get_method_bind(
            SteelStrView::from_str("Greeter"),
            SteelStrView::from_str("greet"),
            42,
        )
    };
    assert!(!bind.is_null());
    let mut result = 0i32;
    assert_eq!(
        unsafe {
            method_bind_ptrcall(
                bind,
                instance,
                std::ptr::null(),
                std::ptr::from_mut(&mut result).cast(),
            )
        },
        STEEL_OK
    );
    assert_eq!(result, 7);
}
