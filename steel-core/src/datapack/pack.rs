//! Data-pack resources, mirroring vanilla `net.minecraft.server.packs`.
//!
//! Vanilla ships its data inside the server jar and exposes it through
//! [`VanillaPackResources`]; Steel's equivalent is the vendored
//! `steel-utils/build_assets/builtin_datapacks/minecraft` tree, which is located
//! at runtime by [`builtin_pack_root`]. User packs are plain directories read by
//! [`PathPackResources`].
//!
//! **Deviation from vanilla:** only [`PackType::Server`] exists. Steel has no
//! client resource tree (vanilla's `PackType.CLIENT` covers textures, models and
//! sounds, which the vanilla client supplies), so modelling it would be dead
//! weight. `/datapack` and the reload path are server-only in vanilla too.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

use steel_utils::Identifier;

/// Vanilla `PackType`: which resource namespace tree a pack contributes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum PackType {
    /// Server-side data (`data/<namespace>/...`).
    Server,
}

impl PackType {
    /// Vanilla `PackType.getDirectory`.
    pub(crate) const fn directory(self) -> &'static str {
        match self {
            Self::Server => "data",
        }
    }
}

/// Vanilla `PackLocationInfo`: the stable identity of one pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackLocationInfo {
    /// Stable pack id, e.g. `vanilla`, `file/my_pack`, `builtin/trade_rebalance`.
    pub(crate) id: String,
    /// Where the pack came from, used by `/datapack list`.
    pub(crate) source: PackSource,
}

/// Vanilla `PackSource`: the repository that discovered a pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PackSource {
    /// The built-in vanilla data.
    Builtin,
    /// A pack shipped inside the vanilla tree but not enabled by default.
    Feature,
    /// A pack discovered in the world's `datapacks/` directory.
    World,
}

impl PackSource {
    /// Vanilla `PackSource.getPromotion`: the source name shown by `/datapack list`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Feature => "feature",
            Self::World => "world",
        }
    }
}

/// Vanilla `PackMetadataSection`, read from a pack's `pack.mcmeta`.
///
/// Vanilla 26.3 replaced the single `pack_format` with a `min_format`/`max_format`
/// range; both forms are accepted so packs authored for either shape load.
#[derive(Debug, Clone)]
pub(crate) struct PackMetadataSection {
    /// Lowest data pack format the pack supports.
    pub(crate) min_format: i32,
    /// Highest data pack format the pack supports.
    pub(crate) max_format: i32,
    /// Human-readable description (a JSON text component, stored as raw JSON).
    pub(crate) description: String,
}

impl PackMetadataSection {
    /// Whether this pack supports `format` (`min_format..=max_format`).
    pub(crate) const fn supports(&self, format: i32) -> bool {
        format >= self.min_format && format <= self.max_format
    }
}

/// The data pack format this build reads, matching the vanilla 26.3 builtin pack.
pub(crate) const DATA_PACK_FORMAT: i32 = 121;

/// A readable pack, mirroring vanilla `PackResources`.
pub(crate) trait PackResources: std::fmt::Debug + Send + Sync {
    /// Vanilla `PackResources.packId`.
    fn location(&self) -> &PackLocationInfo;

    /// Vanilla `PackResources.getRootResource`: a resource directly under the pack root.
    fn get_root_resource(&self, path: &str) -> Option<Vec<u8>>;

    /// Vanilla `PackResources.getResource`: `data/<namespace>/<path>`.
    fn get_resource(&self, pack_type: PackType, location: &Identifier) -> Option<Vec<u8>> {
        let path = format!(
            "{}/{}/{}",
            pack_type.directory(),
            location.namespace,
            location.path
        );
        self.get_root_resource(&path)
    }

    /// Vanilla `PackResources.getNamespaces`.
    fn get_namespaces(&self, pack_type: PackType) -> Vec<String>;

    /// Vanilla `PackResources.listResources`: appends every resource under
    /// `<pack_type>/<namespace>/<path>` whose file name starts with `prefix`.
    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        path: &str,
        prefix: &str,
        out: &mut Vec<String>,
    );

    /// Vanilla `PackResources.getMetadataSection`.
    fn metadata(&self) -> Option<PackMetadataSection> {
        let bytes = self.get_root_resource("pack.mcmeta")?;
        parse_pack_mcmeta(&bytes).ok()
    }
}

