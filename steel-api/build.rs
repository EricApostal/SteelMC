use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = manifest.join("src/api.rs");
    println!("cargo:rerun-if-changed={}", source.display());
    let model = steel_api_codegen::scan_file(&source).expect("scan steel-api annotations");
    let output = steel_api_codegen::emit_method_binds(&model);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("method_binds.rs"), output).expect("write method binds");
}
