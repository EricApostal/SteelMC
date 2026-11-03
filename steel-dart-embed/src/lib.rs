//! In-process Dart VM embedding for Steel extensions.
//!
//! The library loads `libdart_engine_jit_shared.so`, loads a Dart kernel, calls
//! the plugin's top-level `main`, and exposes the Steel C ABI to Dart through
//! `dart:ffi`. Host callbacks acquire the Dart isolate (the engine serializes
//! them), invoke `steelDispatchCommand`/`steelDispatchEvent`, and release it.
//!
//! Dart extension classes are declared with `steel_shim_register_class`. The
//! shim forwards them through `interface.register_extension_class` immediately
//! (with a behavior-phase retry for hosts that are not ready yet); the `ClassDB`
//! then constructs instances
//! and calls methods through the shim thunks, which invoke
//! `steelConstructExtension`/`steelInvokeExtension` in
//! `package:steel_extension/src/steel/extension.dart`.
//!
//! Sidecar next to the library:
//! ```json
//! {"id":"essentials","version":"0.1.0","kernel":"essentials.dill","vm":"/path/libdart_engine_jit_shared.so"}
//! ```

#![allow(
    missing_docs,
    reason = "the embedding internals are documented by the module comment"
)]

use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};

use steel_abi::*;

type DartHandle = *mut c_void;

/// Value type codes exchanged with
/// `clients/dart/lib/src/steel/extension.dart`. The Dart `SteelValueType`
/// ordinals must match these constants.
const STEEL_VALUE_I32: u8 = 0;
const STEEL_VALUE_U32: u8 = 1;
const STEEL_VALUE_F32: u8 = 2;
const STEEL_VALUE_F64: u8 = 3;
const STEEL_VALUE_BOOLEAN: u8 = 4;
const STEEL_VALUE_STRING: u8 = 5;
const STEEL_VALUE_REF: u8 = 6;
const STEEL_VALUE_VOID: u8 = 7;

/// Library that hosts the Dart-side extension registry entry points.
const EXTENSION_LIBRARY: &str = "package:steel_extension/src/steel/extension.dart";

static INTERFACE: AtomicPtr<SteelInterface> = AtomicPtr::new(std::ptr::null_mut());

/// Copy of the host table handed to Dart. Cross-plugin calls temporarily leave
/// this isolate so the callee's shim can enter its own isolate.
static LOCAL_INTERFACE: OnceLock<SteelInterface> = OnceLock::new();
static CONFIG: OnceLock<Config> = OnceLock::new();
static MANIFEST: OnceLock<SteelExtension> = OnceLock::new();
static ENGINE: LazyLock<Mutex<Option<Engine>>> = LazyLock::new(|| Mutex::new(None));
static REGISTRATIONS: LazyLock<Mutex<Vec<Registration>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Extension classes declared by the Dart side, in registration order.
///
/// The classes are appended by `steel_shim_register_class` and never removed,
/// so the strings and method metadata stay valid for the process lifetime and
/// may be referenced by the descriptors handed to the host.
static EXTENSION_CLASSES: LazyLock<Mutex<Vec<ExtensionClass>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Next globally unique method id handed out to `steel_shim_register_class`.
static NEXT_EXTENSION_METHOD_ID: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// Backing storage for string results written by the method thunk.
    ///
    /// A `SteelStrView` return borrowed from Dart memory is only valid until
    /// `Dart_ExitScope`, so the shim copies the bytes here; the view remains
    /// readable until the next extension method call on the same thread.
    static RETURN_STRING: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// One method declared by a Dart extension class.
struct ExtensionMethod {
    name: String,
    hash: u32,
    arg_types: Vec<u8>,
    return_type: u8,
}