/// Parses a `pack.mcmeta` document into its [`PackMetadataSection`].
///
/// # Errors
/// Returns [`PackError::Metadata`] when the document is malformed or lacks a format.
pub(crate) fn parse_pack_mcmeta(bytes: &[u8]) -> Result<PackMetadataSection, PackError> {
    let json: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| PackError::Metadata(format!("invalid pack.mcmeta: {error}")))?;
    let pack = json.get("pack").ok_or_else(|| {
        PackError::Metadata("pack.mcmeta is missing the `pack` object".to_owned())
    })?;

    let int_field = |name: &str| -> Result<Option<i32>, PackError> {
        let Some(value) = pack.get(name) else {
            return Ok(None);
        };
        let value = value
            .as_i64()
            .ok_or_else(|| PackError::Metadata(format!("`pack.{name}` must be an integer")))?;
        i32::try_from(value)
            .map(Some)
            .map_err(|_| PackError::Metadata(format!("`pack.{name}` is out of range")))
    };

    let legacy = int_field("pack_format")?;
    let min_format = int_field("min_format")?.or(legacy).ok_or_else(|| {
        PackError::Metadata(
            "pack.mcmeta must declare `pack.min_format`/`pack.max_format` or `pack.pack_format`"
                .to_owned(),
        )
    })?;
    let max_format = int_field("max_format")?.or(legacy).unwrap_or(min_format);

    Ok(PackMetadataSection {
        min_format,
        max_format,
        description: pack
            .get("description")
            .map_or_else(String::new, ToString::to_string),
    })
}

/// Resolves `relative` under `root`, rejecting absolute paths and `..`.
///
/// Vanilla relies on the file system to reject traversal; Steel validates the
/// relative path explicitly so a malicious pack cannot escape its root.
fn resolve_under(root: &Path, relative: &str) -> Option<PathBuf> {
    let candidate = Path::new(relative);
    if candidate
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(candidate))
}

/// Recursively appends the paths of `dir`'s files whose name starts with `prefix`.
fn collect_files(dir: &Path, base: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, base, prefix, out);
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        if let Ok(relative) = path.strip_prefix(base)
            && let Some(relative) = relative.to_str()
        {
            out.push(relative.replace('\\', "/"));
        }
    }
}

/// A pack backed by a directory on disk (vanilla `PathPackResources`).
///
/// `root` is the directory that contains the `data/` tree.
#[derive(Debug)]
pub(crate) struct PathPackResources {
    location: PackLocationInfo,
    root: PathBuf,
}

impl PathPackResources {
    /// Creates a directory-backed pack. The root is read lazily.
    pub(crate) fn new(location: PackLocationInfo, root: PathBuf) -> Self {
        Self { location, root }
    }
}

impl PackResources for PathPackResources {
    fn location(&self) -> &PackLocationInfo {
        &self.location
    }

    fn get_root_resource(&self, path: &str) -> Option<Vec<u8>> {
        fs::read(resolve_under(&self.root, path)?).ok()
    }

    fn get_namespaces(&self, pack_type: PackType) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.root.join(pack_type.directory())) else {
            return Vec::new();
        };
        let mut namespaces: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        namespaces.sort();
        namespaces
    }

    fn list_resources(
        &self,
        pack_type: PackType,
        namespace: &str,
        path: &str,
        prefix: &str,
        out: &mut Vec<String>,
    ) {
        let relative = format!("{}/{namespace}/{path}", pack_type.directory());
        let Some(dir) = resolve_under(&self.root, &relative) else {
            return;
        };
        collect_files(&dir, &dir, prefix, out);
    }
}

/// The vanilla pack, backed by the vendored builtin datapack tree.
///
/// Vanilla reads these files from the server jar; Steel reads the same tree from
/// [`builtin_pack_root`]. Every registry entry is still generated at build time —
/// this pack exists so `/reload`, `pack.mcmeta` compatibility and mcfunction
/// discovery see the same resources vanilla does.
///
/// **Deviation from vanilla:** the extracted tree stores the `minecraft`
/// namespace's data at the pack root (no `data/minecraft/` prefix), which is the
/// layout `steel-utils/build_assets` uses; [`get_resource`](PackResources::get_resource)
/// therefore strips `data/minecraft/` for that namespace.
#[derive(Debug)]
pub(crate) struct VanillaPackResources {
    location: PackLocationInfo,
    root: PathBuf,
}

