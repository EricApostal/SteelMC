//! Generic bridge that lets a Dart process implement a Steel extension.
//!
//! The bridge is a `cdylib` exporting `steel_extension_entry`. A sidecar file
//! next to the library (`<library>.json`) names the Dart entrypoint:
//!
//! ```json
//! {"id":"essentials","version":"0.1.0","entry":"bin/essentials.dart","cwd":".","dart":"dart"}
//! ```
//!
//! The bridge spawns the Dart process and exchanges newline-delimited JSON over
//! stdin/stdout. Commands and events are forwarded to Dart; Dart may call host
//! services (teleport, messages, world queries) through nested `host_call`
//! messages while a callback is pending.

use std::ffi::c_void;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, OnceLock};

use serde_json::{Value, json};
use steel_abi::*;

static INTERFACE: AtomicPtr<SteelInterface> = AtomicPtr::new(std::ptr::null_mut());
static CONFIG: OnceLock<Config> = OnceLock::new();
static MANIFEST: OnceLock<SteelExtension> = OnceLock::new();
static PROCESS: LazyLock<Mutex<Option<Process>>> = LazyLock::new(|| Mutex::new(None));

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
struct Config {
    id: String,
    version: String,
    entry: PathBuf,
    cwd: PathBuf,
    dart: String,
    error: Option<String>,
}

impl Config {
    fn broken(error: impl Into<String>) -> Self {
        Self {
            id: "steel_dart_bridge".to_owned(),
            version: "0.0.0".to_owned(),
            entry: PathBuf::new(),
            cwd: PathBuf::new(),
            dart: "dart".to_owned(),
            error: Some(error.into()),
        }
    }
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

fn interface() -> Option<&'static SteelInterface> {
    let pointer = INTERFACE.load(Ordering::Acquire);
    // SAFETY: the host keeps the interface alive for the process lifetime.
    (!pointer.is_null()).then(|| unsafe { &*pointer })
}

fn log(level: u32, message: &str) {
    if let Some(interface) = interface()
        && let Some(log) = interface.log
    {
        // SAFETY: the view points at a live string for the duration of the call.
        unsafe { log(level, SteelStrView::from_str(message)) };
    } else {
        eprintln!("steel-dart-bridge: {message}");
    }
}

fn read_sidecar(path: &Path) -> Result<Config, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("failed to read sidecar {}: {error}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("invalid sidecar {}: {error}", path.display()))?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "sidecar is missing `id`".to_owned())?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("0.1.0");
    let entry = value
        .get("entry")
        .and_then(Value::as_str)
        .ok_or_else(|| "sidecar is missing `entry`".to_owned())?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .map_or_else(|| base.to_path_buf(), |cwd| base.join(cwd));
    Ok(Config {
        id: id.to_owned(),
        version: version.to_owned(),
        entry: cwd.join(entry),
        cwd,
        dart: value
            .get("dart")
            .and_then(Value::as_str)
            .unwrap_or("dart")
            .to_owned(),
        error: None,
    })
}

fn load_config(interface: &SteelInterface) -> Result<Config, String> {
    if let Some(path_function) = interface.extension_path {
        // SAFETY: the host returns a view valid for this call.
        let view = unsafe { path_function() };
        // SAFETY: see above.
        if let Some(path) = unsafe { view.as_str() }
            && !path.is_empty()
        {
            let sidecar = PathBuf::from(format!("{path}.json"));
            return read_sidecar(&sidecar);
        }
    }
    if let Ok(path) = std::env::var("STEEL_DART_PLUGIN_CONFIG") {
        return read_sidecar(Path::new(&path));
    }
    Err("extension path is unavailable; place <library>.json next to the bridge".to_owned())
}

fn spawn(config: &Config) -> Result<Process, String> {
    let mut command = if config.entry.extension().and_then(std::ffi::OsStr::to_str) == Some("dart")
    {
        let mut command = Command::new(&config.dart);
        command.arg("run").arg(&config.entry);
        command
    } else {
        Command::new(&config.entry)
    };
    let mut child = command
        .current_dir(&config.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("failed to spawn `{}`: {error}", config.entry.display()))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "failed to open Dart stdin".to_owned())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to open Dart stdout".to_owned())?;
    Ok(Process {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        next_id: 1,
    })
}