/// One Dart extension class declared through `steel_shim_register_class`.
struct ExtensionClass {
    name: String,
    inherits: String,
    /// Id of the first method; `user_data` of method `i` is `method_base + i`.
    method_base: usize,
    methods: Vec<ExtensionMethod>,
    /// Set once the host ClassDB accepted this class.
    registered: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
struct Config {
    id: String,
    version: String,
    kernel: PathBuf,
    vm: PathBuf,
    entry: String,
    error: Option<String>,
}

impl Config {
    fn broken(error: impl Into<String>) -> Self {
        Self {
            id: "steel_dart_embed".to_owned(),
            version: "0.0.0".to_owned(),
            kernel: PathBuf::new(),
            vm: PathBuf::new(),
            entry: "main".to_owned(),
            error: Some(error.into()),
        }
    }
}

#[derive(Clone)]
enum Registration {
    Command {
        id: u64,
        name: String,
        description: String,
        permission: String,
    },
    Event(u32),
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KernelData {
    buffer: *const u8,
    size: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct AotData {
    data: *const u8,
    text: *const u8,
}

#[repr(C)]
union SnapshotUnion {
    kernel: KernelData,
    aot: AotData,
}

#[repr(C)]
struct SnapshotData {
    script_uri: *const c_char,
    kind: i32,
    data: SnapshotUnion,
}

/// Resolved Dart embedder API. Symbols stay valid because `Engine` keeps the
/// `Library` alive.
struct DartApi {
    init: unsafe extern "C" fn(*mut *mut c_char) -> bool,
    kernel_from_file: unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> SnapshotData,
    create_isolate: unsafe extern "C" fn(SnapshotData, *mut *mut c_char) -> *mut c_void,
    acquire_isolate: unsafe extern "C" fn(*mut c_void),
    release_isolate: unsafe extern "C" fn(),
    current_isolate: unsafe extern "C" fn() -> *mut c_void,
    enter_scope: unsafe extern "C" fn(),
    exit_scope: unsafe extern "C" fn(),
    root_library: unsafe extern "C" fn() -> DartHandle,
    lookup_library: unsafe extern "C" fn(DartHandle) -> DartHandle,
    new_string: unsafe extern "C" fn(*const c_char) -> DartHandle,
    new_integer: unsafe extern "C" fn(i64) -> DartHandle,
    new_double: unsafe extern "C" fn(f64) -> DartHandle,
    new_boolean: unsafe extern "C" fn(bool) -> DartHandle,
    new_list: unsafe extern "C" fn(i64) -> DartHandle,
    list_set_at: unsafe extern "C" fn(DartHandle, i64, DartHandle) -> DartHandle,
    list_get_at: unsafe extern "C" fn(DartHandle, i64) -> DartHandle,
    invoke: unsafe extern "C" fn(DartHandle, DartHandle, i32, *mut DartHandle) -> DartHandle,
    integer_to_i64: unsafe extern "C" fn(DartHandle, *mut i64) -> DartHandle,
    double_value: unsafe extern "C" fn(DartHandle, *mut f64) -> DartHandle,
    boolean_value: unsafe extern "C" fn(DartHandle, *mut bool) -> DartHandle,
    string_to_utf8: unsafe extern "C" fn(DartHandle, *mut *mut u8, *mut i64) -> DartHandle,
    is_error: unsafe extern "C" fn(DartHandle) -> bool,
    is_null: unsafe extern "C" fn(DartHandle) -> bool,
    get_error: unsafe extern "C" fn(DartHandle) -> *const c_char,
}

struct Engine {
    api: DartApi,
    _library: libloading::Library,
    isolate: *mut c_void,
}

thread_local! {
    /// Engine whose entrypoint is currently running. `ENGINE` is published only
    /// after startup, but `onEnable` may already call into another plugin.
    static ACTIVE_ENGINE: std::cell::Cell<*const Engine> =
        const { std::cell::Cell::new(std::ptr::null()) };
}

// SAFETY: `isolate` is only accessed through `DartEngine_AcquireIsolate`,
// which serializes callers, and the remaining fields are function pointers or
// a thread-safe library handle.
unsafe impl Send for Engine {}

fn load_api(library: &libloading::Library) -> Result<DartApi, String> {
    // SAFETY: all symbols are resolved from the loaded Dart engine library and
    // have the signatures declared by `dart_api.h`/`dart_engine.h`.
    unsafe {
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                let symbol: libloading::Symbol<'_, $ty> = library
                    .get(concat!($name, "\0").as_bytes())
                    .map_err(|error| format!("missing {}: {error}", $name))?;
                *symbol
            }};
        }
        Ok(DartApi {
            init: symbol!(
                "DartEngine_Init",
                unsafe extern "C" fn(*mut *mut c_char) -> bool
            ),
            kernel_from_file: symbol!(
                "DartEngine_KernelFromFile",
                unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> SnapshotData
            ),
            create_isolate: symbol!(
                "DartEngine_CreateIsolate",
                unsafe extern "C" fn(SnapshotData, *mut *mut c_char) -> *mut c_void
            ),
            acquire_isolate: symbol!(
                "DartEngine_AcquireIsolate",
                unsafe extern "C" fn(*mut c_void)
            ),
            release_isolate: symbol!("DartEngine_ReleaseIsolate", unsafe extern "C" fn()),
            current_isolate: symbol!("Dart_CurrentIsolate", unsafe extern "C" fn() -> *mut c_void),
            enter_scope: symbol!("Dart_EnterScope", unsafe extern "C" fn()),
            exit_scope: symbol!("Dart_ExitScope", unsafe extern "C" fn()),
            root_library: symbol!("Dart_RootLibrary", unsafe extern "C" fn() -> DartHandle),
            lookup_library: symbol!(
                "Dart_LookupLibrary",
                unsafe extern "C" fn(DartHandle) -> DartHandle
            ),
            new_string: symbol!(
                "Dart_NewStringFromCString",
                unsafe extern "C" fn(*const c_char) -> DartHandle
            ),
            new_integer: symbol!("Dart_NewInteger", unsafe extern "C" fn(i64) -> DartHandle),
            new_double: symbol!("Dart_NewDouble", unsafe extern "C" fn(f64) -> DartHandle),
            new_boolean: symbol!("Dart_NewBoolean", unsafe extern "C" fn(bool) -> DartHandle),
            new_list: symbol!("Dart_NewList", unsafe extern "C" fn(i64) -> DartHandle),
            list_set_at: symbol!(
                "Dart_ListSetAt",
                unsafe extern "C" fn(DartHandle, i64, DartHandle) -> DartHandle
            ),
            list_get_at: symbol!(
                "Dart_ListGetAt",
                unsafe extern "C" fn(DartHandle, i64) -> DartHandle
            ),
            invoke: symbol!(
                "Dart_Invoke",
                unsafe extern "C" fn(DartHandle, DartHandle, i32, *mut DartHandle) -> DartHandle
            ),
            integer_to_i64: symbol!(
                "Dart_IntegerToInt64",
                unsafe extern "C" fn(DartHandle, *mut i64) -> DartHandle
            ),
            double_value: symbol!(
                "Dart_DoubleValue",
                unsafe extern "C" fn(DartHandle, *mut f64) -> DartHandle
            ),
            boolean_value: symbol!(
                "Dart_BooleanValue",
                unsafe extern "C" fn(DartHandle, *mut bool) -> DartHandle
            ),
            string_to_utf8: symbol!(
                "Dart_StringToUTF8",
                unsafe extern "C" fn(DartHandle, *mut *mut u8, *mut i64) -> DartHandle
            ),
            is_error: symbol!("Dart_IsError", unsafe extern "C" fn(DartHandle) -> bool),
            is_null: symbol!("Dart_IsNull", unsafe extern "C" fn(DartHandle) -> bool),
            get_error: symbol!(
                "Dart_GetError",
                unsafe extern "C" fn(DartHandle) -> *const c_char
            ),
        })
    }
}

fn c_string(value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| "string contains a NUL byte".to_owned())
}

fn error_text(api: &DartApi, handle: DartHandle) -> String {
    if handle.is_null() {
        return "null Dart handle".to_owned();
    }
    // SAFETY: the handle is an error handle produced by the VM.
    unsafe {
        let pointer = (api.get_error)(handle);
        if pointer.is_null() {
            "unknown Dart error".to_owned()
        } else {
            CStr::from_ptr(pointer).to_string_lossy().into_owned()
        }
    }
}

fn log(level: u32, message: &str) {
    if let Some(interface) = interface()
        && let Some(log) = interface.log
    {
        // SAFETY: the view points at a live string for the call.
        unsafe { log(level, SteelStrView::from_str(message)) };
    } else {
        eprintln!("steel-dart-embed: {message}");
    }
}

fn interface() -> Option<&'static SteelInterface> {
    let pointer = INTERFACE.load(Ordering::Acquire);
    // SAFETY: the host keeps the interface alive for the process lifetime.
    (!pointer.is_null()).then(|| unsafe { &*pointer })
}

