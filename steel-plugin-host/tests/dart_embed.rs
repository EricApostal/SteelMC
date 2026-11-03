//! Optional integration check: the real host loads the embedded Dart runtime.
//!
//! Requires `STEEL_DART_VM_LIB` and `STEEL_DART_ESSENTIALS_KERNEL`; skips
//! otherwise. Build `libsteel_dart_embed.so` first.

use std::path::PathBuf;

use steel_core::behavior::init_behaviors_with;
use steel_core::block_entity::init_block_entities_with;
use steel_core::entity::init_entities_with;
use steel_plugin_host::{ExtensionManager, registered_command_names};
use steel_registry::init_vanilla_registry_with;

#[test]
fn embedded_essentials_registers_commands_with_real_host() {
    let Ok(vm) = std::env::var("STEEL_DART_VM_LIB") else {
        eprintln!("skipping: STEEL_DART_VM_LIB is not set");
        return;
    };
    let Ok(kernel) = std::env::var("STEEL_DART_ESSENTIALS_KERNEL") else {
        eprintln!("skipping: STEEL_DART_ESSENTIALS_KERNEL is not set");
        return;
    };
    let shim = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("target/debug/libsteel_dart_embed.so");
    assert!(
        shim.exists(),
        "missing Dart runtime shim at {}; run `cargo build -p steel-dart-embed`",
        shim.display()
    );

    let directory = std::env::temp_dir().join(format!("steel-dart-host-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("temp dir");
    let library = directory.join("essentials.so");
    std::fs::copy(&shim, &library).expect("copy shim");
    let sidecar = format!(
        r#"{{"id":"essentials","version":"0.1.0","kernel":{},"vm":{},"entry":"main"}}"#,
        serde_json::to_string(&kernel).unwrap(),
        serde_json::to_string(&vm).unwrap(),
    );
    std::fs::write(format!("{}.json", library.display()), sidecar).expect("sidecar");

    let manager = ExtensionManager::new();
    manager.load(&library).expect("load embedded Dart shim");
    assert!(init_vanilla_registry_with(|registry| {
        manager.content_phase(registry);
    }));
    init_behaviors_with(|blocks, items| {
        manager.behavior_phase(blocks, items);
    });
    init_block_entities_with(|registry| manager.block_entity_phase(registry));
    init_entities_with(|registry| manager.entity_phase(registry));
    manager.runtime_phase();

    let commands = registered_command_names();
    for expected in [
        "spawn", "sethome", "home", "delhome", "homes", "tpa", "tpaccept", "tpdeny", "msg", "who",
        "heal",
    ] {
        assert!(
            commands.iter().any(|name| name == expected),
            "expected /{expected} in {commands:?}"
        );
    }
    manager.shutdown();
}