fn write_message(process: &mut Process, message: &Value) -> Result<(), String> {
    let line = serde_json::to_string(message).map_err(|error| error.to_string())?;
    process
        .stdin
        .write_all(line.as_bytes())
        .and_then(|()| process.stdin.write_all(b"\n"))
        .and_then(|()| process.stdin.flush())
        .map_err(|error| format!("failed to write to Dart: {error}"))
}

fn read_message(process: &mut Process) -> Result<Value, String> {
    let mut line = String::new();
    let read = process
        .stdout
        .read_line(&mut line)
        .map_err(|error| format!("failed to read from Dart: {error}"))?;
    if read == 0 {
        return Err("Dart plugin exited".to_owned());
    }
    serde_json::from_str(line.trim()).map_err(|error| format!("invalid Dart message: {error}"))
}

fn entity_from(params: &Value) -> Option<SteelEntityRef> {
    let world = u32::try_from(params.get("world")?.as_u64()?).ok()?;
    let entity = i32::try_from(params.get("entity")?.as_i64()?).ok()?;
    Some(SteelEntityRef {
        world: SteelWorldRef(world),
        entity,
    })
}

fn player_json(entity: SteelEntityRef) -> Value {
    let mut object = json!({
        "entity": entity.entity,
        "world": entity.world.0,
        "name": "",
        "uuid": "",
    });
    if let Some(interface) = interface() {
        if let Some(get_name) = interface.entity_get_name {
            let mut view = SteelStrView::EMPTY;
            // SAFETY: the host writes a borrowed view for this call.
            if unsafe { get_name(entity, &mut view) } == STEEL_OK
                // SAFETY: the view is valid for this call.
                && let Some(name) = unsafe { view.as_str() }
            {
                object["name"] = Value::String(name.to_owned());
            }
        }
        if let Some(get_uuid) = interface.entity_get_uuid {
            let mut view = SteelStrView::EMPTY;
            // SAFETY: the host writes a borrowed view for this call.
            if unsafe { get_uuid(entity, &mut view) } == STEEL_OK
                // SAFETY: the view is valid for this call.
                && let Some(uuid) = unsafe { view.as_str() }
            {
                object["uuid"] = Value::String(uuid.to_owned());
            }
        }
    }
    object
}