fn read_sidecar(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read sidecar {}: {error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("invalid sidecar {}: {error}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let id = value
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "sidecar is missing `id`".to_owned())?;
    let kernel = value
        .get("kernel")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "sidecar is missing `kernel`".to_owned())?;
    let vm = value
        .get("vm")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || std::env::var("STEEL_DART_VM_LIB").map_or_else(|_| PathBuf::new(), PathBuf::from),
            PathBuf::from,
        );
    Ok(Config {
        id: id.to_owned(),
        version: value
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("0.1.0")
            .to_owned(),
        kernel: base.join(kernel),
        vm: if vm.is_absolute() { vm } else { base.join(vm) },
        entry: value
            .get("entry")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("main")
            .to_owned(),
        error: None,
    })
}

fn config_from_interface(interface: &SteelInterface) -> Config {
    let Some(path_function) = interface.extension_path else {
        return Config::broken("host does not expose extension_path");
    };
    // SAFETY: the host returns a view valid for this call.
    let view = unsafe { path_function() };
    // SAFETY: see above.
    let Some(path) = (unsafe { view.as_str() }) else {
        return Config::broken("extension path is not UTF-8");
    };
    if path.is_empty() {
        return Config::broken("extension path is empty");
    }
    let sidecar = PathBuf::from(format!("{path}.json"));
    read_sidecar(&sidecar).unwrap_or_else(|error| Config::broken(error))
}

fn start_engine(config: &Config) -> Result<Engine, String> {
    if config.vm.as_os_str().is_empty() {
        return Err("no Dart VM library; set `vm` in the sidecar or STEEL_DART_VM_LIB".to_owned());
    }
    // SAFETY: loading the configured Dart engine library is required to embed
    // the VM; the sidecar is trusted plugin configuration.
    let library = unsafe { libloading::Library::new(&config.vm) }
        .map_err(|error| format!("failed to load Dart VM {}: {error}", config.vm.display()))?;
    let api = load_api(&library)?;
    let mut error: *mut c_char = std::ptr::null_mut();
    // SAFETY: matches the Dart embedder API contract.
    if !unsafe { (api.init)(&mut error) } {
        let message = unsafe { take_error(error) };
        return Err(format!("DartEngine_Init failed: {message}"));
    }
    let kernel = c_string(
        config
            .kernel
            .to_str()
            .ok_or_else(|| "kernel path is not UTF-8".to_owned())?,
    )?;
    // SAFETY: the kernel path is a valid C string and `error` is writable.
    let snapshot = unsafe { (api.kernel_from_file)(kernel.as_ptr(), &mut error) };
    if !error.is_null() {
        let message = unsafe { take_error(error) };
        return Err(format!("failed to load Dart kernel: {message}"));
    }
    // SAFETY: the snapshot was produced by `kernel_from_file`.
    let isolate = unsafe { (api.create_isolate)(snapshot, &mut error) };
    if isolate.is_null() || !error.is_null() {
        let message = unsafe { take_error(error) };
        return Err(format!("failed to create Dart isolate: {message}"));
    }
    Ok(Engine {
        api,
        _library: library,
        isolate,
    })
}

unsafe fn take_error(pointer: *mut c_char) -> String {
    if pointer.is_null() {
        return "unknown error".to_owned();
    }
    // SAFETY: the embedder API returns a NUL-terminated allocation owned by the
    // caller; it is freed by the Dart runtime allocator via `free`.
    let message = unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned();
    // The SDK allocates these with `Utils::StrDup`/malloc. Leaking a short error
    // string is preferable to calling the wrong allocator.
    message
}

enum Arg<'a> {
    Int(i64),
    Str(&'a str),
}

