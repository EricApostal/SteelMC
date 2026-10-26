//! Pack discovery and selection, mirroring vanilla
//! `net.minecraft.server.packs.repository.PackRepository`.
//!
//! Discovery mirrors vanilla `ServerPacksSource`: the builtin vanilla pack, the
//! feature packs shipped inside the vanilla tree (`datapacks/*`), and the world's
//! own `datapacks/` directory. Selection is a separate, persisted-in-config step;
//! vanilla keeps it in `level.dat`, Steel keeps it in the world directory's
//! `datapacks/` sibling file [`SELECTION_FILE`], which `/datapack enable|disable`
//! rewrites so the choice survives a restart.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use steel_utils::Identifier;

use super::pack::{
    DATA_PACK_FORMAT, PackError, PackLocationInfo, PackMetadataSection, PackResources, PackSource,
    PathPackResources, VanillaPackResources, builtin_pack_root,
};

/// File recording the enabled-pack selection, next to the world's `datapacks/`.
pub(crate) const SELECTION_FILE: &str = "datapacks.json";

/// Vanilla `Pack`: one discovered pack plus whether it is currently selected.
#[derive(Debug)]
pub(crate) struct Pack {
    /// Pack identity and origin.
    pub(crate) location: PackLocationInfo,
    /// `pack.mcmeta`, when the pack ships one.
    pub(crate) metadata: Option<PackMetadataSection>,
    /// Feature flags this pack requires (`pack.mcmeta` `features.enabled`).
    pub(crate) required_features: Vec<Identifier>,
    /// Whether this pack was selected at discovery time.
    pub(crate) selected: bool,
    resources: Arc<dyn PackResources>,
}

impl Pack {
    /// Vanilla `Pack.getId`.
    pub(crate) fn id(&self) -> &str {
        &self.location.id
    }

    /// Vanilla `Pack.getResources`.
    pub(crate) fn resources(&self) -> Arc<dyn PackResources> {
        Arc::clone(&self.resources)
    }

    /// Whether this pack may be selected by `/datapack enable`.
    ///
    /// Vanilla's builtin vanilla pack is fixed and cannot be disabled; feature and
    /// world packs can.
    pub(crate) const fn selectable(&self) -> bool {
        !matches!(self.location.source, PackSource::Builtin)
    }
}

/// A pack discovered from one [`RepositorySource`].
#[derive(Debug)]
pub(crate) struct PackCandidate {
    /// Pack identity and origin.
    pub(crate) location: PackLocationInfo,
    /// `pack.mcmeta`, when the pack ships one.
    pub(crate) metadata: Option<PackMetadataSection>,
    /// Feature flags this pack requires.
    pub(crate) required_features: Vec<Identifier>,
    /// Opening the pack's resource tree.
    pub(crate) resources: Arc<dyn PackResources>,
    /// Whether the pack is enabled when no selection file overrides it.
    pub(crate) default_enabled: bool,
}

/// Vanilla `RepositorySource`: one place packs can come from.
pub(crate) trait RepositorySource: Send + Sync {
    /// Appends every pack this source can discover.
    ///
    /// # Errors
    /// Propagates [`PackError`] for unreadable or malformed packs.
    fn discover(&self, out: &mut Vec<PackCandidate>) -> Result<(), PackError>;
}

/// The builtin vanilla pack plus the feature packs vendored beside it.
#[derive(Debug)]
pub(crate) struct BuiltinPackSource {
    /// `.../builtin_datapacks/minecraft`.
    vanilla_root: PathBuf,
}

impl BuiltinPackSource {
    /// Creates the builtin source, resolving the vendored tree.
    ///
    /// # Errors
    /// Returns [`PackError::MissingBuiltinPack`] when the tree cannot be found.
    pub(crate) fn discover_default() -> Result<Self, PackError> {
        Ok(Self {
            vanilla_root: builtin_pack_root()?,
        })
    }

    /// Creates the builtin source over an explicit vanilla pack root.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "vanilla test fixture API surface")
    )]
    pub(crate) const fn new(vanilla_root: PathBuf) -> Self {
        Self { vanilla_root }
    }
}