fn host_call(method: &str, params: &Value) -> Result<Value, String> {
    let Some(interface) = interface() else {
        return Err("host interface is unavailable".to_owned());
    };
    match method {
        "log" => {
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            log(2, message);
            Ok(Value::Null)
        }
        "player.name" | "player.uuid" => {
            let entity = entity_from(params).ok_or_else(|| "invalid entity".to_owned())?;
            let mut view = SteelStrView::EMPTY;
            let status = if method == "player.name" {
                interface
                    .entity_get_name
                    .map_or(STEEL_ERR_UNSUPPORTED, |function| {
                        // SAFETY: the host writes a borrowed view for this call.
                        unsafe { function(entity, &mut view) }
                    })
            } else {
                interface
                    .entity_get_uuid
                    .map_or(STEEL_ERR_UNSUPPORTED, |function| {
                        // SAFETY: the host writes a borrowed view for this call.
                        unsafe { function(entity, &mut view) }
                    })
            };
            if status != STEEL_OK {
                return Err(format!("player lookup failed with status {status}"));
            }
            // SAFETY: the view is valid for this call.
            let value = unsafe { view.as_str() }.unwrap_or_default().to_owned();
            Ok(Value::String(value))
        }
        "player.position" => {
            let entity = entity_from(params).ok_or_else(|| "invalid entity".to_owned())?;
            let mut position = SteelVec3::default();
            let function = interface
                .entity_get_position
                .ok_or_else(|| "entity_get_position is unavailable".to_owned())?;
            // SAFETY: the host writes a position for this call.
            let status = unsafe { function(entity, &mut position) };
            if status != STEEL_OK {
                return Err(format!("position lookup failed with status {status}"));
            }
            Ok(json!({"x": position.x, "y": position.y, "z": position.z}))
        }
        "player.teleport" => {
            let entity = entity_from(params).ok_or_else(|| "invalid entity".to_owned())?;
            let position = SteelVec3 {
                x: params.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                y: params.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                z: params.get("z").and_then(Value::as_f64).unwrap_or(0.0),
            };
            let yaw = params.get("yaw").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let pitch = params.get("pitch").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let function = interface
                .entity_teleport
                .ok_or_else(|| "entity_teleport is unavailable".to_owned())?;
            // SAFETY: the host performs the teleport.
            let status = unsafe { function(entity, position, yaw, pitch) };
            if status != STEEL_OK {
                return Err(format!("teleport failed with status {status}"));
            }
            Ok(Value::Null)
        }
        "player.send_message" => {
            let entity = entity_from(params).ok_or_else(|| "invalid entity".to_owned())?;
            let message = params
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let function = interface
                .player_send_message
                .ok_or_else(|| "player_send_message is unavailable".to_owned())?;
            // SAFETY: the view points at a live string for the call.
            let status = unsafe { function(entity, SteelStrView::from_str(message)) };
            if status != STEEL_OK {
                return Err(format!("message failed with status {status}"));
            }
            Ok(Value::Null)
        }
        "player.health" => {
            let entity = entity_from(params).ok_or_else(|| "invalid entity".to_owned())?;
            let mut health = 0.0f32;
            let function = interface
                .entity_get_health
                .ok_or_else(|| "entity_get_health is unavailable".to_owned())?;
            // SAFETY: the host writes health for this call.
            let status = unsafe { function(entity, &mut health) };
            if status != STEEL_OK {
                return Err(format!("health lookup failed with status {status}"));
            }
            Ok(json!(health))
        }
        "player.set_health" => {
            let entity = entity_from(params).ok_or_else(|| "invalid entity".to_owned())?;
            let health = params.get("health").and_then(Value::as_f64).unwrap_or(20.0) as f32;
            let function = interface
                .entity_set_health
                .ok_or_else(|| "entity_set_health is unavailable".to_owned())?;
            // SAFETY: the host sets health.
            let status = unsafe { function(entity, health) };
            if status != STEEL_OK {
                return Err(format!("set health failed with status {status}"));
            }
            Ok(Value::Null)
        }
        "world.spawn" => {
            let world = u32::try_from(
                params
                    .get("world")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "missing world".to_owned())?,
            )
            .map_err(|_| "invalid world".to_owned())?;
            let mut position = SteelVec3::default();
            let function = interface
                .world_get_spawn
                .ok_or_else(|| "world_get_spawn is unavailable".to_owned())?;
            // SAFETY: the host writes a position for this call.
            let status = unsafe { function(SteelWorldRef(world), &mut position) };
            if status != STEEL_OK {
                return Err(format!("spawn lookup failed with status {status}"));
            }
            Ok(json!({"x": position.x, "y": position.y, "z": position.z}))
        }
        "world.players" => {
            let world = u32::try_from(
                params
                    .get("world")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "missing world".to_owned())?,
            )
            .map_err(|_| "invalid world".to_owned())?;
            let count_function = interface
                .world_player_count
                .ok_or_else(|| "world_player_count is unavailable".to_owned())?;
            // SAFETY: the host returns a snapshot count.
            let count = unsafe { count_function(SteelWorldRef(world)) };
            let mut players = Vec::with_capacity(count as usize);
            let at = interface
                .world_player_at
                .ok_or_else(|| "world_player_at is unavailable".to_owned())?;
            for index in 0..count {
                let mut player = SteelOptEntityRef::default();
                // SAFETY: the host writes an optional entity for this call.
                if unsafe { at(SteelWorldRef(world), index, &mut player) } == STEEL_OK
                    && player.present != 0
                {
                    players.push(player_json(player.value));
                }
            }
            Ok(Value::Array(players))
        }
        "server.find_player" => {
            let world = u32::try_from(
                params
                    .get("world")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| "missing world".to_owned())?,
            )
            .map_err(|_| "invalid world".to_owned())?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing name".to_owned())?;
            let find = interface
                .world_find_player
                .ok_or_else(|| "world_find_player is unavailable".to_owned())?;
            let mut player = SteelOptEntityRef::default();
            // SAFETY: the host writes an optional entity for this call.
            let status = unsafe {
                find(
                    SteelWorldRef(world),
                    SteelStrView::from_str(name),
                    &mut player,
                )
            };
            if status != STEEL_OK {
                return Err(format!("find player failed with status {status}"));
            }
            Ok(if player.present == 0 {
                Value::Null
            } else {
                player_json(player.value)
            })
        }
        other => Err(format!("unknown host method `{other}`")),
    }
}

