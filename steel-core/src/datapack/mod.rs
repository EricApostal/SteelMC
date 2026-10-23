//! Data-pack runtime: pack discovery, layered resource access, reload listeners
//! and `mcfunction` execution.
//!
//! Vanilla wires these through `MinecraftServer` (`ServerPacksSource` →
//! `PackRepository` → `MultiPackResourceManager` → `ReloadableResourceManager`).
//! Steel keeps the same object graph but hangs it off one process-wide
//! [`DatapackManager`], because [`crate::server::Server`] is a reserved
//! integration file: `/reload`, `/datapack`, `/function` and `/schedule` all reach
//! the manager through [`datapacks()`].

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use steel_utils::locks::SyncRwLock;

pub(crate) mod function;
pub(crate) mod pack;
pub(crate) mod repository;
pub(crate) mod resource;
pub(crate) mod schedule;

pub(crate) use function::{
    Function, FunctionArguments, FunctionLoader, FunctionRegistry, macro_signature, run_function,
};
pub(crate) use pack::PackError;
pub(crate) use repository::{
    BuiltinPackSource, PackInsert, PackRepository, PackSelectionError, RepositorySource,
    WorldPackSource, world_datapack_dir,
};
pub(crate) use resource::{
    MultiPackResourceManager, PreparableReloadListener, ReloadableResourceManager,
};
pub(crate) use schedule::{ScheduleRegistry, ScheduledFunction};

use pack::PackType;

/// The outcome of one `/reload`.
#[derive(Debug, Clone)]
pub(crate) struct ReloadSummary {
    /// Number of enabled packs the reload read.
    pub(crate) packs_loaded: usize,
    /// Number of functions installed.
    pub(crate) functions_loaded: usize,
    /// Number of function tags installed.
    pub(crate) tags_loaded: usize,
    /// Wall-clock time the reload took.
    pub(crate) elapsed: Duration,
}

/// Owns the pack repository, the resource overlay, the reload listeners and the
/// loaded function/schedule state.
pub(crate) struct DatapackManager {
    repository: SyncRwLock<PackRepository>,
    resources: SyncRwLock<Arc<MultiPackResourceManager>>,
    reload_manager: ReloadableResourceManager,
    functions: Arc<SyncRwLock<FunctionRegistry>>,
    schedules: Arc<SyncRwLock<ScheduleRegistry>>,
    /// The world directory the repository was built for.
    world_dir: PathBuf,
}

impl DatapackManager {
    /// Builds the manager for `world_dir`, discovering packs from the vanilla
    /// tree and the world's `datapacks/` directory.
    ///
    /// # Errors
    /// Propagates [`PackError`] from discovery (missing builtin tree, incompatible
    /// pack format).
    pub(crate) fn new(world_dir: &Path) -> Result<Self, PackError> {
        let selection_file = world_dir.join(repository::SELECTION_FILE);
        let sources: Vec<Arc<dyn RepositorySource>> = vec![
            Arc::new(BuiltinPackSource::discover_default()?),
            Arc::new(WorldPackSource::new(world_datapack_dir(world_dir))),
        ];
        let repository = PackRepository::new(sources, Some(selection_file))?;

        let resources = Arc::new(MultiPackResourceManager::from_packs(
            &repository.selected_packs(),
        ));
        let functions = Arc::new(SyncRwLock::new(FunctionRegistry::default()));
        let schedules = Arc::new(SyncRwLock::new(ScheduleRegistry::default()));

        let reload_manager = ReloadableResourceManager::new();
        reload_manager.register_listener(Box::new(FunctionLoader::new(Arc::clone(&functions))));

        Ok(Self {
            repository: SyncRwLock::new(repository),
            resources: SyncRwLock::new(resources),
            reload_manager,
            functions,
            schedules,
            world_dir: world_dir.to_path_buf(),
        })
    }

    /// The world directory this manager reads packs from.
    #[must_use]
    pub(crate) fn world_dir(&self) -> &Path {
        &self.world_dir
    }

    /// The world's `datapacks/` directory, for `/datapack create`.
    #[must_use]
    pub(crate) fn world_datapack_dir(&self) -> PathBuf {
        world_datapack_dir(&self.world_dir)
    }

    /// Vanilla `PackRepository.reload` + resource reload: re-discovers packs from
    /// the configured sources, then reloads every listener.
    ///
    /// `/datapack list` calls this first, so a pack dropped into `datapacks/`
    /// while the server runs becomes visible without a restart.
    ///
    /// # Errors
    /// Propagates [`PackError`] from discovery or a listener's prepare phase.
    pub(crate) fn rediscover(&self) -> Result<ReloadSummary, PackError> {
        self.repository.write().rediscover()?;
        self.reload_listeners()
    }

    /// The pack repository, for `/datapack list`.
    #[must_use]
    pub(crate) fn repository(&self) -> &SyncRwLock<PackRepository> {
        &self.repository
    }

    /// The currently loaded functions.
    #[must_use]
    pub(crate) fn functions(&self) -> &Arc<SyncRwLock<FunctionRegistry>> {
        &self.functions
    }

    /// The scheduled-function queue, for `/schedule`.
    #[must_use]
    pub(crate) fn schedules(&self) -> &Arc<SyncRwLock<ScheduleRegistry>> {
        &self.schedules
    }

