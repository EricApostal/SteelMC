//! End-to-end check for the native extension path: build a `cdylib`, load it,
//! register content, attach behaviours, and exercise the host interface.

use std::path::PathBuf;
use std::process::Command;

use steel_core::behavior::init_behaviors_with;
use steel_core::block_entity::init_block_entities_with;
use steel_core::entity::init_entities_with;
use steel_core::events::{self, EventKind};
use steel_plugin_host::{ExtensionManager, attached_block_identifiers};
use steel_registry::{RegistryExt, init_vanilla_registry_with};
use steel_utils::Identifier;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate lives under the workspace root")
        .to_path_buf()
}

fn build_extension() -> PathBuf {
    let root = workspace_root();
    let target_dir = root.join("target").join("plugin-abi-test");
    let status = Command::new(env!("CARGO"))
        .args([
            "build",
            "-p",
            "steel-plugin-test",
            "--target-dir",
            target_dir.to_str().expect("utf-8 target dir"),
            "--quiet",
        ])
        .current_dir(&root)
        .status()
        .expect("failed to invoke cargo for the test extension");
    assert!(status.success(), "test extension failed to build");

    let name = if cfg!(target_os = "windows") {
        "steel_plugin_test.dll"
    } else if cfg!(target_os = "macos") {
        "libsteel_plugin_test.dylib"
    } else {
        "libsteel_plugin_test.so"
    };
    let path = target_dir.join("debug").join(name);
    assert!(
        path.exists(),
        "missing test extension at {}",
        path.display()
    );
    path
}

#[test]
fn extension_registers_content_behaviour_and_listener() {
    let library_path = build_extension();
    let manager = ExtensionManager::new();
    manager
        .load(&library_path)
        .expect("test extension should load");

    // Content phase: registry is mutable and not yet frozen.
    assert!(init_vanilla_registry_with(|registry| {
        manager.content_phase(registry);
    }));

    let key = Identifier::new("steel_test", "ruby_block");
    let block = steel_registry::REGISTRY
        .blocks
        .by_key(&key)
        .expect("extension block should be registered");
    assert_eq!(
        block.state_count(),
        2,
        "one bool property yields two states"
    );
    assert_eq!(block.properties.len(), 1);
    assert_eq!(block.properties[0].get_name(), "lit");
    assert!(steel_registry::REGISTRY.items.by_key(&key).is_some());

    // Exercise the host interface from inside the extension.
    // SAFETY: the extension was loaded above and the symbol is stable.
    unsafe {
        let library = libloading::Library::new(&library_path).expect("reopen extension");
        let registered: libloading::Symbol<'_, extern "C" fn() -> i32> = library
            .get(b"steel_test_registered\0")
            .expect("steel_test_registered symbol");
        assert_eq!(registered(), 1);
        let roundtrip: libloading::Symbol<'_, extern "C" fn() -> i32> = library
            .get(b"steel_test_state_roundtrip\0")
            .expect("steel_test_state_roundtrip symbol");
        assert_eq!(roundtrip(), 1);
    }

    // Behaviour phase: attach the extension hook table and register listeners.
    init_behaviors_with(|blocks, items| {
        manager.behavior_phase(blocks, items);
    });
    init_block_entities_with(|registry| manager.block_entity_phase(registry));
    init_entities_with(|registry| manager.entity_phase(registry));
    manager.runtime_phase();

    assert!(
        attached_block_identifiers().contains(&key),
        "extension block behaviour should be attached"
    );
    assert_eq!(events::listener_count(EventKind::BlockPlace), 1);
}
