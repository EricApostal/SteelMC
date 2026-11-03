//! In-process embedding tests.
//!
//! Requires `STEEL_DART_TEST_KERNEL` (smoke plugin),
//! `STEEL_DART_ESSENTIALS_KERNEL` (Essentials example),
//! `STEEL_DART_EXTENSION_KERNEL` (minimal extension-class plugin) and/or
//! `STEEL_DART_VM_LIB`. Each test skips when its kernel is absent.
//!
//! `embedded_essentials_plugin` and `embedded_extension_class` exercise the
//! extension-class path end to end: the shim registers `Greeter`, the test host
//! constructs it and calls its methods through MethodBind ptrcall.

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
    run_kernel_with(kernel, vm, expected, |_| {});
}

/// One class registered by the shim, as observed by the test host.
struct RegisteredClass {
    name: String,
    construct: SteelClassConstructFn,
    construct_user_data: *mut c_void,
    methods: Vec<RegisteredMethod>,
}

/// One method descriptor owned by a [`RegisteredClass`].
///
/// The name lives next to the descriptor so the `SteelStrView` inside `bind`
/// keeps pointing at live data for the process lifetime.
struct RegisteredMethod {
    name: String,
    bind: SteelMethodBindDesc,
}

// SAFETY: the test only calls class thunks while holding the test lock, and
// the registrations are immutable after insertion.
unsafe impl Send for RegisteredClass {}

/// Registered classes; boxed so growing the outer list does not move method
/// descriptors referenced by `classdb_get_method_bind`.
#[allow(
    clippy::box_collection,
    reason = "boxed classes keep method descriptor addresses stable"
)]
static CLASSES: LazyLock<Mutex<Vec<Box<RegisteredClass>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

unsafe extern "C" fn register_extension_class(desc: *const SteelExtensionClassDesc) -> i32 {
    if desc.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: the shim supplies a valid descriptor for the call.
    let desc = unsafe { &*desc };
    // SAFETY: the view is valid for the call.
    let Some(name) = (unsafe { desc.class_name.as_str() }) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let name = name.to_owned();
    let class_view = SteelStrView::from_str(&name);
    let mut methods = Vec::new();
    if !desc.methods.is_null() && desc.method_count > 0 {
        // SAFETY: the shim supplies `method_count` descriptors.
        let entries = unsafe { std::slice::from_raw_parts(desc.methods, desc.method_count) };
        for method in entries {
            // SAFETY: the view is valid for the call.
            let Some(method_name) = (unsafe { method.method_name.as_str() }) else {
                return STEEL_ERR_INVALID_ARGUMENT;
            };
            let method_name = method_name.to_owned();
            let bind = SteelMethodBindDesc {
                class_name: class_view,
                method_name: SteelStrView::from_str(&method_name),
                hash: method.hash,
                thunk: method.thunk,
                user_data: method.user_data,
            };
            methods.push(RegisteredMethod {
                name: method_name,
                bind,
            });
        }
    }
    CLASSES.lock().unwrap().push(Box::new(RegisteredClass {
        name,
        construct: desc
            .construct
            .expect("shim must register a construct thunk"),
        construct_user_data: desc.construct_user_data,
        methods,
    }));
    STEEL_OK
}

/// One native ClassDB method provided by the test host.
///
/// The shim forwards extension-class lookups to the host; the host then falls
/// back to its compiled MethodBinds. The test host only needs the method the
/// Essentials `/heal` command calls.
struct NativeMethod {
    class_name: String,
    method_name: String,
    bind: SteelMethodBindDesc,
}

static NATIVE_METHODS: LazyLock<Vec<NativeMethod>> = LazyLock::new(|| {
    let mut methods = Vec::new();
    methods.push(native_method(
        "Entity",
        "set_health",
        3_926_284_238,
        entity_set_health_thunk,
    ));
    methods.push(native_method(
        "Player",
        "send_message",
        1_771_619_083,
        player_send_message_thunk,
    ));
    methods
});

