//! Code generator for the Steel extension ABI.
//!
//! Parses `steel-abi/src/lib.rs` with `syn` and renders:
//!
//! * `steel-abi/steel_api.json`
//! * `steel-abi/include/steel_extension.h`
//! * `clients/dart/lib/src/bindings.g.dart`
//! * `clients/dart/lib/src/steel/ffi_api.dart`
//! * `steel-api/steel_api.json`
//! * `clients/dart/lib/src/steel/generated.dart`

mod c;
mod dart;
mod dart_api;
mod ffi_api;
mod generate;
mod model;
mod parser;

pub use c::emit_header as render_header;
pub use dart::emit_bindings as render_dart;
pub use dart_api::{DART_API_PATH, emit_dart_api};
pub use ffi_api::{FFI_API_PATH, emit_ffi_api};
pub use generate::{
    Artifact, DART_PATH, HEADER_PATH, JSON_PATH, STEEL_API_JSON_PATH, check_artifacts, generate,
    render_artifacts, write_artifacts,
};
pub use model::Api;
pub use parser::parse_source;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::model::ConstantDef;

    const SNIPPET: &str = r#"
        pub const STEEL_ABI_VERSION: u32 = 1;
        pub const STEEL_FLAG: u32 = 1 << 3;

        #[repr(C)]
        pub struct SteelStrView {
            pub ptr: *const u8,
            pub len: usize,
        }

        #[repr(C)]
        pub struct SteelInterface {
            pub api_version: u32,
            pub log: Option<SteelLogFn>,
        }

        pub type SteelLogFn = unsafe extern "C" fn(level: u32) -> u8;

        #[unsafe(no_mangle)]
        pub extern "C" fn steel_abi_test_add(left: i32, right: i32) -> i32 {
            left + right
        }
    "#;

    #[test]
    fn parses_inline_abi_snippet() {
        let api = parse_source(SNIPPET).expect("inline ABI snippet should parse");

        assert_eq!(api.api_version, 1);
        assert_eq!(api.host_interface, "SteelInterface");
        assert_eq!(
            api.constants,
            vec![
                ConstantDef {
                    name: "STEEL_ABI_VERSION".to_owned(),
                    value: 1,
                },
                ConstantDef {
                    name: "STEEL_FLAG".to_owned(),
                    value: 8,
                },
            ]
        );

        let view = api
            .structs
            .iter()
            .find(|definition| definition.name == "SteelStrView")
            .expect("SteelStrView should be parsed");
        assert_eq!(view.fields[0].name, "ptr");
        assert_eq!(view.fields[0].rust_type, "*const u8");
        assert_eq!(view.fields[1].rust_type, "usize");

        let log = api
            .function_types
            .iter()
            .find(|function| function.name == "SteelLogFn")
            .expect("SteelLogFn should be parsed");
        assert_eq!(log.args, vec!["u32"]);
        assert_eq!(log.return_type, "u8");

        assert_eq!(api.exports.len(), 1);
        assert_eq!(api.exports[0].name, "steel_abi_test_add");
        assert_eq!(api.exports[0].arg_names, vec!["left", "right"]);
    }

    fn real_source() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("steel-plugin-gen lives in the workspace")
            .join("steel-abi/src/lib.rs");
        fs::read_to_string(path).expect("steel-abi/src/lib.rs should be readable")
    }

    #[test]
    fn parses_the_real_abi() {
        let api = parse_source(&real_source()).expect("steel-abi should parse");

        assert_eq!(api.api_version, 1);
        assert!(api.structs.iter().any(|s| s.name == "SteelInterface"));
        assert!(api.structs.iter().any(|s| s.name == "SteelBlockHooks"));
        assert!(
            api.constants
                .iter()
                .any(|constant| constant.name == "STEEL_ABI_VERSION" && constant.value == 1)
        );
        assert_eq!(
            api.hook_tables,
            vec![
                "SteelBlockHooks",
                "SteelItemHooks",
                "SteelEntityHooks",
                "SteelBlockEntityHooks",
            ]
        );

        let scheduled = api
            .function_types
            .iter()
            .find(|function| function.name == "SteelBlockScheduledTickFn")
            .expect("SteelBlockScheduledTickFn should be parsed");
        assert_eq!(
            scheduled.alias_of.as_deref(),
            Some("SteelBlockRandomTickFn")
        );

        let log = api
            .function_types
            .iter()
            .find(|function| function.name == "SteelLogFn")
            .expect("SteelLogFn should be parsed");
        assert_eq!(log.args, vec!["u32", "SteelStrView"]);
        assert_eq!(log.return_type, "()");
    }

    #[test]
    fn renders_deterministic_artifacts() {
        let api = parse_source(&real_source()).expect("steel-abi should parse");
        let first = render_artifacts(&api).expect("artifacts should render");
        let second = render_artifacts(&api).expect("artifacts should render again");
        assert_eq!(first.len(), 4);
        for (left, right) in first.iter().zip(second.iter()) {
            assert_eq!(left.relative_path, right.relative_path);
            assert_eq!(left.contents, right.contents);
        }

        let json = &first[0].contents;
        assert!(json.ends_with('\n'));
        assert!(json.contains("\"host_interface\": \"SteelInterface\""));
        assert!(json.contains("\"return\""));

        let header = &first[1].contents;
        assert!(header.contains("#define STEEL_API_VERSION 1"));
        assert!(header.contains("typedef struct SteelInterface SteelInterface;"));
        assert!(header.contains("typedef uint32_t (*SteelApiVersionFn)(void);"));

        let dart = &first[2].contents;
        assert!(dart.contains("final class SteelStrView extends Struct"));
        assert!(dart.contains(
            "typedef SteelExtensionEntryNative = Pointer<SteelExtension> Function(Pointer<SteelInterface>);"
        ));
        assert!(dart.contains("factory SteelBindings.open(DynamicLibrary lib)"));
        assert!(dart.contains("steelApiVersion"));

        let ffi_api = &first[3].contents;
        assert!(ffi_api.contains("classdbGetMethodBind"));
        assert!(ffi_api.contains("methodBindPtrcall"));
    }

    const API_JSON: &str = r#"
{
  "api_version": 1,
  "classes": [
    {"name": "Object", "inherits": null, "handle": "none", "receiver": "()", "methods": []},
    {"name": "Entity", "inherits": "Object", "handle": "entity", "receiver": "SteelEntityRef", "methods": [
      {"name": "teleport", "hash": 7, "static": false,
       "args": [{"name": "position", "type": "SteelVec3"}, {"name": "yaw", "type": "f32"}],
       "return": "bool"}
    ]},
    {"name": "Player", "inherits": "Entity", "handle": "entity", "receiver": "SteelEntityRef", "methods": [
      {"name": "name", "hash": 8, "static": false, "args": [], "return": "SteelStrView"}
    ]},
    {"name": "Server", "inherits": "Object", "handle": "server", "receiver": "()", "methods": [
      {"name": "find_player", "hash": 9, "static": true,
       "args": [{"name": "world_id", "type": "u32"}, {"name": "name", "type": "SteelStrView"}],
       "return": "SteelRef", "returns_class": "Player"}
    ]},
    {"name": "Block", "inherits": "Object", "handle": "block", "receiver": "BlockId", "methods": [
      {"name": "default_state", "hash": 10, "static": false, "args": [], "return": "u32"}
    ]}
  ]
}
"#;

    #[test]
    fn renders_the_dart_host_api() {
        let dart = emit_dart_api(API_JSON).expect("API JSON should render");
        assert!(dart.contains("abstract class SteelObject"));
        assert!(dart.contains("class Entity extends SteelObject"));
        assert!(dart.contains("class Player extends Entity"));
        assert!(dart.contains("class Server extends SteelObject"));
        assert!(dart.contains("const int steelHostApiVersion = 1;"));
        assert!(
            dart.contains(
                "static Player? findPlayer(SteelBinding binding, int worldId, String name)"
            )
        );
        assert!(dart.contains("SteelReturnKind.ref"));
        assert!(dart.contains("Player.fromRef(binding, returned)"));
        assert!(dart.contains("SteelArg.vec3(position)"));
        assert!(dart.contains("SteelArg.f32(yaw)"));
        assert!(!dart.contains("asFunction"));
        assert!(!dart.contains("NativeFunction"));
    }
}
