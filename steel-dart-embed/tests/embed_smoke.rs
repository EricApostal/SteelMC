//! In-process embedding tests.
//!
//! Requires `STEEL_DART_TEST_KERNEL` (smoke plugin),
//! `STEEL_DART_ESSENTIALS_KERNEL` (Essentials example), and/or
//! `STEEL_DART_VM_LIB`. Each test skips when its kernel is absent.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use steel_abi::*;

static TEST_LOCK: Mutex<()> = Mutex::new(());
static PATH: LazyLock<Mutex<Option<&'static str>>> = LazyLock::new(|| Mutex::new(None));
struct RegisteredCommand {
    name: String,
    handler: SteelCommandHandlerFn,
    user_data: *mut c_void,
}

// SAFETY: the test only invokes commands while holding the test lock, and the
// shim keeps the user data alive for the process lifetime.
unsafe impl Send for RegisteredCommand {}

static COMMANDS: LazyLock<Mutex<Vec<RegisteredCommand>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static HEALTH: LazyLock<Mutex<Option<f64>>> = LazyLock::new(|| Mutex::new(None));
static EVENTS: LazyLock<Mutex<Vec<u32>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static LOGS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

unsafe extern "C" fn extension_path() -> SteelStrView {
    let path = (*PATH.lock().unwrap()).expect("path set before entry");
    SteelStrView::from_str(path)
}

unsafe extern "C" fn log(_level: u32, message: SteelStrView) {
    // SAFETY: the shim supplies a valid view for the call.
    let message = unsafe { message.as_str() }.unwrap_or_default().to_owned();
    LOGS.lock().unwrap().push(message);
}

unsafe extern "C" fn register_command(
    name: SteelStrView,
    _description: SteelStrView,
    _permission: SteelStrView,
    handler: Option<SteelCommandHandlerFn>,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: the shim supplies a valid view for the call.
    let name = unsafe { name.as_str() }.unwrap_or_default().to_owned();
    let handler = handler.expect("shim must register a command callback");
    COMMANDS.lock().unwrap().push(RegisteredCommand {
        name,
        handler,
        user_data,
    });
    STEEL_OK
}

unsafe extern "C" fn register_event_listener(
    kind: u32,
    _priority: i32,
    callback: Option<SteelEventListenerFn>,
    _user_data: *mut c_void,
) -> i32 {
    assert!(callback.is_some(), "shim must register an event callback");
    EVENTS.lock().unwrap().push(kind);
    STEEL_OK
}

unsafe extern "C" fn entity_get_name(_entity: SteelEntityRef, out: *mut SteelStrView) -> i32 {
    eprintln!("mock entity_get_name");
    assert!(!out.is_null());
    // SAFETY: the shim supplied a writable out-pointer.
    unsafe { *out = SteelStrView::from_str("Steve") };
    STEEL_OK
}

unsafe extern "C" fn entity_get_uuid(_entity: SteelEntityRef, out: *mut SteelStrView) -> i32 {
    eprintln!("mock entity_get_uuid");
    assert!(!out.is_null());
    // SAFETY: the shim supplied a writable out-pointer.
    unsafe { *out = SteelStrView::from_str("uuid-steve") };
    STEEL_OK
}

unsafe extern "C" fn player_send_message(_entity: SteelEntityRef, message: SteelStrView) -> i32 {
    // SAFETY: the shim supplies a valid view for the call.
    let message = unsafe { message.as_str() }.unwrap_or_default().to_owned();
    LOGS.lock().unwrap().push(message);
    STEEL_OK
}

unsafe extern "C" fn entity_get_health(_entity: SteelEntityRef, out: *mut f32) -> i32 {
    eprintln!("mock entity_get_health");
    assert!(!out.is_null());
    // SAFETY: the shim supplied a writable out-pointer.
    unsafe { *out = 20.0 };
    STEEL_OK
}

unsafe extern "C" fn entity_set_health(_entity: SteelEntityRef, health: f32) -> i32 {
    eprintln!("mock entity_set_health {health}");
    *HEALTH.lock().unwrap() = Some(f64::from(health));
    STEEL_OK
}

unsafe extern "C" fn world_get_spawn(_world: SteelWorldRef, out: *mut SteelVec3) -> i32 {
    assert!(!out.is_null());
    // SAFETY: the shim supplied a writable out-pointer.
    unsafe {
        *out = SteelVec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        }
    };
    STEEL_OK
}