fn invoke(engine: &Engine, function: &str, args: &[Arg<'_>]) -> Result<i64, String> {
    invoke_target(engine, None, function, args)
}

fn invoke_library(
    engine: &Engine,
    library_uri: &str,
    function: &str,
    args: &[Arg<'_>],
) -> Result<i64, String> {
    invoke_target(engine, Some(library_uri), function, args)
}

fn invoke_target(
    engine: &Engine,
    library_uri: Option<&str>,
    function: &str,
    args: &[Arg<'_>],
) -> Result<i64, String> {
    let api = &engine.api;
    let name = c_string(function)?;
    // SAFETY: the isolate is acquired and a scope is entered around every call.
    unsafe {
        (api.acquire_isolate)(engine.isolate);
        (api.enter_scope)();
        let handle_name = (api.new_string)(name.as_ptr());
        if (api.is_error)(handle_name) {
            let message = error_text(api, handle_name);
            (api.exit_scope)();
            (api.release_isolate)();
            return Err(message);
        }
        let target = if let Some(uri) = library_uri {
            let uri = c_string(uri)?;
            let handle = (api.lookup_library)((api.new_string)(uri.as_ptr()));
            if (api.is_error)(handle) || (api.is_null)(handle) {
                (api.root_library)()
            } else {
                handle
            }
        } else {
            (api.root_library)()
        };
        let mut handles = Vec::with_capacity(args.len());
        for argument in args {
            let handle = match argument {
                Arg::Int(value) => (api.new_integer)(*value),
                Arg::Str(value) => {
                    let text = c_string(value)?;
                    (api.new_string)(text.as_ptr())
                }
            };
            if (api.is_error)(handle) {
                let message = error_text(api, handle);
                (api.exit_scope)();
                (api.release_isolate)();
                return Err(message);
            }
            handles.push(handle);
        }
        let result = (api.invoke)(
            target,
            handle_name,
            i32::try_from(handles.len()).unwrap_or(0),
            handles.as_mut_ptr(),
        );
        let outcome = if result.is_null() || (api.is_null)(result) {
            Ok(0)
        } else if (api.is_error)(result) {
            Err(error_text(api, result))
        } else {
            let mut value = 0i64;
            let status = (api.integer_to_i64)(result, &mut value);
            if (api.is_error)(status) {
                Err(error_text(api, status))
            } else {
                Ok(value)
            }
        };
        (api.exit_scope)();
        (api.release_isolate)();
        outcome
    }
}

fn with_engine<T>(f: impl FnOnce(&Engine) -> T) -> Option<T> {
    let guard = lock(&ENGINE);
    guard.as_ref().map(f)
}

/// One argument decoded from a MethodBind ptrcall.
enum ThunkArg {
    Int(i64),
    Double(f64),
    Bool(bool),
    Str(String),
    Ref(SteelRef),
}

/// One result decoded from a Dart extension method call.
enum ThunkValue {
    Null,
    Int(i64),
    Double(f64),
    Bool(bool),
    Str(String),
    Ref(Option<SteelRef>),
}

/// Returns the signature of the extension method registered under `method_id`.
fn extension_method_signature(method_id: usize) -> Option<(Vec<u8>, u8)> {
    let classes = lock(&EXTENSION_CLASSES);
    for class in classes.iter() {
        let Some(offset) = method_id.checked_sub(class.method_base) else {
            continue;
        };
        let Some(method) = class.methods.get(offset) else {
            continue;
        };
        return Some((method.arg_types.clone(), method.return_type));
    }
    None
}

/// Invokes `steelInvokeExtension` and decodes its result.
///
/// The Dart isolate is acquired for the whole call, including argument
/// construction and result decoding, because handles are only valid inside the
/// scope.
fn invoke_extension(
    engine: &Engine,
    method_id: usize,
    instance_index: u32,
    args: &[ThunkArg],
    return_type: u8,
) -> Result<ThunkValue, String> {
    let api = &engine.api;
    let function = c_string("steelInvokeExtension")?;
    let library = c_string(EXTENSION_LIBRARY)?;
    let length = i64::try_from(args.len()).unwrap_or(0);
    let method = i64::try_from(method_id).unwrap_or(-1);
    let instance = i64::from(instance_index);
    // SAFETY: the isolate is acquired before any VM call and released after the
    // scope is exited, matching the embedder API contract.
    unsafe {
        (api.acquire_isolate)(engine.isolate);
        (api.enter_scope)();
        let outcome = (|| -> Result<ThunkValue, String> {
            let handle_name = (api.new_string)(function.as_ptr());
            if (api.is_error)(handle_name) {
                return Err(error_text(api, handle_name));
            }
            let target = {
                let uri = (api.new_string)(library.as_ptr());
                let handle = (api.lookup_library)(uri);
                if (api.is_error)(handle) || (api.is_null)(handle) {
                    (api.root_library)()
                } else {
                    handle
                }
            };
            let list = (api.new_list)(length);
            if list.is_null() {
                return Err("Dart_NewList returned null".to_owned());
            }
            if (api.is_error)(list) {
                return Err(error_text(api, list));
            }
            for (index, argument) in args.iter().enumerate() {
                let handle = match argument {
                    ThunkArg::Int(value) => (api.new_integer)(*value),
                    ThunkArg::Double(value) => (api.new_double)(*value),
                    ThunkArg::Bool(value) => (api.new_boolean)(*value),
                    ThunkArg::Str(value) => {
                        let text = c_string(value)?;
                        (api.new_string)(text.as_ptr())
                    }
                    ThunkArg::Ref(reference) => {
                        let pair = (api.new_list)(3);
                        if pair.is_null() {
                            return Err("Dart_NewList returned null".to_owned());
                        }
                        if (api.is_error)(pair) {
                            return Err(error_text(api, pair));
                        }
                        let fields = [reference.kind, reference.index, reference.generation];
                        for (offset, field) in fields.into_iter().enumerate() {
                            let field_handle = (api.new_integer)(i64::from(field));
                            let position = i64::try_from(offset).unwrap_or(0);
                            let status = (api.list_set_at)(pair, position, field_handle);
                            if (api.is_error)(status) {
                                return Err(error_text(api, status));
                            }
                        }
                        pair
                    }
                };
                if handle.is_null() {
                    return Err("Dart handle allocation failed".to_owned());
                }
                if (api.is_error)(handle) {
                    return Err(error_text(api, handle));
                }
                let position = i64::try_from(index).unwrap_or(0);
                let status = (api.list_set_at)(list, position, handle);
                if (api.is_error)(status) {
                    return Err(error_text(api, status));
                }
            }
            let mut arguments = vec![(api.new_integer)(method), (api.new_integer)(instance), list];
            let result = (api.invoke)(target, handle_name, 3, arguments.as_mut_ptr());
            if result.is_null() || (api.is_null)(result) {
                // A null reference result means `SteelRef::NONE`; other types
                // have no null representation on the ABI.
                return if return_type == STEEL_VALUE_REF {
                    Ok(ThunkValue::Ref(None))
                } else {
                    Ok(ThunkValue::Null)
                };
            }
            if (api.is_error)(result) {
                return Err(error_text(api, result));
            }
            match return_type {
                STEEL_VALUE_I32 | STEEL_VALUE_U32 => {
                    let mut value = 0_i64;
                    let status = (api.integer_to_i64)(result, &mut value);
                    if (api.is_error)(status) {
                        return Err(error_text(api, status));
                    }
                    Ok(ThunkValue::Int(value))
                }
                STEEL_VALUE_F32 | STEEL_VALUE_F64 => {
                    let mut value = 0.0_f64;
                    let status = (api.double_value)(result, &mut value);
                    if (api.is_error)(status) {
                        return Err(error_text(api, status));
                    }
                    Ok(ThunkValue::Double(value))
                }
                STEEL_VALUE_BOOLEAN => {
                    let mut value = false;
                    let status = (api.boolean_value)(result, &mut value);
                    if (api.is_error)(status) {
                        return Err(error_text(api, status));
                    }
                    Ok(ThunkValue::Bool(value))
                }
                STEEL_VALUE_STRING => {
                    let mut pointer: *mut u8 = std::ptr::null_mut();
                    let mut length = 0_i64;
                    let status = (api.string_to_utf8)(result, &mut pointer, &mut length);
                    if (api.is_error)(status) {
                        return Err(error_text(api, status));
                    }
                    if length <= 0 || pointer.is_null() {
                        return Ok(ThunkValue::Str(String::new()));
                    }
                    let length = usize::try_from(length)
                        .map_err(|_| "string result is too long".to_owned())?;
                    // The VM returned a buffer of `length` bytes that stays
                    // valid until `Dart_ExitScope`.
                    let bytes = std::slice::from_raw_parts(pointer, length);
                    Ok(ThunkValue::Str(String::from_utf8_lossy(bytes).into_owned()))
                }
                STEEL_VALUE_REF => {
                    let mut fields = [0_u32; 3];
                    for (offset, field) in fields.iter_mut().enumerate() {
                        let position = i64::try_from(offset).unwrap_or(0);
                        let handle = (api.list_get_at)(result, position);
                        if handle.is_null() {
                            return Err("reference result is null".to_owned());
                        }
                        if (api.is_error)(handle) {
                            return Err(error_text(api, handle));
                        }
                        let mut value = 0_i64;
                        let status = (api.integer_to_i64)(handle, &mut value);
                        if (api.is_error)(status) {
                            return Err(error_text(api, status));
                        }
                        *field = u32::try_from(value)
                            .map_err(|_| format!("invalid reference component {value}"))?;
                    }
                    Ok(ThunkValue::Ref(Some(SteelRef::owned(
                        fields[0], fields[1], fields[2],
                    ))))
                }
                STEEL_VALUE_VOID => Ok(ThunkValue::Null),
                other => Err(format!("unsupported extension return type code {other}")),
            }
        })();
        (api.exit_scope)();
        (api.release_isolate)();
        outcome
    }
}

/// Writes `value` into the receiver's return slot.
///
/// # Safety
///
/// `ret` must point at a writable slot matching `return_type`, or be null for
/// `void` methods.
unsafe fn write_result(return_type: u8, value: ThunkValue, ret: *mut c_void) {
    match (return_type, value) {
        (STEEL_VALUE_VOID, _) => {}
        (STEEL_VALUE_I32, ThunkValue::Int(value)) if !ret.is_null() => {
            // SAFETY: the host supplies an `i32` return slot.
            unsafe { *ret.cast::<i32>() = value as i32 };
        }
        (STEEL_VALUE_U32, ThunkValue::Int(value)) if !ret.is_null() => {
            // SAFETY: the host supplies a `u32` return slot.
            unsafe { *ret.cast::<u32>() = value as u32 };
        }
        (STEEL_VALUE_F32, ThunkValue::Double(value)) if !ret.is_null() => {
            // SAFETY: the host supplies an `f32` return slot.
            unsafe { *ret.cast::<f32>() = value as f32 };
        }
        (STEEL_VALUE_F64, ThunkValue::Double(value)) if !ret.is_null() => {
            // SAFETY: the host supplies an `f64` return slot.
            unsafe { *ret.cast::<f64>() = value };
        }
        (STEEL_VALUE_BOOLEAN, ThunkValue::Bool(value)) if !ret.is_null() => {
            // SAFETY: the host supplies a `bool` return slot, one byte large.
            unsafe { *ret.cast::<u8>() = u8::from(value) };
        }
        (STEEL_VALUE_STRING, ThunkValue::Str(value)) if !ret.is_null() => {
            let view = RETURN_STRING.with(|cell| {
                let mut buffer = cell.borrow_mut();
                buffer.clear();
                buffer.extend_from_slice(value.as_bytes());
                SteelStrView {
                    ptr: buffer.as_ptr(),
                    len: buffer.len(),
                }
            });
            // SAFETY: the host supplies a `SteelStrView` return slot. The view
            // points at thread-local storage that outlives this call.
            unsafe { *ret.cast::<SteelStrView>() = view };
        }
        (STEEL_VALUE_REF, ThunkValue::Ref(value)) if !ret.is_null() => {
            let reference = value.unwrap_or(SteelRef::NONE);
            // SAFETY: the host supplies a `SteelRef` return slot.
            unsafe { *ret.cast::<SteelRef>() = reference };
        }
        (code, _) => {
            log(
                1,
                &format!("extension method returned no value for type code {code}"),
            );
        }
    }
}

unsafe extern "C" fn initialize(level: u32, _user_data: *mut c_void) -> i32 {
    let Some(config) = CONFIG.get() else {
        return STEEL_ERR_INTERNAL;
    };
    if let Some(error) = &config.error {
        log(0, error);
        return STEEL_ERR_INTERNAL;
    }
    match level {
        STEEL_LEVEL_CONTENT => {
            let engine = match start_engine(config) {
                Ok(engine) => engine,
                Err(error) => {
                    log(0, &error);
                    return STEEL_ERR_INTERNAL;
                }
            };
            let interface_address = LOCAL_INTERFACE
                .get()
                .map_or(0, |interface| std::ptr::from_ref(interface) as i64);
            // The runtime package provides this; older/minimal kernels may not.
            let _ = invoke_library(
                &engine,
                "package:steel_extension/src/plugin/runtime.dart",
                "steelSetInterface",
                &[Arg::Int(interface_address)],
            );
            let entry = config.entry.clone();
            ACTIVE_ENGINE.with(|active| active.set(&engine));
            let entry_result = invoke(&engine, &entry, &[]);
            ACTIVE_ENGINE.with(|active| active.set(std::ptr::null()));
            if let Err(error) = entry_result {
                log(0, &format!("Dart entrypoint `{entry}` failed: {error}"));
                return STEEL_ERR_INTERNAL;
            }
            *lock(&ENGINE) = Some(engine);
            STEEL_OK
        }
        STEEL_LEVEL_BEHAVIOR => {
            let Some(interface) = interface() else {
                return STEEL_ERR_INTERNAL;
            };
            let registrations = std::mem::take(&mut *lock(&REGISTRATIONS));
            for registration in registrations {
                match registration {
                    Registration::Command {
                        id,
                        name,
                        description,
                        permission,
                    } => {
                        let Some(register) = interface.register_command else {
                            return STEEL_ERR_UNSUPPORTED;
                        };
                        let user_data = Box::into_raw(Box::new(id)).cast::<c_void>();
                        // SAFETY: strings live for the call; `user_data` lives
                        // for the process lifetime.
                        let status = unsafe {
                            register(
                                SteelStrView::from_str(&name),
                                SteelStrView::from_str(&description),
                                SteelStrView::from_str(&permission),
                                Some(command_callback),
                                user_data,
                            )
                        };
                        if status != STEEL_OK {
                            log(1, &format!("failed to register /{name}: {status}"));
                        }
                    }
                    Registration::Event(kind) => {
                        let Some(register) = interface.register_event_listener else {
                            return STEEL_ERR_UNSUPPORTED;
                        };
                        // SAFETY: the callback lives for the process lifetime.
                        let status = unsafe {
                            register(kind, 0, Some(event_callback), std::ptr::null_mut())
                        };
                        if status != STEEL_OK {
                            log(1, &format!("failed to register event {kind}: {status}"));
                        }
                    }
                }
            }
            register_extension_classes(interface);
            STEEL_OK
        }
        STEEL_LEVEL_RUNTIME => STEEL_OK,
        _ => STEEL_ERR_INVALID_ARGUMENT,
    }
}

/// Registers every Dart-declared extension class with the host `ClassDB`.
///
/// Registration is retried on every behavior phase so a host that loaded the
/// extension before the classes were queued still sees them. The host copies
/// the descriptors, so the temporary method array only has to live for the
/// call.
fn register_extension_classes(interface: &'static SteelInterface) {
    let mut classes = lock(&EXTENSION_CLASSES);
    if classes.is_empty() {
        return;
    }
    let Some(register) = interface.register_extension_class else {
        log(1, "host does not support extension classes");
        return;
    };
    for (class_id, class) in classes.iter_mut().enumerate() {
        if class.registered {
            continue;
        }
        let methods: Vec<SteelExtensionMethodDesc> = class
            .methods
            .iter()
            .enumerate()
            .map(|(offset, method)| SteelExtensionMethodDesc {
                method_name: SteelStrView::from_str(&method.name),
                hash: method.hash,
                thunk: Some(extension_method_thunk),
                user_data: (class.method_base + offset) as *mut c_void,
            })
            .collect();
        let desc = SteelExtensionClassDesc {
            struct_size: std::mem::size_of::<SteelExtensionClassDesc>() as u32,
            class_name: SteelStrView::from_str(&class.name),
            inherits: SteelStrView::from_str(&class.inherits),
            construct: Some(extension_construct_thunk),
            construct_user_data: class_id as *mut c_void,
            methods: methods.as_ptr(),
            method_count: methods.len(),
        };
        // SAFETY: every string and method entry lives in `EXTENSION_CLASSES`,
        // which is never mutated or cleared, so the descriptors stay valid for
        // the duration of the call.
        let status = unsafe { register(&desc) };
        if status == STEEL_OK {
            class.registered = true;
        } else {
            log(
                1,
                &format!(
                    "failed to register extension class {}: {status}",
                    class.name
                ),
            );
        }
    }
}

/// Construct thunk: calls Dart `steelConstructExtension` and returns its id.
unsafe extern "C" fn extension_construct_thunk(
    user_data: *mut c_void,
    _args: *const *const c_void,
    ret: *mut SteelRef,
) -> i32 {
    let class_id = user_data as usize;
    if ret.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    if class_id >= lock(&EXTENSION_CLASSES).len() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    let result = with_engine(|engine| {
        invoke_library(
            engine,
            EXTENSION_LIBRARY,
            "steelConstructExtension",
            &[Arg::Int(i64::try_from(class_id).unwrap_or(-1))],
        )
    });
    match result {
        Some(Ok(id)) if (1..=i64::from(u32::MAX)).contains(&id) => {
            // SAFETY: the host supplies a writable `SteelRef` return slot.
            unsafe {
                *ret = SteelRef::owned(STEEL_REF_EXTENSION, id as u32, 1);
            }
            STEEL_OK
        }
        Some(Ok(id)) => {
            log(
                1,
                &format!("steelConstructExtension({class_id}) returned {id}"),
            );
            STEEL_ERR_INTERNAL
        }
        Some(Err(error)) => {
            log(
                1,
                &format!("steelConstructExtension({class_id}) failed: {error}"),
            );
            STEEL_ERR_INTERNAL
        }
        None => STEEL_ERR_INTERNAL,
    }
}

/// Method thunk: decodes ptrcall arguments, calls Dart, and writes the result.
unsafe extern "C" fn extension_method_thunk(
    user_data: *mut c_void,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
) {
    let method_id = user_data as usize;
    let Some((arg_types, return_type)) = extension_method_signature(method_id) else {
        log(1, &format!("unknown extension method id {method_id}"));
        return;
    };
    let mut values = Vec::with_capacity(arg_types.len());
    for (index, code) in arg_types.iter().enumerate() {
        if args.is_null() {
            log(1, "extension method called without an argument array");
            return;
        }
        // SAFETY: the host supplies one pointer per declared argument.
        let slot = unsafe { args.add(index) };
        // SAFETY: the slot holds the argument pointer for this position.
        let pointer = unsafe { *slot };
        if pointer.is_null() {
            log(1, "extension method called with a null argument");
            return;
        }
        let value = match *code {
            // SAFETY: the host points at an `i32` for this argument position.
            STEEL_VALUE_I32 => ThunkArg::Int(i64::from(unsafe { *pointer.cast::<i32>() })),
            // SAFETY: the host points at a `u32` for this argument position.
            STEEL_VALUE_U32 => ThunkArg::Int(i64::from(unsafe { *pointer.cast::<u32>() })),
            // SAFETY: the host points at an `f32` for this argument position.
            STEEL_VALUE_F32 => ThunkArg::Double(f64::from(unsafe { *pointer.cast::<f32>() })),
            // SAFETY: the host points at an `f64` for this argument position.
            STEEL_VALUE_F64 => ThunkArg::Double(unsafe { *pointer.cast::<f64>() }),
            // SAFETY: booleans travel as one byte over the ABI.
            STEEL_VALUE_BOOLEAN => ThunkArg::Bool(unsafe { *pointer.cast::<u8>() } != 0),
            STEEL_VALUE_STRING => {
                // SAFETY: the host supplies a `SteelStrView`.
                let view = unsafe { *pointer.cast::<SteelStrView>() };
                // SAFETY: the view is valid for the call.
                let Some(text) = (unsafe { view.as_str() }) else {
                    log(1, "extension method received an invalid UTF-8 string");
                    return;
                };
                ThunkArg::Str(text.to_owned())
            }
            STEEL_VALUE_REF => {
                // SAFETY: the host supplies a `SteelRef`.
                ThunkArg::Ref(unsafe { *pointer.cast::<SteelRef>() })
            }
            other => {
                log(
                    1,
                    &format!("unsupported extension argument type code {other}"),
                );
                return;
            }
        };
        values.push(value);
    }
    let result = with_engine(|engine| {
        invoke_extension(engine, method_id, instance.index, &values, return_type)
    });
    match result {
        Some(Ok(value)) => {
            // SAFETY: the host supplies a return slot matching the method
            // signature registered in the class descriptor.
            unsafe { write_result(return_type, value, ret) };
        }
        Some(Err(error)) => {
            log(1, &format!("extension method {method_id} failed: {error}"));
        }
        None => {
            log(1, "extension method called before the Dart engine started");
        }
    }
}

unsafe extern "C" fn command_callback(
    user_data: *mut c_void,
    player: SteelEntityRef,
    has_player: u8,
    command: SteelStrView,
    args: SteelStrView,
) -> i32 {
    let id = if user_data.is_null() {
        0
    } else {
        // SAFETY: the shim leaked a boxed command id.
        unsafe { *(user_data.cast::<u64>()) }
    };
    // SAFETY: the host supplies valid views for the call.
    let command = unsafe { command.as_str() }.unwrap_or_default();
    // SAFETY: see above.
    let args = unsafe { args.as_str() }.unwrap_or_default();
    let Ok(id) = i64::try_from(id) else {
        return 0;
    };
    let result = with_engine(|engine| {
        invoke_library(
            engine,
            "package:steel_extension/src/plugin/runtime.dart",
            "steelDispatchCommand",
            &[
                Arg::Int(id),
                Arg::Int(i64::from(player.world.0)),
                Arg::Int(i64::from(player.entity)),
                Arg::Int(i64::from(has_player)),
                Arg::Str(command),
                Arg::Str(args),
            ],
        )
    });
    match result {
        Some(Ok(value)) => i32::try_from(value).unwrap_or(1),
        Some(Err(error)) => {
            log(1, &format!("Dart command callback failed: {error}"));
            0
        }
        None => 0,
    }
}

unsafe extern "C" fn event_callback(event: *mut SteelEvent, _user_data: *mut c_void) -> u32 {
    if event.is_null() {
        return 0;
    }
    // SAFETY: the host supplies a valid event for the call.
    let event = unsafe { &*event };
    // SAFETY: the text view is valid for the call.
    let text = unsafe { event.text.as_str() }.unwrap_or_default();
    let result = with_engine(|engine| {
        invoke_library(
            engine,
            "package:steel_extension/src/plugin/runtime.dart",
            "steelDispatchEvent",
            &[
                Arg::Int(i64::from(event.kind)),
                Arg::Int(i64::from(event.world.0)),
                Arg::Int(i64::from(event.x)),
                Arg::Int(i64::from(event.y)),
                Arg::Int(i64::from(event.z)),
                Arg::Int(i64::from(event.block_state.0)),
                Arg::Int(i64::from(event.entity.entity)),
                Arg::Int(i64::from(event.entity.entity >= 0)),
                Arg::Str(text),
            ],
        )
    });
    match result {
        Some(Ok(value)) => u32::try_from(value).unwrap_or(0),
        Some(Err(error)) => {
            log(1, &format!("Dart event callback failed: {error}"));
            0
        }
        None => 0,
    }
}

unsafe extern "C" fn deinitialize(_level: u32, _user_data: *mut c_void) {
    // The Dart engine is process-wide and may serve other embedded plugins, so
    // it is intentionally left running.
}

/// Returns the host interface pointer as an integer for `dart:ffi`.
#[unsafe(no_mangle)]
pub extern "C" fn steel_shim_interface() -> usize {
    INTERFACE.load(Ordering::Acquire) as usize
}

/// Queues a command registration; `initialize(BEHAVIOR)` forwards it to the host.
#[unsafe(no_mangle)]
pub extern "C" fn steel_shim_register_command(
    id: u64,
    name: *const c_char,
    description: *const c_char,
    permission: *const c_char,
) -> i32 {
    if name.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller supplies NUL-terminated strings.
    let name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: see above.
    let description = if description.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(description) }
            .to_string_lossy()
            .into_owned()
    };
    // SAFETY: see above.
    let permission = if permission.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(permission) }
            .to_string_lossy()
            .into_owned()
    };
    lock(&REGISTRATIONS).push(Registration::Command {
        id,
        name,
        description,
        permission,
    });
    STEEL_OK
}