impl RepositorySource for BuiltinPackSource {
    fn discover(&self, out: &mut Vec<PackCandidate>) -> Result<(), PackError> {
        out.push(PackCandidate {
            location: PackLocationInfo {
                id: "vanilla".to_owned(),
                source: PackSource::Builtin,
            },
            metadata: Some(PackMetadataSection {
                min_format: DATA_PACK_FORMAT,
                max_format: DATA_PACK_FORMAT,
                description: "{\"text\":\"Vanilla\"}".to_owned(),
            }),
            required_features: Vec::new(),
            resources: Arc::new(VanillaPackResources::new(self.vanilla_root.clone())),
            default_enabled: true,
        });

        // Vanilla `VanillaPackResourcesBuilder` also exposes `datapacks/*` as
        // feature packs; `pack.mcmeta` `features.enabled` decides their default.
        let feature_root = self.vanilla_root.join("datapacks");
        let Ok(entries) = fs::read_dir(&feature_root) else {
            return Ok(());
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();

        for name in names {
            let root = feature_root.join(&name);
            let resources: Arc<dyn PackResources> = Arc::new(PathPackResources::new(
                PackLocationInfo {
                    id: format!("builtin/{name}"),
                    source: PackSource::Feature,
                },
                root,
            ));
            let metadata = resources.metadata();
            let required_features = read_required_features(resources.as_ref());
            out.push(PackCandidate {
                location: resources.location().clone(),
                default_enabled: false,
                metadata,
                required_features,
                resources,
            });
        }
        Ok(())
    }
}

/// The world's own `datapacks/` directory (vanilla `FolderRepositorySource`).
#[derive(Debug)]
pub(crate) struct WorldPackSource {
    /// `<world>/datapacks`.
    root: PathBuf,
}

impl WorldPackSource {
    /// Creates a source over a world's `datapacks/` directory.
    pub(crate) const fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl RepositorySource for WorldPackSource {
    fn discover(&self, out: &mut Vec<PackCandidate>) -> Result<(), PackError> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Ok(());
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();

        for name in names {
            let root = self.root.join(&name);
            let resources: Arc<dyn PackResources> = Arc::new(PathPackResources::new(
                PackLocationInfo {
                    id: format!("file/{name}"),
                    source: PackSource::World,
                },
                root,
            ));
            let metadata = resources.metadata();
            let required_features = read_required_features(resources.as_ref());
            out.push(PackCandidate {
                location: resources.location().clone(),
                metadata,
                required_features,
                resources,
                default_enabled: true,
            });
        }
        Ok(())
    }
}

/// Reads `features.enabled` from a pack's `pack.mcmeta`.
///
/// Vanilla `FeatureFlagsMetadataSection` treats these as the feature flags the
/// pack requires; `/datapack disable` refuses to drop a pack whose flags are still
/// enabled elsewhere.
fn read_required_features(resources: &dyn PackResources) -> Vec<Identifier> {
    let Some(bytes) = resources.get_root_resource("pack.mcmeta") else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    json.get("features")
        .and_then(|features| features.get("enabled"))
        .and_then(serde_json::Value::as_array)
        .map_or_else(Vec::new, |flags| {
            flags
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter_map(|flag| flag.parse::<Identifier>().ok())
                .collect()
        })
}

/// Where a newly enabled pack is inserted in the load order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PackInsert {
    /// Vanilla `Position.FIRST`: before every other pack.
    First,
    /// Vanilla `Position.LAST`: after every other pack.
    Last,
    /// Vanilla `Position.BEFORE`: immediately before the named pack.
    Before(String),
    /// Vanilla `Position.AFTER`: immediately after the named pack.
    After(String),
}

/// Vanilla `PackRepository`.
pub(crate) struct PackRepository {
    packs: Vec<Pack>,
    /// Enabled ids in load order (`vanilla` is always first).
    enabled: Vec<String>,
    selection_file: Option<PathBuf>,
    /// Sources are retained so [`PackRepository::rediscover`] can re-run them,
    /// mirroring vanilla `PackRepository.reload`.
    sources: Vec<Arc<dyn RepositorySource>>,
}

impl PackRepository {
    /// Discovers packs from `sources`, honoring `selection_file` when present.
    ///
    /// # Errors
    /// Propagates [`PackError`] from any source, and rejects packs whose
    /// `pack.mcmeta` format range excludes [`DATA_PACK_FORMAT`].
    pub(crate) fn new(
        sources: Vec<Arc<dyn RepositorySource>>,
        selection_file: Option<PathBuf>,
    ) -> Result<Self, PackError> {
        let (packs, enabled) = Self::assemble(&sources, selection_file.as_deref())?;
        Ok(Self {
            packs,
            enabled,
            selection_file,
            sources,
        })
    }

