//! Code generator for the Steel extension ABI.
//!
//! Parses `steel-abi/src/lib.rs` with `syn` and renders:
//!
//! * `steel-abi/steel_api.json`
//! * `steel-abi/include/steel_extension.h`
//! * `clients/dart/lib/src/bindings.g.dart`
//! * `clients/dart/lib/src/plugin/generated_api.dart`

mod c;
mod dart;
mod generate;
mod model;
mod parser;
mod plugin_api;

pub use c::emit_header as render_header;
pub use dart::emit_bindings as render_dart;
pub use generate::{Artifact, check_artifacts, generate, render_artifacts, write_artifacts};
pub use model::Api;
pub use parser::parse_source;
pub use plugin_api::{PLUGIN_API_PATH, emit_plugin_api};

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

        let plugin_api = &first[3].contents;
        assert!(plugin_api.contains("final class Player"));
        assert!(plugin_api.contains("playerSendMessage"));
        assert!(plugin_api.contains("findPlayerInWorld"));
    }
}
