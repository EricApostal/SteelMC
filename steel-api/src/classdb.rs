//! ClassDB-style MethodBind registry and ptrcall dispatch.

use std::ffi::c_void;
use std::sync::LazyLock;

use steel_abi::*;
use steel_utils::locks::SyncRwLock;

use crate::{CLASSES, METHOD_BINDS};

struct ExtensionClass {
    name: &'static str,
    inherits: Option<&'static str>,
    construct: Option<SteelClassConstructFn>,
    construct_user_data: *mut c_void,
    methods: &'static [SteelMethodBindDesc],
}

// SAFETY: extension class metadata is immutable after registration and the
// extension contract requires thunk user data to be thread-safe.
unsafe impl Send for ExtensionClass {}
unsafe impl Sync for ExtensionClass {}

static EXTENSION_CLASSES: LazyLock<SyncRwLock<Vec<&'static ExtensionClass>>> =
    LazyLock::new(|| SyncRwLock::new(Vec::new()));

fn view(value: SteelStrView) -> Option<&'static str> {
    // SAFETY: the host supplies the view for the duration of the call; the
    // ClassDB copies extension names before returning.
    unsafe { value.as_str() }
}

fn method_in<'a>(
    methods: &'a [SteelMethodBindDesc],
    name: &str,
    hash: u32,
) -> Option<&'a SteelMethodBindDesc> {
    methods.iter().find(|method| {
        // SAFETY: native entries point at static strings.
        let method_name = unsafe { method.method_name.as_str() }.unwrap_or_default();
        method_name == name && (hash == 0 || method.hash == hash)
    })
}

pub unsafe extern "C" fn classdb_get_method_bind(
    class_name: SteelStrView,
    method_name: SteelStrView,
    hash: u32,
) -> *const SteelMethodBind {
    let Some(mut class_name) = view(class_name) else {
        return std::ptr::null();
    };
    let Some(method_name) = view(method_name) else {
        return std::ptr::null();
    };
    loop {
        if let Some(method) = method_in(METHOD_BINDS, method_name, hash).filter(|method| {
            // SAFETY: native entries point at static strings.
            unsafe { method.class_name.as_str() }.is_some_and(|name| name == class_name)
        }) {
            return std::ptr::from_ref(method).cast();
        }
        let extension = {
            let classes = EXTENSION_CLASSES.read();
            classes
                .iter()
                .copied()
                .find(|class| class.name == class_name)
        };
        if let Some(class) = extension {
            if let Some(method) = method_in(class.methods, method_name, hash) {
                return std::ptr::from_ref(method).cast();
            }
            match class.inherits {
                Some(inherits) => class_name = inherits,
                None => return std::ptr::null(),
            }
            continue;
        }
        let native = CLASSES.iter().find(|class| class.name == class_name);
        match native.and_then(|class| class.inherits) {
            Some(inherits) => class_name = inherits,
            None => return std::ptr::null(),
        }
    }
}

pub unsafe extern "C" fn method_bind_ptrcall(
    bind: *const SteelMethodBind,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
) -> i32 {
    if bind.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: `bind` was returned by `classdb_get_method_bind` and points at a
    // `SteelMethodBindDesc`.
    let method = unsafe { &*bind.cast::<SteelMethodBindDesc>() };
    let Some(thunk) = method.thunk else {
        return STEEL_ERR_UNSUPPORTED;
    };
    // SAFETY: the caller supplies argument/return pointers matching the
    // method's generated signature.
    unsafe { thunk(method.user_data, instance, args, ret) };
    STEEL_OK
}

pub unsafe extern "C" fn register_extension_class(desc: *const SteelExtensionClassDesc) -> i32 {
    if desc.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the extension supplies a valid descriptor for the call.
    let desc = unsafe { &*desc };
    let Some(name) = view(desc.class_name) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let inherits = view(desc.inherits);
    let name: &'static str = Box::leak(name.to_owned().into_boxed_str());
    let inherits: Option<&'static str> =
        inherits.map(|value| Box::leak(value.to_owned().into_boxed_str()) as &'static str);
    // SAFETY: the extension supplies `method_count` method descriptors.
    let methods = unsafe {
        if desc.method_count == 0 {
            &[][..]
        } else if desc.methods.is_null() {
            return STEEL_ERR_INVALID_ARGUMENT;
        } else {
            std::slice::from_raw_parts(desc.methods, desc.method_count)
        }
    };
    let mut binds = Vec::with_capacity(methods.len());
    for method in methods {
        let Some(method_name) = view(method.method_name) else {
            return STEEL_ERR_INVALID_ARGUMENT;
        };
        let method_name: &'static str = Box::leak(method_name.to_owned().into_boxed_str());
        binds.push(SteelMethodBindDesc {
            class_name: SteelStrView::from_str(name),
            method_name: SteelStrView::from_str(method_name),
            hash: method.hash,
            thunk: method.thunk,
            user_data: method.user_data,
        });
    }
    let class = Box::leak(Box::new(ExtensionClass {
        name,
        inherits,
        construct: desc.construct,
        construct_user_data: desc.construct_user_data,
        methods: Box::leak(binds.into_boxed_slice()),
    }));
    EXTENSION_CLASSES.write().push(class);
    STEEL_OK
}

/// Reports whether a MethodBind came from an extension class.
pub unsafe extern "C" fn method_bind_is_extension(bind: *const SteelMethodBind) -> bool {
    if bind.is_null() {
        return false;
    }
    let bind = bind.cast::<SteelMethodBindDesc>();
    let classes = EXTENSION_CLASSES.read();
    // SAFETY: extension method descriptors are leaked for the process lifetime.
    classes.iter().any(|class| {
        class
            .methods
            .iter()
            .any(|method| std::ptr::eq(method, bind))
    })
}

pub unsafe extern "C" fn classdb_construct(
    class_name: SteelStrView,
    args: *const *const c_void,
    ret: *mut SteelRef,
) -> i32 {
    let Some(class_name) = view(class_name) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    if ret.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let class = {
        let classes = EXTENSION_CLASSES.read();
        classes
            .iter()
            .copied()
            .find(|class| class.name == class_name)
    };
    let Some(class) = class else {
        return STEEL_ERR_NOT_FOUND;
    };
    let Some(construct) = class.construct else {
        return STEEL_ERR_UNSUPPORTED;
    };
    // SAFETY: the caller supplies argument pointers matching the construct thunk.
    unsafe { construct(class.construct_user_data, args, ret) }
}
