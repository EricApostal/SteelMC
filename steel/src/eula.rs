//! Mojang EULA acceptance, mirroring `net.minecraft.server.Eula`.
//!
//! Vanilla reads `eula.txt` from the working directory and refuses to boot
//! until the operator sets `eula=true`. A missing or unreadable file is
//! replaced with commented-out `eula=false` defaults and counts as rejected.

use std::fs;
use std::path::Path;

/// The Mojang EULA URL vanilla prints into a fresh `eula.txt`.
/// Mirrors `net.minecraft.util.CommonLinks#EULA`.
pub const EULA_URL: &str = "https://aka.ms/MinecraftEULA";

/// Parsed `eula.txt` acceptance state.
pub struct Eula {
    agreed: bool,
    wrote_defaults: bool,
}

impl Eula {
    /// Loads `eula.txt`, writing `eula=false` defaults when the file is
    /// missing or cannot be parsed (vanilla `Eula#readFile` behavior).
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let contents = fs::read_to_string(path).ok();
        let Some(contents) = contents else {
            Self::save_defaults(path);
            return Self {
                agreed: false,
                wrote_defaults: true,
            };
        };
        Self {
            agreed: agrees(&contents),
            wrote_defaults: false,
        }
    }

    /// Whether the operator agreed to the EULA (`eula=true`).
    #[must_use]
    pub const fn has_agreed_to_eula(&self) -> bool {
        self.agreed
    }

    /// Whether loading wrote fresh defaults because the file was missing or unreadable.
    #[must_use]
    pub const fn wrote_defaults(&self) -> bool {
        self.wrote_defaults
    }

    /// Writes fresh `eula=false` defaults (vanilla `Eula#saveDefaults`).
    pub fn save_defaults(path: &Path) {
        let contents = format!(
            "# By changing the setting below to TRUE you are indicating your agreement to our EULA ({EULA_URL}).\neula=false\n"
        );
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, contents);
    }
}

/// Returns whether the given `eula.txt` contents agree to the EULA.
///
/// Mirrors `Boolean.parseBoolean(Properties.getProperty("eula", "false"))`:
/// the last `eula` assignment wins, comparison is ASCII case-insensitive,
/// `#`/`!` comment lines and surrounding whitespace are ignored.
fn agrees(contents: &str) -> bool {
    let mut agreed = false;
    for line in contents.lines() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        let (key, value) = split_assignment(line);
        if key.trim() == "eula" {
            agreed = value.trim().eq_ignore_ascii_case("true");
        }
    }
    agreed
}

/// Splits a Java-properties line into its key and value.
///
/// The key ends at the first `=`, `:` or whitespace; whitespace after the key
/// is skipped, then an optional `=`/`:` separator, then more whitespace.
/// Escape sequences in values are passed through, which cannot turn a
/// non-`true` value into `true`.
fn split_assignment(line: &str) -> (&str, &str) {
    let key_end = line.find(['=', ':', ' ', '\t']).unwrap_or(line.len());
    let (key, rest) = line.split_at(key_end);
    let rest = rest.trim_start_matches([' ', '\t']);
    let rest = rest.strip_prefix(['=', ':']).unwrap_or(rest);
    (key, rest)
}

#[cfg(test)]
mod tests {
    use super::agrees;

    #[test]
    fn parses_vanilla_eula_files() {
        assert!(agrees("eula=true\n"));
        assert!(agrees("# comment\neula=TRUE\n"));
        assert!(agrees("eula = true"));
        assert!(agrees("eula:true"));
        assert!(agrees("eula=false\neula=true"));
        assert!(!agrees("eula=false\n"));
        assert!(!agrees(""));
        assert!(!agrees("# eula=true\n"));
        assert!(!agrees("eula=yes\n"));
    }
}