fn native_method(
    class_name: &str,
    method_name: &str,
    hash: u32,
    thunk: SteelMethodThunk,
) -> NativeMethod {
    let class_name = class_name.to_owned();
    let method_name = method_name.to_owned();
    NativeMethod {
        bind: SteelMethodBindDesc {
            class_name: SteelStrView::from_str(&class_name),
            method_name: SteelStrView::from_str(&method_name),
            hash,
            thunk: Some(thunk),
            user_data: std::ptr::null_mut(),
        },
        class_name,
        method_name,
    }
}

unsafe extern "C" fn entity_set_health_thunk(
    _user_data: *mut c_void,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
) {
    if args.is_null() {
        return;
    }
    // SAFETY: the Dart binding passes one `f32` argument pointer.
    let pointer = unsafe { *args };
    if pointer.is_null() {
        return;
    }
    // SAFETY: see above.
    let health = unsafe { *pointer.cast::<f32>() };
    // SAFETY: the mock host function only reads `HEALTH`.
    let status = unsafe {
        entity_set_health(
            SteelEntityRef {
                world: SteelWorldRef(instance.index),
                entity: i32::try_from(instance.generation).unwrap_or(-1),
            },
            health,
        )
    };
    if !ret.is_null() {
        // SAFETY: the Dart binding passes a `bool` return slot.
        unsafe { *ret.cast::<u8>() = u8::from(status == STEEL_OK) };
    }
}

unsafe extern "C" fn player_send_message_thunk(
    _user_data: *mut c_void,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
) {
    if args.is_null() {
        return;
    }
    // SAFETY: the Dart binding passes one `SteelStrView` argument pointer.
    let pointer = unsafe { *args };
    if pointer.is_null() {
        return;
    }
    // SAFETY: the view is valid for the call.
    let view = unsafe { *pointer.cast::<SteelStrView>() };
    // SAFETY: the mock host function only reads the view.
    let status = unsafe {
        player_send_message(
            SteelEntityRef {
                world: SteelWorldRef(instance.index),
                entity: i32::try_from(instance.generation).unwrap_or(-1),
            },
            view,
        )
    };
    if !ret.is_null() {
        // SAFETY: the Dart binding passes a `bool` return slot.
        unsafe { *ret.cast::<u8>() = u8::from(status == STEEL_OK) };
    }
}

unsafe extern "C" fn classdb_get_method_bind(
    class_name: SteelStrView,
    method_name: SteelStrView,
    hash: u32,
) -> *const SteelMethodBind {
    // SAFETY: the caller supplies views valid for the call.
    let Some(class_name) = (unsafe { class_name.as_str() }) else {
        return std::ptr::null();
    };
    // SAFETY: see above.
    let Some(method_name) = (unsafe { method_name.as_str() }) else {
        return std::ptr::null();
    };
    let classes = CLASSES.lock().unwrap();
    for class in classes.iter() {
        if class.name != class_name {
            continue;
        }
        for method in &class.methods {
            if method.name == method_name && (hash == 0 || method.bind.hash == hash) {
                return std::ptr::from_ref(&method.bind).cast::<SteelMethodBind>();
            }
        }
    }
    drop(classes);
    for method in NATIVE_METHODS.iter() {
        if method.class_name == class_name
            && method.method_name == method_name
            && (hash == 0 || method.bind.hash == hash)
        {
            return std::ptr::from_ref(&method.bind).cast::<SteelMethodBind>();
        }
    }
    std::ptr::null()
}

unsafe extern "C" fn classdb_construct(
    class_name: SteelStrView,
    args: *const *const c_void,
    ret: *mut SteelRef,
) -> i32 {
    // SAFETY: the caller supplies a view valid for the call.
    let Some(class_name) = (unsafe { class_name.as_str() }) else {
        return STEEL_ERR_INVALID_ARGUMENT;
    };
    let class = {
        let classes = CLASSES.lock().unwrap();
        classes
            .iter()
            .find(|class| class.name == class_name)
            .map(|class| (class.construct, class.construct_user_data))
    };
    let Some((construct, user_data)) = class else {
        return STEEL_ERR_NOT_FOUND;
    };
    // SAFETY: the shim supplies the construct thunk for this class.
    unsafe { construct(user_data, args, ret) }
}