    /// Vanilla `PackRepository.reload`: re-runs every source and rebuilds the
    /// selection from the previously selected ids.
    ///
    /// Packs that disappeared are dropped from the selection; packs that appeared
    /// (for example a directory added to the world's `datapacks/` while the server
    /// runs) become available but stay unselected, matching vanilla's
    /// `rebuildSelected`, which only re-adds `Pack.isRequired()` packs (none on the
    /// server, where every `PackSelectionConfig` sets `required = false`). Returns
    /// whether the available set or the selection changed.
    ///
    /// # Errors
    /// Propagates [`PackError`] from any source.
    pub(crate) fn rediscover(&mut self) -> Result<bool, PackError> {
        self.rebuild_selection(false)
    }

    /// Vanilla `ReloadCommand.discoverNewPacks`, including the auto-add step of
    /// `MinecraftServer.configurePackRepository`.
    ///
    /// `/reload` selects every available pack the persisted selection does not list
    /// as disabled, so a pack dropped into the world's `datapacks/` while the
    /// server runs becomes active on the next `/reload`.
    ///
    /// # Errors
    /// Propagates [`PackError`] from any source.
    pub(crate) fn discover_new_packs(&mut self) -> Result<bool, PackError> {
        self.rebuild_selection(true)
    }

    /// Vanilla `PackRepository.rebuildSelected`: keep the previously selected ids
    /// that still exist, in order, with the builtin pack first.
    fn rebuild_selection(&mut self, enable_new: bool) -> Result<bool, PackError> {
        let previous_available = self.available_ids();
        let previous_enabled = self.enabled.clone();
        let (mut packs, _) = Self::assemble(&self.sources, None)?;
        // The packs the operator turned off. Vanilla reads the world's disabled list
        // the same way, so `/datapack disable` survives a reload and a restart.
        let disabled = self
            .selection_file
            .as_deref()
            .map_or_default(read_selection_file)
            .disabled;

        let mut enabled: Vec<String> = Vec::new();
        if let Some(vanilla) = packs
            .iter()
            .find(|pack| matches!(pack.location.source, PackSource::Builtin))
        {
            enabled.push(vanilla.id().to_owned());
        }
        for id in &previous_enabled {
            if packs.iter().any(|pack| pack.id() == id) && !enabled.contains(id) {
                enabled.push(id.clone());
            }
        }
        if enable_new {
            for pack in &packs {
                if pack.default_enabled()
                    && !disabled.iter().any(|id| id == pack.id())
                    && !enabled.iter().any(|id| id == pack.id())
                {
                    enabled.push(pack.id().to_owned());
                }
            }
        }

        for pack in &mut packs {
            pack.selected = enabled.iter().any(|id| id == pack.id());
        }
        // Compare the freshly discovered set against the previous state before installing it.
        let available: Vec<String> = packs.iter().map(|pack| pack.id().to_owned()).collect();
        let changed = available != previous_available || enabled != previous_enabled;
        self.packs = packs;
        self.enabled = enabled;
        Ok(changed)
    }

    /// Builds the pack list and the default selection for `sources`.
    fn assemble(
        sources: &[Arc<dyn RepositorySource>],
        selection_file: Option<&Path>,
    ) -> Result<(Vec<Pack>, Vec<String>), PackError> {
        let mut candidates = Vec::new();
        for source in sources {
            source.discover(&mut candidates)?;
        }

        let saved = selection_file.map_or_default(read_selection_file);

        let mut packs = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if let Some(metadata) = &candidate.metadata
                && !metadata.supports(DATA_PACK_FORMAT)
            {
                return Err(PackError::IncompatibleFormat {
                    id: candidate.location.id.clone(),
                    min: metadata.min_format,
                    max: metadata.max_format,
                    required: DATA_PACK_FORMAT,
                });
            }
            let selected = if candidate.default_enabled {
                // A saved selection can only disable, never drop the builtin pack.
                !saved.disabled.contains(&candidate.location.id)
            } else {
                saved.enabled.contains(&candidate.location.id)
            };
            packs.push(Pack {
                location: candidate.location,
                metadata: candidate.metadata,
                required_features: candidate.required_features,
                selected,
                resources: candidate.resources,
            });
        }