/// Queues an event subscription; `initialize(BEHAVIOR)` forwards it to the host.
#[unsafe(no_mangle)]
pub extern "C" fn steel_shim_register_event(kind: u32) -> i32 {
    lock(&REGISTRATIONS).push(Registration::Event(kind));
    STEEL_OK
}

/// Queues an extension class declaration; `initialize(BEHAVIOR)` registers it
/// with the host `ClassDB`.
///
/// `arg_counts` holds one parameter count per method and `arg_types` the
/// flattened [`SteelValueType`](STEEL_VALUE_I32) codes in method order.
///
/// # Safety
///
/// Every pointer must reference an array of the length implied by
/// `method_count` and `arg_counts`, and the strings must be NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn steel_shim_register_class(
    name: *const c_char,
    inherits: *const c_char,
    method_count: usize,
    method_names: *const *const c_char,
    method_hashes: *const u32,
    arg_counts: *const u8,
    arg_types: *const u8,
    return_types: *const u8,
) -> i32 {
    if name.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller supplies NUL-terminated strings.
    let name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    let inherits = if inherits.is_null() {
        "Object".to_owned()
    } else {
        // SAFETY: see above.
        unsafe { CStr::from_ptr(inherits) }
            .to_string_lossy()
            .into_owned()
    };
    let mut methods = Vec::with_capacity(method_count);
    if method_count > 0 {
        if method_names.is_null()
            || method_hashes.is_null()
            || arg_counts.is_null()
            || return_types.is_null()
        {
            return STEEL_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: the caller supplies arrays of `method_count` entries.
        let names = unsafe { std::slice::from_raw_parts(method_names, method_count) };
        // SAFETY: see above.
        let hashes = unsafe { std::slice::from_raw_parts(method_hashes, method_count) };
        // SAFETY: see above.
        let counts = unsafe { std::slice::from_raw_parts(arg_counts, method_count) };
        // SAFETY: see above.
        let returns = unsafe { std::slice::from_raw_parts(return_types, method_count) };
        let total = counts
            .iter()
            .try_fold(0_usize, |sum, count| sum.checked_add(usize::from(*count)));
        let Some(total) = total else {
            return STEEL_ERR_INVALID_ARGUMENT;
        };
        let codes: &[u8] = if total == 0 {
            &[]
        } else {
            if arg_types.is_null() {
                return STEEL_ERR_INVALID_ARGUMENT;
            }
            // SAFETY: the caller supplies `total` flattened type codes.
            unsafe { std::slice::from_raw_parts(arg_types, total) }
        };
        let mut offset = 0_usize;
        for index in 0..method_count {
            if names[index].is_null() {
                return STEEL_ERR_INVALID_ARGUMENT;
            }
            let return_type = returns[index];
            if return_type > STEEL_VALUE_VOID {
                return STEEL_ERR_INVALID_ARGUMENT;
            }
            let count = usize::from(counts[index]);
            let Some(end) = offset.checked_add(count) else {
                return STEEL_ERR_INVALID_ARGUMENT;
            };
            let Some(types) = codes.get(offset..end) else {
                return STEEL_ERR_INVALID_ARGUMENT;
            };
            if types.iter().any(|code| *code > STEEL_VALUE_REF) {
                return STEEL_ERR_INVALID_ARGUMENT;
            }
            // SAFETY: the caller supplies NUL-terminated method names.
            let method_name = unsafe { CStr::from_ptr(names[index]) }
                .to_string_lossy()
                .into_owned();
            methods.push(ExtensionMethod {
                name: method_name,
                hash: hashes[index],
                arg_types: types.to_vec(),
                return_type,
            });
            offset = end;
        }
    }
    let method_base = NEXT_EXTENSION_METHOD_ID.fetch_add(method_count, Ordering::AcqRel);
    lock(&EXTENSION_CLASSES).push(ExtensionClass {
        name,
        inherits,
        method_base,
        methods,
        registered: false,
    });
    if let Some(interface) = interface() {
        register_extension_classes(interface);
    }
    STEEL_OK
}

/// Writes a line through the host logger.
#[unsafe(no_mangle)]
pub extern "C" fn steel_shim_log(level: u32, message: *const c_char) -> i32 {
    if message.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the caller supplies a NUL-terminated string.
    let message = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    log(level, &message);
    STEEL_OK
}

/// Leaves this shim's isolate around a host call that may enter another
/// plugin's isolate, then restores it before returning to Dart.
fn with_isolate_suspended<R>(f: impl FnOnce() -> R) -> R {
    let active = ACTIVE_ENGINE.with(std::cell::Cell::get);
    let suspended = if active.is_null() {
        with_engine(|engine| {
            // SAFETY: called from Dart on this shim's isolate.
            let current = unsafe { (engine.api.current_isolate)() };
            (current == engine.isolate).then_some((
                engine.api.exit_scope,
                engine.api.release_isolate,
                engine.api.acquire_isolate,
                engine.api.enter_scope,
                engine.isolate,
            ))
        })
    } else {
        // SAFETY: the thread-local only points at the engine for the duration
        // of its entrypoint call.
        let engine = unsafe { &*active };
        // SAFETY: called from Dart on this shim's isolate.
        let current = unsafe { (engine.api.current_isolate)() };
        Some((current == engine.isolate).then_some((
            engine.api.exit_scope,
            engine.api.release_isolate,
            engine.api.acquire_isolate,
            engine.api.enter_scope,
            engine.isolate,
        )))
    }
    .flatten();
    let Some((exit_scope, release, acquire, enter_scope, isolate)) = suspended else {
        return f();
    };
    // SAFETY: the current isolate is this shim's isolate.
    unsafe {
        exit_scope();
        release();
    }
    let result = f();
    // SAFETY: restore the isolate that was current before the host call.
    unsafe {
        acquire(isolate);
        enter_scope();
    }
    result
}

unsafe extern "C" fn isolated_method_bind_ptrcall(
    bind: *const SteelMethodBind,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
) -> i32 {
    let Some(interface) = interface() else {
        return STEEL_ERR_INTERNAL;
    };
    let Some(call) = interface.method_bind_ptrcall else {
        return STEEL_ERR_UNSUPPORTED;
    };
    let is_extension = interface
        .classdb_method_bind_is_extension
        // SAFETY: the bind came from this host's ClassDB.
        .is_some_and(|is_extension| unsafe { is_extension(bind) });
    if !is_extension {
        // SAFETY: the host supplied a valid bind and the caller owns the slots.
        return unsafe { call(bind, instance, args, ret) };
    }
    with_isolate_suspended(|| {
        // SAFETY: the host supplied a valid bind and the caller owns the slots.
        unsafe { call(bind, instance, args, ret) }
    })
}

unsafe extern "C" fn isolated_classdb_construct(
    class_name: SteelStrView,
    args: *const *const c_void,
    ret: *mut SteelRef,
) -> i32 {
    with_isolate_suspended(|| {
        let Some(interface) = interface() else {
            return STEEL_ERR_INTERNAL;
        };
        let Some(call) = interface.classdb_construct else {
            return STEEL_ERR_UNSUPPORTED;
        };
        // SAFETY: the host supplied valid arguments and a return slot.
        unsafe { call(class_name, args, ret) }
    })
}

/// Extension entry point.
///
/// # Safety
///
/// `interface` must point to a valid host interface table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn steel_extension_entry(
    interface: *const SteelInterface,
) -> *const SteelExtension {
    INTERFACE.store(interface.cast_mut(), Ordering::Release);
    if !interface.is_null() {
        // SAFETY: the host supplies a valid table.
        let mut local = unsafe { *interface };
        local.method_bind_ptrcall = Some(isolated_method_bind_ptrcall);
        local.classdb_construct = Some(isolated_classdb_construct);
        let _ = LOCAL_INTERFACE.set(local);
    }
    let config = if interface.is_null() {
        Config::broken("null host interface")
    } else {
        // SAFETY: the host supplies a valid table.
        config_from_interface(unsafe { &*interface })
    };
    let id: &'static str = Box::leak(config.id.clone().into_boxed_str());
    let version: &'static str = Box::leak(config.version.clone().into_boxed_str());
    let _ = CONFIG.set(config);
    MANIFEST.get_or_init(|| SteelExtension {
        struct_size: std::mem::size_of::<SteelExtension>() as u32,
        api_version: STEEL_ABI_VERSION,
        id: SteelStrView::from_str(id),
        version: SteelStrView::from_str(version),
        api_min: 1,
        api_max: 1,
        initialize: Some(initialize),
        deinitialize: Some(deinitialize),
        user_data: std::ptr::null_mut(),
    })
}