impl VanillaPackResources {
    /// Creates the vanilla pack over `root` (`.../builtin_datapacks/minecraft`).
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            location: PackLocationInfo {
                id: "vanilla".to_owned(),
                source: PackSource::Builtin,
            },
            root,
        }
    }
}

impl PackResources for VanillaPackResources {
    fn location(&self) -> &PackLocationInfo {
        &self.location
    }

    fn get_root_resource(&self, path: &str) -> Option<Vec<u8>> {
        fs::read(resolve_under(&self.root, path)?).ok()
    }

    fn get_resource(&self, _pack_type: PackType, location: &Identifier) -> Option<Vec<u8>> {
        // The extracted tree stores the `minecraft` namespace's data at the pack
        // root (`<path>.json`), not under `data/minecraft/`.
        if location.namespace != "minecraft" {
            return None;
        }
        let path = location.path.as_ref();
        let file = if Path::new(path).extension().is_some() {
            path.to_owned()
        } else {
            format!("{path}.json")
        };
        self.get_root_resource(&file)
    }

    fn get_namespaces(&self, _pack_type: PackType) -> Vec<String> {
        vec!["minecraft".to_owned()]
    }

    fn list_resources(
        &self,
        _pack_type: PackType,
        namespace: &str,
        path: &str,
        prefix: &str,
        out: &mut Vec<String>,
    ) {
        if namespace != "minecraft" {
            return;
        }
        let dir = self.root.join(path);
        collect_files(&dir, &dir, prefix, out);
    }

    fn metadata(&self) -> Option<PackMetadataSection> {
        // The vendored tree has no `pack.mcmeta`; vanilla's builtin pack always
        // matches the running version, so its format is this build's format.
        Some(PackMetadataSection {
            min_format: DATA_PACK_FORMAT,
            max_format: DATA_PACK_FORMAT,
            description: "{\"text\":\"Vanilla\"}".to_owned(),
        })
    }
}

/// Resolves the vendored builtin datapack tree (`.../builtin_datapacks/minecraft`).
///
/// Vanilla reads its pack from the server jar. Steel keeps the extracted tree on
/// disk, so the location is resolved in this order:
///
/// 1. `$STEEL_DATAPACK_ROOT` (a directory containing `builtin_datapacks/`),
/// 2. `<executable dir>/datapacks/`,
/// 3. `<executable dir>/../steel-utils/build_assets/`,
/// 4. the workspace path `<CARGO_MANIFEST_DIR>/../steel-utils/build_assets/`.
///
/// # Errors
/// Returns [`PackError::MissingBuiltinPack`] when no candidate contains the pack.
pub(crate) fn builtin_pack_root() -> Result<PathBuf, PackError> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(root) = std::env::var_os("STEEL_DATAPACK_ROOT") {
        candidates.push(PathBuf::from(root));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        candidates.push(dir.join("datapacks"));
        if let Some(parent) = dir.parent() {
            candidates.push(parent.join("steel-utils/build_assets"));
        }
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../steel-utils/build_assets"));

    for candidate in candidates {
        let pack = candidate.join("builtin_datapacks/minecraft");
        // The vendored tree has no `pack.mcmeta`; a registry directory is the marker.
        if pack.join("loot_table").is_dir() {
            return Ok(pack);
        }
    }

    Err(PackError::MissingBuiltinPack)
}

/// Errors raised while discovering, reading or reloading data packs.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PackError {
    /// No candidate directory contained the vendored vanilla pack.
    #[error(
        "could not locate the builtin datapack tree; set STEEL_DATAPACK_ROOT to the directory containing `builtin_datapacks/`"
    )]
    MissingBuiltinPack,
    /// A `pack.mcmeta` was present but malformed.
    #[error("{0}")]
    Metadata(String),
    /// A pack's declared format range excludes this build's format.
    #[error(
        "pack `{id}` supports data pack formats {min}..={max}, this server requires {required}"
    )]
    IncompatibleFormat {
        /// Pack id.
        id: String,
        /// Lowest format declared by the pack.
        min: i32,
        /// Highest format declared by the pack.
        max: i32,
        /// Format this build reads.
        required: i32,
    },
    /// A resource could not be read.
    #[error("failed to read `{path}`: {source}")]
    Io {
        /// Resource path.
        path: String,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// A JSON resource did not decode into its registry entry.
    #[error("failed to decode `{path}`: {message}")]
    Decode {
        /// Resource path.
        path: String,
        /// Decoder error message.
        message: String,
    },
}