        let mut enabled: Vec<String> = Vec::new();
        if let Some(vanilla) = packs
            .iter()
            .find(|pack| matches!(pack.location.source, PackSource::Builtin))
        {
            enabled.push(vanilla.id().to_owned());
        }
        for id in &saved.enabled {
            if packs.iter().any(|pack| pack.id() == id) && !enabled.contains(id) {
                enabled.push(id.clone());
            }
        }
        for id in &saved.disabled {
            enabled.retain(|enabled| enabled != id);
        }
        // Default-enabled packs that were never explicitly disabled.
        for pack in &packs {
            if pack.selected && !enabled.iter().any(|id| id == pack.id()) {
                enabled.push(pack.id().to_owned());
            }
        }

        Ok((packs, enabled))
    }

    /// Vanilla `PackRepository.getAvailableIds`.
    pub(crate) fn available_ids(&self) -> Vec<String> {
        self.packs.iter().map(|pack| pack.id().to_owned()).collect()
    }

    /// Vanilla `PackRepository.getSelectedIds`.
    pub(crate) fn enabled_ids(&self) -> Vec<String> {
        self.enabled.clone()
    }

    /// Whether `id` is discovered and currently enabled.
    pub(crate) fn is_enabled(&self, id: &str) -> bool {
        self.enabled.iter().any(|enabled| enabled == id)
    }

    /// Vanilla `PackRepository.getPack`.
    pub(crate) fn pack(&self, id: &str) -> Option<&Pack> {
        self.packs.iter().find(|pack| pack.id() == id)
    }

    /// The feature flags `id` requires.
    #[expect(dead_code, reason = "vanilla Pack feature-flags API surface")]
    pub(crate) fn features_enabled_by(&self, id: &str) -> Vec<Identifier> {
        self.pack(id)
            .map_or_default(|pack| pack.required_features.clone())
    }

    /// Enabled packs in load order, used by the resource manager.
    pub(crate) fn selected_packs(&self) -> Vec<&Pack> {
        self.enabled.iter().filter_map(|id| self.pack(id)).collect()
    }

    /// Vanilla `PackRepository.setSelected`: enable or disable one pack.
    ///
    /// # Errors
    /// Returns [`PackSelectionError`] for an unknown, unselectable, already-enabled
    /// or feature-required pack.
    pub(crate) fn set_selected(
        &mut self,
        id: &str,
        enabled: bool,
        position: PackInsert,
    ) -> Result<bool, PackSelectionError> {
        let pack = self
            .pack(id)
            .ok_or_else(|| PackSelectionError::UnknownPack(id.to_owned()))?;
        if !pack.selectable() {
            return Err(PackSelectionError::Unselectable(id.to_owned()));
        }
        let is_enabled = self.enabled.iter().any(|current| current == id);
        if enabled == is_enabled {
            return Ok(false);
        }

        if enabled {
            let index = self.insert_index(&position)?;
            self.enabled.insert(index, id.to_owned());
        } else {
            // Vanilla refuses to drop the pack that supplies a still-required flag.
            if !pack.required_features.is_empty() {
                return Err(PackSelectionError::PackRequiredByOthers {
                    id: id.to_owned(),
                    features: pack.required_features.clone(),
                });
            }
            self.enabled.retain(|current| current != id);
        }
        if let Some(pack) = self.packs.iter_mut().find(|pack| pack.id() == id) {
            pack.selected = enabled;
        }
        Ok(true)
    }

    /// Computes the load-order index for a newly enabled pack.
    fn insert_index(&self, position: &PackInsert) -> Result<usize, PackSelectionError> {
        let anchor = |anchor: &str| -> Result<usize, PackSelectionError> {
            self.enabled
                .iter()
                .position(|current| current == anchor)
                .ok_or_else(|| PackSelectionError::AnchorNotEnabled(anchor.to_owned()))
        };
        Ok(match position {
            // `vanilla` always stays first, so a "first" pack goes right after it.
            PackInsert::First => 1.min(self.enabled.len()),
            PackInsert::Last => self.enabled.len(),
            PackInsert::Before(id) => anchor(id)?,
            PackInsert::After(id) => anchor(id)? + 1,
        })
    }

    /// Persists the current selection next to the world.
    ///
    /// # Errors
    /// Returns [`PackError::Io`] when the selection file cannot be written.
    pub(crate) fn save_selection(&self) -> Result<(), PackError> {
        let Some(path) = &self.selection_file else {
            return Ok(());
        };
        let defaults: Vec<&str> = self
            .packs
            .iter()
            .filter(|pack| {
                matches!(pack.location.source, PackSource::Builtin)
                    || (pack.selected && pack.default_enabled())
            })
            .map(Pack::id)
            .collect();
        let disabled: Vec<&str> = self
            .packs
            .iter()
            .filter(|pack| pack.default_enabled() && !self.is_enabled(pack.id()))
            .map(Pack::id)
            .collect();
        let enabled: Vec<&str> = self
            .enabled
            .iter()
            .map(String::as_str)
            .filter(|id| !defaults.contains(id))
            .collect();
        let document = serde_json::json!({ "enabled": enabled, "disabled": disabled });
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| PackError::Decode {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| PackError::Io {
                path: parent.display().to_string(),
                source,
            })?;
        }
        fs::write(path, bytes).map_err(|source| PackError::Io {
            path: path.display().to_string(),
            source,
        })
    }
}