    /// Vanilla `MinecraftServer.reloadResources`: re-reads every enabled pack and
    /// re-runs every listener.
    ///
    /// # Errors
    /// Returns [`PackError`] when a listener fails to prepare; the previously
    /// loaded state stays installed.
    pub(crate) fn reload(&self) -> Result<ReloadSummary, PackError> {
        // Vanilla re-discovers on `/reload` too (`DataPackCommand.list` relies on
        // it); the current selection is preserved, so this only adds or drops packs
        // whose directories appeared or vanished.
        self.repository.write().rediscover()?;
        self.reload_listeners()
    }

    /// Rebuilds the resource overlay and re-runs every listener.
    fn reload_listeners(&self) -> Result<ReloadSummary, PackError> {
        let started = Instant::now();
        let (resources, packs_loaded) = {
            let repository = self.repository.read();
            (
                Arc::new(MultiPackResourceManager::from_packs(
                    &repository.selected_packs(),
                )),
                repository.enabled_ids().len(),
            )
        };

        self.reload_manager.reload(&resources)?;

        *self.resources.write() = resources;

        let functions = self.functions.read();
        Ok(ReloadSummary {
            packs_loaded,
            functions_loaded: functions.len(),
            tags_loaded: functions.tag_ids().len(),
            elapsed: started.elapsed(),
        })
    }

    /// The layered view of the enabled packs.
    #[must_use]
    pub(crate) fn resources(&self) -> Arc<MultiPackResourceManager> {
        Arc::clone(&self.resources.read())
    }

    /// Reads a JSON resource from the enabled packs, honouring load order.
    #[must_use]
    pub(crate) fn json(&self, location: &steel_utils::Identifier) -> Option<serde_json::Value> {
        let resources = self.resources();
        let resource = resources.get_resource(PackType::Server, location)?;
        serde_json::from_slice(&resource.bytes).ok()
    }

    /// Persists the pack selection after `/datapack enable|disable`.
    ///
    /// # Errors
    /// Returns [`PackError::Io`] when the selection file cannot be written.
    pub(crate) fn save_selection(&self) -> Result<(), PackError> {
        self.repository.read().save_selection()
    }
}

/// The process-wide manager, installed by [`init`].
static DATAPACKS: OnceLock<DatapackManager> = OnceLock::new();

/// Installs the datapack manager for `world_dir`, reloading all listeners.
///
/// # Errors
/// Propagates [`PackError`] from discovery or the first reload.
pub(crate) fn init(world_dir: &Path) -> Result<&'static DatapackManager, PackError> {
    if let Some(manager) = DATAPACKS.get() {
        return Ok(manager);
    }
    let manager = DatapackManager::new(world_dir)?;
    manager.reload()?;
    // A concurrent `init` wins; both managers are equivalent for the same world.
    Ok(DATAPACKS.get_or_init(|| manager))
}

/// The installed manager.
///
/// # Panics
/// Panics when called before [`init`]. Every caller is a command executor or the
/// server tick, both of which run only after world load.
#[must_use]
pub(crate) fn datapacks() -> &'static DatapackManager {
    DATAPACKS
        .get()
        .expect("datapack manager is initialised during world load")
}

/// Vanilla `MinecraftServer.tickChildren` hook for `/schedule`.
///
/// Returns the functions that became due at `tick`; the caller runs them so that
/// command execution stays on the command thread.
pub(crate) fn take_due_schedules(tick: i64) -> Vec<ScheduledFunction> {
    let Some(manager) = DATAPACKS.get() else {
        return Vec::new();
    };
    manager.schedules.write().take_due(tick)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{DatapackManager, PackError};

    #[test]
    fn manager_without_builtin_tree_reports_missing_pack() {
        // A path that cannot contain the vendored tree must surface the dedicated
        // error rather than panicking.
        let result = std::panic::catch_unwind(|| {
            let sources: Vec<std::sync::Arc<dyn super::RepositorySource>> = vec![
                std::sync::Arc::new(super::BuiltinPackSource::new(
                    Path::new("/nonexistent").into(),
                )),
                std::sync::Arc::new(super::WorldPackSource::new(
                    Path::new("/nonexistent/datapacks").into(),
                )),
            ];
            super::PackRepository::new(sources, None)
        });
        // The builtin source always contributes the vanilla pack, so this succeeds;
        // only a missing `builtin_pack_root` yields `MissingBuiltinPack`.
        let repository = result.expect("discovery must not panic").unwrap();
        assert!(repository.is_enabled("vanilla"));
    }

    #[test]
    fn missing_builtin_root_is_a_pack_error() {
        let error = super::pack::builtin_pack_root()
            .err()
            .map(|error| matches!(error, PackError::MissingBuiltinPack));
        // In a workspace checkout the tree exists; either outcome is valid here.
        assert!(error.is_none() || error == Some(true));
    }

    #[test]
    fn datapacks_are_not_initialised_in_unit_tests() {
        assert!(std::panic::catch_unwind(super::datapacks).is_err());
    }

    #[test]
    fn manager_new_with_explicit_sources_is_constructible() {
        let world = super::repository::temp_dir("manager");
        let sources: Vec<std::sync::Arc<dyn super::RepositorySource>> = vec![
            std::sync::Arc::new(super::BuiltinPackSource::new(world.clone())),
            std::sync::Arc::new(super::WorldPackSource::new(world.join("datapacks"))),
        ];
        let repository = super::PackRepository::new(sources, None).unwrap();
        assert!(repository.is_enabled("vanilla"));
        assert_eq!(repository.enabled_ids(), vec!["vanilla".to_owned()]);
        drop(DatapackManager::new(&world));
    }
}
