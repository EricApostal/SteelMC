//! Optional end-to-end check for the Dart bridge.
//!
//! Requires:
//!   STEEL_DART_SDK=/path/to/dart
//!   STEEL_DART_EXAMPLE=/abs/path/to/examples/dart_essentials
//!   STEEL_DART_BRIDGE=/abs/path/to/libsteel_dart_bridge.so (defaults to target/debug)
//!
//! Run `dart pub get` in the example directory first. The test is skipped when
//! the environment variables are absent so normal Rust test runs stay hermetic.

use std::path::{Path, PathBuf};

use steel_core::behavior::init_behaviors_with;
use steel_plugin_host::{ExtensionManager, registered_command_names};
use steel_registry::init_vanilla_registry_with;

fn bridge_path() -> PathBuf {
    std::env::var("STEEL_DART_BRIDGE").map_or_else(
        |_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("workspace root")
                .join("target/debug/libsteel_dart_bridge.so")
        },
        PathBuf::from,
    )
}

#[test]
fn dart_essentials_registers_commands() {
    let Ok(dart) = std::env::var("STEEL_DART_SDK") else {
        eprintln!("skipping: STEEL_DART_SDK is not set");
        return;
    };
    let Ok(example) = std::env::var("STEEL_DART_EXAMPLE") else {
        eprintln!("skipping: STEEL_DART_EXAMPLE is not set");
        return;
    };
    let bridge = bridge_path();
    assert!(
        bridge.exists(),
        "missing Dart bridge at {}; run `cargo build -p steel-dart-bridge`",
        bridge.display()
    );
    assert!(
        Path::new(&example).join("bin/essentials.dart").exists(),
        "missing example entrypoint under {example}"
    );

    let directory = std::env::temp_dir().join(format!("steel-dart-test-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create temp plugin directory");
    let library = directory.join("essentials.so");
    std::fs::copy(&bridge, &library).expect("copy bridge");
    let sidecar = format!(
        r#"{{"id":"essentials","version":"0.1.0","entry":"bin/essentials.dart","cwd":{},"dart":{}}}"#,
        serde_json::to_string(&example).expect("example path"),
        serde_json::to_string(&dart).expect("dart path"),
    );
    std::fs::write(format!("{}.json", library.display()), sidecar).expect("write sidecar");

    let manager = ExtensionManager::new();
    manager.load(&library).expect("load Dart bridge");
    assert!(init_vanilla_registry_with(|registry| {
        manager.content_phase(registry);
    }));
    init_behaviors_with(|blocks, items| {
        manager.behavior_phase(blocks, items);
    });

    let names = registered_command_names();
    for expected in [
        "spawn", "sethome", "home", "delhome", "homes", "tpa", "tpaccept", "tpdeny", "msg", "who",
        "heal",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "expected /{expected} in {names:?}"
        );
    }

    manager.shutdown();
}