fn run_kernel(kernel: &str, vm: &str, expected: &[&str]) {
    let shim = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("target/debug/libsteel_dart_embed.so");
    assert!(shim.exists(), "missing {}", shim.display());

    let directory = std::env::temp_dir().join(format!(
        "steel-dart-embed-{}-{}",
        std::process::id(),
        expected.first().unwrap_or(&"plugin")
    ));
    std::fs::create_dir_all(&directory).expect("temp dir");
    let library_path = directory.join("plugin.so");
    std::fs::copy(&shim, &library_path).expect("copy shim");
    let sidecar = format!(
        r#"{{"id":"embed_test_{}","version":"0.1.0","kernel":{},"vm":{},"entry":"main"}}"#,
        expected.first().unwrap_or(&"plugin"),
        serde_json::to_string(kernel).unwrap(),
        serde_json::to_string(vm).unwrap(),
    );
    std::fs::write(format!("{}.json", library_path.display()), sidecar).expect("sidecar");
    let path: &'static str =
        Box::leak(library_path.to_string_lossy().into_owned().into_boxed_str());
    *PATH.lock().unwrap() = Some(path);
    COMMANDS.lock().unwrap().clear();
    EVENTS.lock().unwrap().clear();
    LOGS.lock().unwrap().clear();
    *HEALTH.lock().unwrap() = None;

    let mut interface = SteelInterface::default();
    interface.extension_path = Some(extension_path);
    interface.log = Some(log);
    interface.register_command = Some(register_command);
    interface.register_event_listener = Some(register_event_listener);
    interface.world_get_spawn = Some(world_get_spawn);
    interface.entity_get_name = Some(entity_get_name);
    interface.entity_get_uuid = Some(entity_get_uuid);
    interface.entity_set_health = Some(entity_set_health);
    interface.entity_get_health = Some(entity_get_health);
    interface.player_send_message = Some(player_send_message);
    let interface: &'static SteelInterface = Box::leak(Box::new(interface));

    // SAFETY: the shim is a trusted test artifact and the interface is valid
    // for the duration of the test.
    unsafe {
        let library = libloading::os::unix::Library::open(
            Some(&library_path),
            libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_GLOBAL,
        )
        .expect("load shim");
        let entry: libloading::os::unix::Symbol<SteelExtensionEntryFn> = library
            .get(b"steel_extension_entry\0")
            .expect("entry symbol");
        let manifest = entry(interface);
        assert!(!manifest.is_null());
        let initialize = (*manifest).initialize.expect("initialize");
        assert_eq!(
            initialize(STEEL_LEVEL_CONTENT, std::ptr::null_mut()),
            STEEL_OK
        );
        assert_eq!(
            initialize(STEEL_LEVEL_BEHAVIOR, std::ptr::null_mut()),
            STEEL_OK
        );
        assert_eq!(
            initialize(STEEL_LEVEL_RUNTIME, std::ptr::null_mut()),
            STEEL_OK
        );
        // Keep the shim and its Dart isolate loaded for command callbacks.
        std::mem::forget(library);
    }

    let commands = COMMANDS.lock().unwrap();
    for name in expected {
        assert!(
            commands.iter().any(|command| command.name == *name),
            "expected /{name} in {:?}",
            commands
                .iter()
                .map(|command| &command.name)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn embedded_smoke_plugin() {
    let _guard = TEST_LOCK.lock().unwrap();
    let Ok(kernel) = std::env::var("STEEL_DART_TEST_KERNEL") else {
        eprintln!("skipping: STEEL_DART_TEST_KERNEL is not set");
        return;
    };
    let Ok(vm) = std::env::var("STEEL_DART_VM_LIB") else {
        eprintln!("skipping: STEEL_DART_VM_LIB is not set");
        return;
    };
    run_kernel(&kernel, &vm, &["smoke"]);
    assert!(
        LOGS.lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("spawn 1.0,2.0,3.0")),
        "expected a native spawn call, got {:?}",
        LOGS.lock().unwrap()
    );
}

#[test]
fn embedded_essentials_plugin() {
    let _guard = TEST_LOCK.lock().unwrap();
    let Ok(kernel) = std::env::var("STEEL_DART_ESSENTIALS_KERNEL") else {
        eprintln!("skipping: STEEL_DART_ESSENTIALS_KERNEL is not set");
        return;
    };
    let Ok(vm) = std::env::var("STEEL_DART_VM_LIB") else {
        eprintln!("skipping: STEEL_DART_VM_LIB is not set");
        return;
    };
    run_kernel(
        &kernel,
        &vm,
        &[
            "spawn", "sethome", "home", "delhome", "homes", "tpa", "tpaccept", "tpdeny", "msg",
            "who", "heal",
        ],
    );

    let heal = {
        let commands = COMMANDS.lock().unwrap();
        let heal = commands
            .iter()
            .find(|command| command.name == "heal")
            .expect("heal command");
        (heal.handler, heal.user_data)
    };
    // SAFETY: the handler and user data belong to the loaded shim and the Dart
    // isolate is still alive.
    let status = unsafe {
        (heal.0)(
            heal.1,
            SteelEntityRef {
                world: SteelWorldRef(0),
                entity: 1,
            },
            1,
            SteelStrView::from_str("heal"),
            SteelStrView::from_str(""),
        )
    };
    assert_eq!(status, 1);
    assert_eq!(*HEALTH.lock().unwrap(), Some(20.0));
}
