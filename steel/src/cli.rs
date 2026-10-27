//! Command-line options, mirroring vanilla `net.minecraft.server.Main`'s option set.
//!
//! Steel keeps its authoritative settings in `config/config.toml` and
//! `config/worlds.toml`; the flags below only override the startup identity
//! the vanilla launcher exposes:
//!
//! | Flag | Effect |
//! |------|--------|
//! | `--nogui` | Accepted for script parity. Steel has no Swing GUI to suppress, so log and console handling stay unchanged (vanilla still shows its text console with `--nogui`). |
//! | `--port <n>` | Overrides `server.server_port`. Negative values fall back to the config, like vanilla's default `-1`. |
//! | `--world <name>` | Selects which `worlds.toml` domain boots as the default level, Steel's equivalent of vanilla's `level-name`. |
//! | `--universe <dir>` | Overrides the save root (`worlds.toml` `save_path`), Steel's equivalent of vanilla's `--universe` working directory. |
//! | `--pidFile <path>` | Writes the process id and continues, like vanilla `--pidFile`. |
//! | `--initSettings` | Writes the default config and exits, like vanilla `--initSettings` (no `eula.txt`: Steel does not gate startup on the EULA). |
//!
//! Vanilla-only workflow flags that have no Steel equivalent (`--demo`,
//! `--bonusChest`, `--forceUpgrade`, `--eraseCache`, `--recreateRegionFiles`,
//! `--safeMode`, `--jfrProfile`, `--serverId`) are rejected with a startup
//! error instead of being silently ignored.

use std::ffi::OsString;
use std::path::PathBuf;

/// Parsed command-line options.
#[derive(Debug, Default)]
pub struct CliArgs {
    /// Whether to write default settings and exit before booting.
    pub init_settings: bool,
    /// Save-root override (`--universe`).
    pub universe: Option<PathBuf>,
    /// Default-domain override (`--world`).
    pub world: Option<String>,
    /// Port override (`--port`).
    pub port: Option<i64>,
    /// Process-id file (`--pidFile`).
    pub pid_file: Option<PathBuf>,
}

impl CliArgs {
    /// Parses the process arguments (vanilla `Main#main` option handling).
    ///
    /// # Errors
    /// Returns a message for the usage text when `--help` is passed, when
    /// an unknown option is passed, or when a Steel-unsupported vanilla option
    /// is passed.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut args = args.into_iter();
        // Skip the executable name, mirroring `args[0]`.
        args.next();
        while let Some(arg) = args.next() {
            let Some(text) = arg.to_str() else {
                return Err(format!("invalid argument: {}", arg.to_string_lossy()));
            };
            // Vanilla treats a bare `nogui` token as requesting no GUI. Steel
            // never opens a GUI, so the flag only needs to be accepted.
            if text == "nogui" {
                continue;
            }
            let Some(name) = text.strip_prefix("--") else {
                return Err(format!(
                    "unexpected argument '{text}'; Steel only accepts `--<option>` flags (see --help)"
                ));
            };
            let (name, inline_value) = match name.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (name, None),
            };
            match name {
                "help" => return Err(usage()),
                "nogui" => reject_value("nogui", inline_value)?,
                "initSettings" => {
                    reject_value("initSettings", inline_value)?;
                    parsed.init_settings = true;
                }
                "port" => {
                    parsed.port = Some(parse_value("port", inline_value, &mut args)?);
                }
                "world" => {
                    parsed.world = Some(parse_string("world", inline_value, &mut args)?);
                }
                "universe" => {
                    parsed.universe = Some(PathBuf::from(parse_string(
                        "universe",
                        inline_value,
                        &mut args,
                    )?));
                }
                "pidFile" => {
                    parsed.pid_file = Some(PathBuf::from(parse_string(
                        "pidFile",
                        inline_value,
                        &mut args,
                    )?));
                }
                "demo"
                | "bonusChest"
                | "forceUpgrade"
                | "eraseCache"
                | "recreateRegionFiles"
                | "safeMode"
                | "jfrProfile"
                | "serverId" => {
                    return Err(format!(
                        "option '--{name}' has no Steel equivalent and is not supported"
                    ));
                }
                _ => {
                    return Err(format!("unknown option '--{name}' (see --help)"));
                }
            }
        }
        Ok(parsed)
    }
}

fn reject_value(name: &str, inline_value: Option<String>) -> Result<(), String> {
    if inline_value.is_some() {
        return Err(format!("option '--{name}' takes no value"));
    }
    Ok(())
}

fn parse_string(
    name: &str,
    inline_value: Option<String>,
    args: &mut impl Iterator<Item = OsString>,
) -> Result<String, String> {
    if let Some(value) = inline_value {
        return Ok(value);
    }
    args.next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| format!("option '--{name}' requires a value"))
}

fn parse_value(
    name: &str,
    inline_value: Option<String>,
    args: &mut impl Iterator<Item = OsString>,
) -> Result<i64, String> {
    let text = parse_string(name, inline_value, args)?;
    text.parse::<i64>()
        .map_err(|_| format!("option '--{name}' requires an integer, got '{text}'"))
}

/// Short usage text printed for `--help` (vanilla `parser.printHelpOn`).
fn usage() -> String {
    "Steel Minecraft server\n\nOptions:\n  --nogui            run without a GUI window (Steel never opens one)\n  --port <n>         override server.server_port (negative falls back to config)\n  --world <name>     select the worlds.toml domain that boots as the default level\n  --universe <dir>   override the save root (worlds.toml save_path)\n  --pidFile <path>   write the process id to <path> and continue\n  --initSettings     write the default config, then exit\n  --help             show this message"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::CliArgs;
    use std::ffi::OsString;
    use std::path::Path;

    fn parse(args: &[&str]) -> Result<CliArgs, String> {
        CliArgs::parse(args.iter().map(|arg| OsString::from(*arg)))
    }

    #[test]
    fn parses_overrides() {
        let args = parse(&[
            "steel",
            "--port",
            "25566",
            "--world=adventure",
            "--universe",
            "data",
            "--nogui",
        ])
        .expect("valid flags should parse");
        assert_eq!(args.port, Some(25566));
        assert_eq!(args.world.as_deref(), Some("adventure"));
        assert_eq!(args.universe.as_deref(), Some(Path::new("data")));
        assert!(!args.init_settings);
    }

    #[test]
    fn accepts_nogui_forms() {
        parse(&["steel", "nogui"]).expect("bare nogui should parse");
        parse(&["steel", "--nogui"]).expect("--nogui should parse");
    }

    #[test]
    fn rejects_unknown_and_unsupported_options() {
        assert!(parse(&["steel", "--bogus"]).is_err());
        assert!(parse(&["steel", "--demo"]).is_err());
        assert!(parse(&["steel", "--help"]).is_err());
        assert!(parse(&["steel", "--port"]).is_err());
        assert!(parse(&["steel", "--port", "abc"]).is_err());
        assert!(parse(&["steel", "--nogui=x"]).is_err());
    }
}