impl Pack {
    /// Whether this pack is enabled when no selection file overrides it.
    const fn default_enabled(&self) -> bool {
        matches!(self.location.source, PackSource::Builtin)
            || matches!(self.location.source, PackSource::World)
    }
}

/// The persisted selection document.
#[derive(Debug, Default)]
struct Selection {
    enabled: Vec<String>,
    disabled: Vec<String>,
}

/// Reads [`SELECTION_FILE`], returning an empty selection when absent or unreadable.
fn read_selection_file(path: &Path) -> Selection {
    let Ok(bytes) = fs::read(path) else {
        return Selection::default();
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        tracing::warn!(path = %path.display(), "ignoring malformed datapack selection file");
        return Selection::default();
    };
    let ids = |name: &str| -> Vec<String> {
        json.get(name)
            .and_then(serde_json::Value::as_array)
            .map_or_default(|values| {
                values
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
    };
    Selection {
        enabled: ids("enabled"),
        disabled: ids("disabled"),
    }
}

/// Errors raised by `/datapack enable|disable`.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PackSelectionError {
    /// The pack id is not discovered.
    #[error("unknown data pack `{0}`")]
    UnknownPack(String),
    /// The builtin vanilla pack cannot be selected or dropped.
    #[error("data pack `{0}` is built in and cannot be changed")]
    Unselectable(String),
    /// A `before`/`after` anchor is not enabled.
    #[error("data pack `{0}` is not enabled")]
    AnchorNotEnabled(String),
    /// The pack supplies feature flags that are still enabled.
    #[error("data pack `{id}` provides features that are still required: {features:?}")]
    PackRequiredByOthers {
        /// Pack id.
        id: String,
        /// Features the pack supplies.
        features: Vec<Identifier>,
    },
}

/// Default world datapack directory for `save_path`.
pub(crate) fn world_datapack_dir(save_path: &Path) -> PathBuf {
    save_path.join("datapacks")
}

/// Creates a unique empty temporary directory for repository tests.
///
/// Kept outside the `tests` module so the sibling `datapack` tests reuse it
/// instead of duplicating the helper.
#[cfg(test)]
pub(crate) fn temp_dir(label: &str) -> PathBuf {
    use std::{env, process, thread};

    let base = env::temp_dir().join(format!(
        "steel-datapack-{label}-{}-{:?}",
        process::id(),
        thread::current().id()
    ));
    fs::create_dir_all(&base).expect("temporary directory should be creatable");
    base
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "these tests unwrap their own fixture data so failures point at the offending assertion"
)]
mod tests {
    use std::{fs, sync::Arc};

    use super::{PackInsert, PackRepository, RepositorySource, WorldPackSource};
    use crate::datapack::pack::PackType;

    #[test]
    fn missing_world_directory_discovers_no_packs() {
        let source = WorldPackSource::new("/definitely/not/a/directory".into());
        let mut candidates = Vec::new();
        source.discover(&mut candidates).unwrap();
        assert!(candidates.is_empty());
    }

    #[test]
    fn repository_without_sources_is_empty() {
        let repository = PackRepository::new(Vec::new(), None).unwrap();
        assert_eq!(repository.available_ids(), [] as [String; 0]);
        assert_eq!(repository.enabled_ids(), [] as [String; 0]);
        assert!(!repository.is_enabled("vanilla"));
    }

    #[test]
    fn enabling_unknown_pack_fails() {
        let mut repository = PackRepository::new(Vec::new(), None).unwrap();
        assert!(
            repository
                .set_selected("file/missing", true, PackInsert::Last)
                .is_err()
        );
    }

    #[test]
    fn world_pack_type_directory_is_data() {
        assert_eq!(PackType::Server.directory(), "data");
    }

