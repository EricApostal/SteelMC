//! Optional integration check: two Dart plugins communicate through ClassDB.
//!
//! Plugin A (`Vault`) registers an extension class; plugin B (`economy`)
//! constructs it and calls a method. Both are separate shim copies and
//! kernels, loaded from a nested `plugins/` directory.
//!
//! Requires `STEEL_DART_VM_LIB`, `STEEL_DART_VAULT_KERNEL`, and
//! `STEEL_DART_ECONOMY_KERNEL`; skips otherwise.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use steel_core::behavior::init_behaviors_with;
use steel_core::block_entity::init_block_entities_with;
use steel_core::entity::init_entities_with;
use steel_plugin_host::ExtensionManager;
use steel_registry::init_vanilla_registry_with;

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            LOGS.lock()
                .expect("log lock")
                .push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static LOGGER: Capture = Capture;

fn install_logger() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

fn shim_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("target/debug/libsteel_dart_embed.so")
}

fn write_plugin(
    root: &Path,
    directory: &str,
    id: &str,
    shim: &Path,
    vm: &str,
    kernel: &str,
) -> PathBuf {
    let plugin_dir = root.join(directory);
    std::fs::create_dir_all(&plugin_dir).expect("plugin dir");
    let library = plugin_dir.join("plugin.so");
    std::fs::copy(shim, &library).expect("copy shim");
    let sidecar = format!(
        r#"{{"id":{},"version":"0.1.0","kernel":{},"vm":{},"entry":"main"}}"#,
        serde_json::to_string(id).unwrap(),
        serde_json::to_string(kernel).unwrap(),
        serde_json::to_string(vm).unwrap(),
    );
    let mut file = std::fs::File::create(format!("{}.json", library.display())).expect("sidecar");
    file.write_all(sidecar.as_bytes()).expect("write sidecar");
    library
}

#[test]
fn dart_plugins_call_each_other_through_classdb() {
    let Ok(vm) = std::env::var("STEEL_DART_VM_LIB") else {
        eprintln!("skipping: STEEL_DART_VM_LIB is not set");
        return;
    };
    let Ok(vault_kernel) = std::env::var("STEEL_DART_VAULT_KERNEL") else {
        eprintln!("skipping: STEEL_DART_VAULT_KERNEL is not set");
        return;
    };
    let Ok(economy_kernel) = std::env::var("STEEL_DART_ECONOMY_KERNEL") else {
        eprintln!("skipping: STEEL_DART_ECONOMY_KERNEL is not set");
        return;
    };
    let shim = shim_path();
    assert!(
        shim.exists(),
        "missing Dart runtime shim at {}; run `cargo build -p steel-dart-embed`",
        shim.display()
    );

    install_logger();
    LOGS.lock().expect("log lock").clear();

    let root = std::env::temp_dir().join(format!("steel-dart-interop-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    // Sorted by path: the provider registers `Vault` before `economy` starts.
    write_plugin(&root, "01_vault", "vault", &shim, &vm, &vault_kernel);
    write_plugin(&root, "02_economy", "economy", &shim, &vm, &economy_kernel);

    let manager = ExtensionManager::new();
    let loaded = manager.load_dir(&root).expect("load plugin directory");
    assert_eq!(loaded, 2);

    assert!(init_vanilla_registry_with(|registry| {
        manager.content_phase(registry);
    }));
    init_behaviors_with(|blocks, items| {
        manager.behavior_phase(blocks, items);
    });
    init_block_entities_with(|registry| manager.block_entity_phase(registry));
    init_entities_with(|registry| manager.entity_phase(registry));
    manager.runtime_phase();

    let logs = LOGS.lock().expect("log lock").clone();
    assert!(
        logs.iter().any(|line| line == "vault balance: 15.5"),
        "expected the economy plugin to call Vault; logs: {logs:?}"
    );
    manager.shutdown();
    let _ = std::fs::remove_dir_all(&root);
}
