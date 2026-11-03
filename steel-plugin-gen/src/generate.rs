//! Artifact rendering and filesystem operations.

use std::fs;
use std::path::{Path, PathBuf};

use crate::c::emit_header;
use crate::dart::emit_bindings;
use crate::dart_api::{DART_API_PATH, emit_dart_api};
use crate::ffi_api::{FFI_API_PATH, emit_ffi_api};
use crate::model::Api;
use crate::parser::parse_source;

/// Workspace-relative path of the generated ABI JSON schema.
pub const JSON_PATH: &str = "steel-abi/steel_api.json";
/// Workspace-relative path of the generated C header.
pub const HEADER_PATH: &str = "steel-abi/include/steel_extension.h";
/// Workspace-relative path of the generated Dart FFI bindings.
pub const DART_PATH: &str = "clients/dart/lib/src/bindings.g.dart";
/// Workspace-relative path of the annotated host API model.
pub const STEEL_API_JSON_PATH: &str = "steel-api/steel_api.json";

/// A generated file.
#[derive(Debug, Clone)]
pub struct Artifact {
    /// Workspace-relative output path.
    pub relative_path: &'static str,
    /// Complete file contents.
    pub contents: String,
}

/// Parses the ABI source and renders every generated artifact.
pub fn generate(root: &Path) -> Result<Vec<Artifact>, String> {
    let source_path = root.join("steel-abi/src/lib.rs");
    let source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    let api = parse_source(&source)?;
    let mut artifacts = render_artifacts(&api)?;

    // The annotated host API is the source for the plugin-facing Dart module.
    let api_source = root.join("steel-api/src/api.rs");
    let annotated = steel_api_codegen::scan_file(&api_source)?;
    let steel_api_json = steel_api_codegen::emit_json(&annotated)?;
    artifacts.push(Artifact {
        relative_path: STEEL_API_JSON_PATH,
        contents: steel_api_json.clone(),
    });
    artifacts.push(Artifact {
        relative_path: DART_API_PATH,
        contents: emit_dart_api(&steel_api_json)?,
    });
    Ok(artifacts)
}

/// Renders the ABI artifacts derived from `steel-abi/src/lib.rs`.
pub fn render_artifacts(api: &Api) -> Result<Vec<Artifact>, String> {
    let mut json = serde_json::to_string_pretty(api)
        .map_err(|error| format!("failed to serialize {JSON_PATH}: {error}"))?;
    json.push('\n');

    Ok(vec![
        Artifact {
            relative_path: JSON_PATH,
            contents: json,
        },
        Artifact {
            relative_path: HEADER_PATH,
            contents: emit_header(api),
        },
        Artifact {
            relative_path: DART_PATH,
            contents: emit_bindings(api),
        },
        Artifact {
            relative_path: FFI_API_PATH,
            contents: emit_ffi_api(api),
        },
    ])
}

/// Writes every artifact, creating parent directories as needed.
pub fn write_artifacts(root: &Path, artifacts: &[Artifact]) -> Result<(), String> {
    for artifact in artifacts {
        let path = root.join(artifact.relative_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(&path, &artifact.contents)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    Ok(())
}

/// Returns the artifacts whose on-disk contents differ from the rendered ones.
#[must_use]
pub fn check_artifacts(root: &Path, artifacts: &[Artifact]) -> Vec<PathBuf> {
    artifacts
        .iter()
        .filter_map(|artifact| {
            let path = root.join(artifact.relative_path);
            match fs::read_to_string(&path) {
                Ok(contents) if contents == artifact.contents => None,
                _ => Some(path),
            }
        })
        .collect()
}