    #[test]
    fn rediscover_picks_up_a_pack_added_after_construction() {
        let root = super::temp_dir("rediscover");
        let packs = root.join("datapacks");
        fs::create_dir_all(&packs).unwrap();

        let sources: Vec<Arc<dyn RepositorySource>> = vec![
            Arc::new(super::BuiltinPackSource::new(root.clone())),
            Arc::new(WorldPackSource::new(packs.clone())),
        ];
        let mut repository = PackRepository::new(sources, None).unwrap();
        assert!(!repository.is_enabled("file/late_pack"));
        assert!(!repository.rediscover().unwrap(), "nothing changed yet");

        // A pack directory that appears while the server runs must become available.
        let late = packs.join("late_pack");
        fs::create_dir_all(late.join("data/minecraft")).unwrap();
        fs::write(
            late.join("pack.mcmeta"),
            format!(
                "{{\"pack\":{{\"min_format\":{format},\"max_format\":{format},\"description\":\"late\"}}}}",
                format = super::DATA_PACK_FORMAT
            ),
        )
        .unwrap();

        // Vanilla `PackRepository.reload` only rebuilds the previous selection, so
        // `/datapack list` shows the new pack as available.
        assert!(repository.rediscover().unwrap(), "the pack set changed");
        assert!(
            repository
                .available_ids()
                .contains(&"file/late_pack".to_owned())
        );
        assert!(!repository.is_enabled("file/late_pack"));

        // Vanilla `ReloadCommand.discoverNewPacks` selects it, so a dropped pack
        // becomes active on the next `/reload`.
        assert!(repository.discover_new_packs().unwrap());
        assert!(repository.is_enabled("file/late_pack"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reload_keeps_a_disabled_pack_disabled() {
        let root = super::temp_dir("rediscover-disabled");
        let packs = root.join("datapacks");
        let pack = packs.join("my_pack");
        fs::create_dir_all(pack.join("data/minecraft")).unwrap();
        fs::write(
            pack.join("pack.mcmeta"),
            format!(
                "{{\"pack\":{{\"min_format\":{format},\"max_format\":{format}}}}}",
                format = super::DATA_PACK_FORMAT
            ),
        )
        .unwrap();

        let sources = || -> Vec<Arc<dyn RepositorySource>> {
            vec![
                Arc::new(super::BuiltinPackSource::new(root.clone())),
                Arc::new(WorldPackSource::new(packs.clone())),
            ]
        };
        let selection = root.join(super::SELECTION_FILE);
        let mut repository = PackRepository::new(sources(), Some(selection.clone())).unwrap();
        assert!(repository.is_enabled("file/my_pack"));

        // `/datapack disable file/my_pack` persists the operator's choice.
        assert!(
            repository
                .set_selected("file/my_pack", false, PackInsert::Last)
                .unwrap()
        );
        repository.save_selection().unwrap();

        // `/reload` re-discovers and must not resurrect it.
        repository.discover_new_packs().unwrap();
        assert!(!repository.is_enabled("file/my_pack"));
        assert!(
            repository
                .available_ids()
                .contains(&"file/my_pack".to_owned())
        );

        // A restart reads the same selection file and stays disabled too.
        let reopened = PackRepository::new(sources(), Some(selection)).unwrap();
        assert!(!reopened.is_enabled("file/my_pack"));

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rediscover_drops_a_pack_whose_directory_vanished() {
        let root = super::temp_dir("rediscover-drop");
        let packs = root.join("datapacks");
        let pack = packs.join("gone_soon");
        fs::create_dir_all(pack.join("data/minecraft")).unwrap();
        fs::write(
            pack.join("pack.mcmeta"),
            format!(
                "{{\"pack\":{{\"min_format\":{format},\"max_format\":{format}}}}}",
                format = super::DATA_PACK_FORMAT
            ),
        )
        .unwrap();

        let sources: Vec<Arc<dyn RepositorySource>> = vec![
            Arc::new(super::BuiltinPackSource::new(root.clone())),
            Arc::new(WorldPackSource::new(packs)),
        ];
        let mut repository = PackRepository::new(sources, None).unwrap();
        assert!(repository.is_enabled("file/gone_soon"));

        fs::remove_dir_all(&pack).unwrap();
        assert!(repository.rediscover().unwrap());
        assert!(
            !repository
                .available_ids()
                .contains(&"file/gone_soon".to_owned())
        );
        assert!(!repository.is_enabled("file/gone_soon"));

        fs::remove_dir_all(&root).ok();
    }
}