fn exchange(message: Value, collect: bool) -> Result<(Value, Vec<Value>), String> {
    let mut guard = lock(&PROCESS);
    let Some(process) = guard.as_mut() else {
        return Err("Dart plugin process is not running".to_owned());
    };
    let id = process.next_id;
    process.next_id = process.next_id.wrapping_add(1);
    let message = if message.get("id").is_some() {
        message
    } else {
        let mut message = message;
        message["id"] = json!(id);
        message
    };
    write_message(process, &message)?;
    let expected = message["id"].as_u64().unwrap_or(id);
    let mut registrations = Vec::new();
    loop {
        let reply = read_message(process)?;
        match reply.get("t").and_then(Value::as_str) {
            Some("ready") => return Ok((Value::Null, registrations)),
            Some("response") => {
                if reply.get("id").and_then(Value::as_u64) == Some(expected) {
                    return Ok((reply, registrations));
                }
            }
            Some("host_call") => {
                let host_id = reply.get("id").cloned().unwrap_or(Value::Null);
                let result = reply
                    .get("method")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "host_call without method".to_owned())
                    .and_then(|method| {
                        host_call(method, reply.get("params").unwrap_or(&Value::Null))
                    });
                let response = match result {
                    Ok(result) => json!({"t":"host_result","id":host_id,"ok":true,"result":result}),
                    Err(error) => json!({"t":"host_error","id":host_id,"ok":false,"error":error}),
                };
                write_message(process, &response)?;
            }
            Some("register_command" | "register_event") if collect => registrations.push(reply),
            Some("log") => {
                let message = reply
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                log(2, message);
            }
            Some("shutdown") => return Err("Dart plugin requested shutdown".to_owned()),
            Some(other) => log(1, &format!("unexpected Dart message `{other}`")),
            None => log(1, "Dart message without a `t` field"),
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
    if lock(&PROCESS).is_none() {
        match spawn(config) {
            Ok(process) => *lock(&PROCESS) = Some(process),
            Err(error) => {
                log(0, &error);
                return STEEL_ERR_INTERNAL;
            }
        }
    }
    if level != 1 {
        let mut guard = lock(&PROCESS);
        let Some(process) = guard.as_mut() else {
            return STEEL_ERR_INTERNAL;
        };
        if let Err(error) = write_message(process, &json!({"t":"init","level":level})) {
            log(
                0,
                &format!("failed to notify Dart of level {level}: {error}"),
            );
            return STEEL_ERR_INTERNAL;
        }
        return STEEL_OK;
    }
    let (_, registrations) = match exchange(json!({"t":"init","level":level}), true) {
        Ok(result) => result,
        Err(error) => {
            log(0, &format!("Dart plugin initialization failed: {error}"));
            return STEEL_ERR_INTERNAL;
        }
    };
    if level == 1 {
        let Some(interface) = interface() else {
            return STEEL_ERR_INTERNAL;
        };
        for registration in registrations {
            match registration.get("t").and_then(Value::as_str) {
                Some("register_command") => {
                    let name = registration
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let description = registration
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let permission = registration
                        .get("permission")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let user_data = Box::into_raw(Box::new(name.to_owned())).cast::<c_void>();
                    let status =
                        interface
                            .register_command
                            .map_or(STEEL_ERR_UNSUPPORTED, |function| {
                                // SAFETY: the strings live for the call and the boxed name
                                // lives for the process lifetime.
                                unsafe {
                                    function(
                                        SteelStrView::from_str(name),
                                        SteelStrView::from_str(description),
                                        SteelStrView::from_str(permission),
                                        Some(on_command),
                                        user_data,
                                    )
                                }
                            });
                    if status != STEEL_OK {
                        log(1, &format!("failed to register command `{name}`: {status}"));
                    }
                }
                Some("register_event") => {
                    let name = registration
                        .get("event")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let Some(kind) = event_kind(name) else {
                        log(1, &format!("unknown event `{name}`"));
                        continue;
                    };
                    let status = interface.register_event_listener.map_or(
                        STEEL_ERR_UNSUPPORTED,
                        |function| {
                            // SAFETY: the callback lives for the process lifetime.
                            unsafe { function(kind, 0, Some(on_event), std::ptr::null_mut()) }
                        },
                    );
                    if status != STEEL_OK {
                        log(1, &format!("failed to register event `{name}`: {status}"));
                    }
                }
                _ => {}
            }
        }
    }
    STEEL_OK
}

fn event_kind(name: &str) -> Option<u32> {
    match name {
        "block_break" => Some(STEEL_EVENT_BLOCK_BREAK),
        "block_place" => Some(STEEL_EVENT_BLOCK_PLACE),
        "player_join" => Some(STEEL_EVENT_PLAYER_JOIN),
        "player_leave" => Some(STEEL_EVENT_PLAYER_LEAVE),
        "chat" => Some(STEEL_EVENT_CHAT),
        _ => None,
    }
}

unsafe extern "C" fn on_command(
    user_data: *mut c_void,
    player: SteelEntityRef,
    has_player: u8,
    command: SteelStrView,
    args: SteelStrView,
) -> i32 {
    let name = if user_data.is_null() {
        String::new()
    } else {
        // SAFETY: the bridge leaked a boxed command name for this callback.
        unsafe { (*(user_data.cast::<String>())).clone() }
    };
    // SAFETY: the host passes valid views for this call.
    let command = unsafe { command.as_str() }.unwrap_or(&name);
    // SAFETY: see above.
    let args = unsafe { args.as_str() }.unwrap_or_default();
    let player = (has_player != 0).then(|| player_json(player));
    let message = json!({
        "t":"command",
        "player":player,
        "command":command,
        "args":args,
    });
    match exchange(message, false) {
        Ok((reply, _)) => reply.get("result").and_then(Value::as_i64).unwrap_or(1) as i32,
        Err(error) => {
            log(1, &format!("command `{command}` failed: {error}"));
            0
        }
    }
}

unsafe extern "C" fn on_event(event: *mut SteelEvent, _user_data: *mut c_void) -> u32 {
    if event.is_null() {
        return 0;
    }
    // SAFETY: the host passes a valid event for this call.
    let event = unsafe { &*event };
    let name = match event.kind {
        STEEL_EVENT_BLOCK_BREAK => "block_break",
        STEEL_EVENT_BLOCK_PLACE => "block_place",
        STEEL_EVENT_PLAYER_JOIN => "player_join",
        STEEL_EVENT_PLAYER_LEAVE => "player_leave",
        STEEL_EVENT_CHAT => "chat",
        _ => return 0,
    };
    let player = if event.entity.entity >= 0 {
        Some(player_json(event.entity))
    } else {
        None
    };
    let text = if event.kind == STEEL_EVENT_CHAT {
        // SAFETY: the host passes a view valid for this call.
        unsafe { event.text.as_str() }.unwrap_or_default()
    } else {
        ""
    };
    let message = json!({
        "t":"event",
        "event":name,
        "player":player,
        "world":event.world.0,
        "x":event.x,
        "y":event.y,
        "z":event.z,
        "block_state":event.block_state.0,
        "text":text,
    });
    match exchange(message, false) {
        Ok((reply, _)) => u32::from(
            reply
                .get("cancel")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        Err(error) => {
            log(1, &format!("event `{name}` failed: {error}"));
            0
        }
    }
}

unsafe extern "C" fn deinitialize(_level: u32, _user_data: *mut c_void) {
    let mut guard = lock(&PROCESS);
    if let Some(mut process) = guard.take() {
        let _ = write_message(&mut process, &json!({"t":"shutdown"}));
        let _ = process.child.kill();
        let _ = process.child.wait();
    }
}

/// Extension entry point. Reads the sidecar next to the loaded library.
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
        match load_config(unsafe { &*interface }) {
            Ok(config) => config,
            Err(error) => Config::broken(error),
        }
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
