//! In-process Dart VM embedding for Steel extensions.
//!
//! The library loads `libdart_engine_jit_shared.so`, loads a Dart kernel, calls
//! the plugin's top-level `main`, and exposes the Steel C ABI to Dart through
//! `dart:ffi`. Host callbacks acquire the Dart isolate (the engine serializes
//! them), invoke `steelDispatchCommand`/`steelDispatchEvent`, and release it.
//!
//! Sidecar next to the library:
//! ```json
//! {"id":"essentials","version":"0.1.0","kernel":"essentials.dill","vm":"/path/libdart_engine_jit_shared.so"}
//! ```

#![allow(
    missing_docs,
    reason = "the embedding internals are documented by the module comment"
)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};

use steel_abi::*;

type DartHandle = *mut c_void;

static INTERFACE: AtomicPtr<SteelInterface> = AtomicPtr::new(std::ptr::null_mut());
static CONFIG: OnceLock<Config> = OnceLock::new();
static MANIFEST: OnceLock<SteelExtension> = OnceLock::new();
static ENGINE: LazyLock<Mutex<Option<Engine>>> = LazyLock::new(|| Mutex::new(None));
static REGISTRATIONS: LazyLock<Mutex<Vec<Registration>>> = LazyLock::new(|| Mutex::new(Vec::new()));

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
    enter_scope: unsafe extern "C" fn(),
    exit_scope: unsafe extern "C" fn(),
    root_library: unsafe extern "C" fn() -> DartHandle,
    lookup_library: unsafe extern "C" fn(DartHandle) -> DartHandle,
    new_string: unsafe extern "C" fn(*const c_char) -> DartHandle,
    new_integer: unsafe extern "C" fn(i64) -> DartHandle,
    invoke: unsafe extern "C" fn(DartHandle, DartHandle, i32, *mut DartHandle) -> DartHandle,
    integer_to_i64: unsafe extern "C" fn(DartHandle, *mut i64) -> DartHandle,
    is_error: unsafe extern "C" fn(DartHandle) -> bool,
    is_null: unsafe extern "C" fn(DartHandle) -> bool,
    get_error: unsafe extern "C" fn(DartHandle) -> *const c_char,
}

struct Engine {
    api: DartApi,
    _library: libloading::Library,
    isolate: *mut c_void,
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
            invoke: symbol!(
                "Dart_Invoke",
                unsafe extern "C" fn(DartHandle, DartHandle, i32, *mut DartHandle) -> DartHandle
            ),
            integer_to_i64: symbol!(
                "Dart_IntegerToInt64",
                unsafe extern "C" fn(DartHandle, *mut i64) -> DartHandle
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
            let interface_address = INTERFACE.load(Ordering::Acquire) as i64;
            // The runtime package provides this; older/minimal kernels may not.
            let _ = invoke_library(
                &engine,
                "package:steel_extension/src/plugin/runtime.dart",
                "steelSetInterface",
                &[Arg::Int(interface_address)],
            );
            let entry = config.entry.clone();
            if let Err(error) = invoke(&engine, &entry, &[]) {
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
            STEEL_OK
        }
        STEEL_LEVEL_RUNTIME => STEEL_OK,
        _ => STEEL_ERR_INVALID_ARGUMENT,
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
