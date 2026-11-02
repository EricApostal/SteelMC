//! CLI entry point for the Steel extension ABI generator.

use std::env;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use steel_plugin_gen::{check_artifacts, generate, write_artifacts};

const USAGE: &str = "\
Usage: cargo run -p steel-plugin-gen [-- --check]

Writes the generated ABI artifacts:
  steel-abi/steel_api.json
  steel-abi/include/steel_extension.h
  clients/dart/lib/src/bindings.g.dart

Options:
  --check   Verify the generated files are up to date without writing them.
  -h, --help
            Show this message.
";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("steel-plugin-gen: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let check = match arguments.as_slice() {
        [] => false,
        [flag] if flag == "--check" => true,
        [flag] if flag == "-h" || flag == "--help" => {
            print_usage()?;
            return Ok(ExitCode::SUCCESS);
        }
        _ => {
            let _ = print_usage();
            return Err(format!("unrecognized arguments: {}", arguments.join(" ")));
        }
    };

    let root = workspace_root()?;
    let artifacts = generate(&root)?;

    if check {
        let stale = check_artifacts(&root, &artifacts);
        if stale.is_empty() {
            return Ok(ExitCode::SUCCESS);
        }
        for path in stale {
            eprintln!("stale: {}", path.display());
        }
        eprintln!("run `cargo run -p steel-plugin-gen` to regenerate");
        return Ok(ExitCode::FAILURE);
    }

    write_artifacts(&root, &artifacts)?;
    Ok(ExitCode::SUCCESS)
}

fn print_usage() -> Result<(), String> {
    io::stdout()
        .write_all(USAGE.as_bytes())
        .map_err(|error| format!("failed to write usage: {error}"))
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{} has no parent directory", manifest_dir.display()))
}