unsafe extern "C" fn method_bind_ptrcall(
    bind: *const SteelMethodBind,
    instance: SteelRef,
    args: *const *const c_void,
    ret: *mut c_void,
) -> i32 {
    if bind.is_null() {
        return STEEL_ERR_INVALID_ARGUMENT;
    }
    // SAFETY: `bind` points at a descriptor produced by
    // `classdb_get_method_bind`.
    let method = unsafe { &*bind.cast::<SteelMethodBindDesc>() };
    let Some(thunk) = method.thunk else {
        return STEEL_ERR_UNSUPPORTED;
    };
    // SAFETY: the caller supplies argument and return pointers matching the
    // method signature.
    unsafe { thunk(method.user_data, instance, args, ret) };
    STEEL_OK
}

fn run_kernel_with(
    kernel: &str,
    vm: &str,
    expected: &[&str],
    configure: impl FnOnce(&mut SteelInterface),
) {
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
    CLASSES.lock().unwrap().clear();

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
    configure(&mut interface);
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
    run_kernel_with(
        &kernel,
        &vm,
        &[
            "spawn", "sethome", "home", "delhome", "homes", "tpa", "tpaccept", "tpdeny", "msg",
            "who", "heal",
        ],
        |interface| {
            interface.register_extension_class = Some(register_extension_class);
            interface.classdb_get_method_bind = Some(classdb_get_method_bind);
            interface.classdb_construct = Some(classdb_construct);
            interface.method_bind_ptrcall = Some(method_bind_ptrcall);
        },
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
    assert_eq!(
        status,
        1,
        "Dart command failed; host logs: {:?}",
        *LOGS.lock().unwrap()
    );
    assert_eq!(*HEALTH.lock().unwrap(), Some(20.0));
}

#[test]
fn embedded_extension_class() {
    let _guard = TEST_LOCK.lock().unwrap();
    let Ok(kernel) = std::env::var("STEEL_DART_EXTENSION_KERNEL") else {
        eprintln!("skipping: STEEL_DART_EXTENSION_KERNEL is not set");
        return;
    };
    let Ok(vm) = std::env::var("STEEL_DART_VM_LIB") else {
        eprintln!("skipping: STEEL_DART_VM_LIB is not set");
        return;
    };
    run_kernel_with(&kernel, &vm, &[], |interface| {
        interface.register_extension_class = Some(register_extension_class);
        interface.classdb_get_method_bind = Some(classdb_get_method_bind);
        interface.classdb_construct = Some(classdb_construct);
        interface.method_bind_ptrcall = Some(method_bind_ptrcall);
    });
    exercise_greeter();
}

/// Constructs the Dart `Greeter` extension class and calls it through the
/// ClassDB MethodBind path.
fn exercise_greeter() {
    let registration = {
        let classes = CLASSES.lock().unwrap();
        let greeter = classes
            .iter()
            .find(|class| class.name == "Greeter")
            .expect("the shim must register Greeter");
        let names: Vec<&str> = greeter
            .methods
            .iter()
            .map(|method| method.name.as_str())
            .collect();
        let hashes: Vec<u32> = greeter
            .methods
            .iter()
            .map(|method| method.bind.hash)
            .collect();
        assert_eq!(
            names,
            ["greet", "count", "increment", "half", "toggle", "reset"]
        );
        assert_eq!(
            hashes,
            [
                1_768_310_884,
                3_076_273_273,
                2_681_002_766,
                3_262_667_310,
                1_561_724_341,
                2_168_424_486,
            ]
        );
        greeter.methods.len()
    };
    assert_eq!(registration, 6);
    // SAFETY: `classdb_construct` and the ptrcall entry points use the mock
    // host table above; the Dart isolate is still alive.
    unsafe {
        let mut instance = SteelRef::NONE;
        let status = classdb_construct(
            SteelStrView::from_str("Greeter"),
            std::ptr::null(),
            &mut instance,
        );
        assert_eq!(status, STEEL_OK, "Greeter construct failed");
        assert_eq!(instance.kind, STEEL_REF_EXTENSION);
        assert_eq!(instance.generation, 1);

        let greet = classdb_get_method_bind(
            SteelStrView::from_str("Greeter"),
            SteelStrView::from_str("greet"),
            0,
        );
        assert!(!greet.is_null(), "Greeter.greet is not registered");
        let name = SteelStrView::from_str("Steve");
        let args = [std::ptr::from_ref(&name).cast::<c_void>()];
        let mut greeting = SteelStrView::EMPTY;
        let status = method_bind_ptrcall(
            greet,
            instance,
            args.as_ptr(),
            std::ptr::from_mut(&mut greeting).cast::<c_void>(),
        );
        assert_eq!(status, STEEL_OK);
        assert_eq!(
            greeting.as_str(),
            Some("Hello, Steve!"),
            "unexpected Greeter.greet result"
        );

        let increment = classdb_get_method_bind(
            SteelStrView::from_str("Greeter"),
            SteelStrView::from_str("increment"),
            0,
        );
        assert!(!increment.is_null(), "Greeter.increment is not registered");
        let amount = 4_i32;
        let args = [std::ptr::from_ref(&amount).cast::<c_void>()];
        let mut total = 0_i32;
        let status = method_bind_ptrcall(
            increment,
            instance,
            args.as_ptr(),
            std::ptr::from_mut(&mut total).cast::<c_void>(),
        );
        assert_eq!(status, STEEL_OK);
        assert_eq!(total, 5, "Greeter.increment did not use method arguments");

        let count = classdb_get_method_bind(
            SteelStrView::from_str("Greeter"),
            SteelStrView::from_str("count"),
            0,
        );
        assert!(!count.is_null(), "Greeter.count is not registered");
        let mut stored = 0_i32;
        let status = method_bind_ptrcall(
            count,
            instance,
            std::ptr::null(),
            std::ptr::from_mut(&mut stored).cast::<c_void>(),
        );
        assert_eq!(status, STEEL_OK);
        assert_eq!(stored, 5, "Greeter instances do not retain state");

        let half = classdb_get_method_bind(
            SteelStrView::from_str("Greeter"),
            SteelStrView::from_str("half"),
            0,
        );
        assert!(!half.is_null(), "Greeter.half is not registered");
        let value = 7.0_f64;
        let args = [std::ptr::from_ref(&value).cast::<c_void>()];
        let mut halved = 0.0_f64;
        let status = method_bind_ptrcall(
            half,
            instance,
            args.as_ptr(),
            std::ptr::from_mut(&mut halved).cast::<c_void>(),
        );
        assert_eq!(status, STEEL_OK);
        assert_eq!(halved, 3.5, "Greeter.half did not decode a double");

        let toggle = classdb_get_method_bind(
            SteelStrView::from_str("Greeter"),
            SteelStrView::from_str("toggle"),
            0,
        );
        assert!(!toggle.is_null(), "Greeter.toggle is not registered");
        let flag = 1_u8;
        let args = [std::ptr::from_ref(&flag).cast::<c_void>()];
        let mut inverted = 1_u8;
        let status = method_bind_ptrcall(
            toggle,
            instance,
            args.as_ptr(),
            std::ptr::from_mut(&mut inverted).cast::<c_void>(),
        );
        assert_eq!(status, STEEL_OK);
        assert_eq!(inverted, 0, "Greeter.toggle did not decode a bool");

        let reset = classdb_get_method_bind(
            SteelStrView::from_str("Greeter"),
            SteelStrView::from_str("reset"),
            0,
        );
        assert!(!reset.is_null(), "Greeter.reset is not registered");
        let status = method_bind_ptrcall(reset, instance, std::ptr::null(), std::ptr::null_mut());
        assert_eq!(status, STEEL_OK);
        let status = method_bind_ptrcall(
            count,
            instance,
            std::ptr::null(),
            std::ptr::from_mut(&mut stored).cast::<c_void>(),
        );
        assert_eq!(status, STEEL_OK);
        assert_eq!(stored, 0, "Greeter.reset did not clear the counter");
    }
}
