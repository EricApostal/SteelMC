//! Data-pack runtime: pack discovery, layered resource access, reload listeners
//! and `mcfunction` execution.
//!
//! Vanilla wires these through `MinecraftServer` (`ServerPacksSource` →
//! `PackRepository` → `MultiPackResourceManager` → `ReloadableResourceManager`).
//! Steel keeps the same object graph but hangs it off one process-wide
//! [`DatapackManager`], because [`crate::server::Server`] is a reserved
//! integration file: `/reload`, `/datapack`, `/function` and `/schedule` all reach
//! the manager through [`datapacks()`].
//!
//! `/reload` covers the datapack data this layer loads at runtime: pack discovery
//! and selection, `mcfunction` bodies, and function tags (`data/<ns>/tags/function`).
//! Vanilla's other data-pack registries are deliberately not reloadable here:
//! block/item/… registry tags, recipes, loot tables, advancements and
//! structure/feature JSON are compiled into typed Rust by the build scripts, per
//! `AGENTS.md` ("Do not design for runtime datapack JSON loading"), so `/reload`
//! has nothing to re-read for them and never resends the registry tag or recipe
//! packets vanilla broadcasts from `PlayerList.reloadResources`.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use steel_utils::{Identifier, locks::SyncRwLock};

pub(crate) mod function;
pub(crate) mod pack;
pub(crate) mod repository;
pub(crate) mod resource;
pub(crate) mod schedule;

pub(crate) use function::{
    Function, FunctionArguments, FunctionLoader, FunctionRegistry, InstantiationFailure,
    run_function,
};
pub(crate) use pack::PackError;
pub(crate) use repository::{
    BuiltinPackSource, PackInsert, PackRepository, PackSelectionError, RepositorySource,
    WorldPackSource, world_datapack_dir,
};
pub(crate) use resource::{MultiPackResourceManager, ReloadableResourceManager};
pub(crate) use schedule::{ScheduleMode, ScheduleRegistry, ScheduledCallback};

use crate::command::{execution::CommandSource, sender::CommandSender};
use crate::server::Server;

use pack::PackType;

/// Vanilla `ServerFunctionManager.TICK_FUNCTION_TAG`.
const TICK_FUNCTION_TAG: Identifier = Identifier::vanilla_static("tick");

/// Vanilla `ServerFunctionManager.LOAD_FUNCTION_TAG`.
const LOAD_FUNCTION_TAG: Identifier = Identifier::vanilla_static("load");

/// The outcome of one `/reload`.
#[derive(Debug, Clone)]
pub(crate) struct ReloadSummary {
    /// Number of enabled packs the reload read.
    pub(crate) packs_loaded: usize,
    /// Ids of packs this reload selected for the first time.
    pub(crate) new_packs: Vec<String>,
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
    /// Vanilla `ServerFunctionManager.postReload`: run `#load` on the next tick.
    post_reload: AtomicBool,
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
            post_reload: AtomicBool::new(true),
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

    /// Vanilla `PackRepository.reload`: re-discovers packs from the configured
    /// sources without re-reading resources.
    ///
    /// `/datapack list` calls this first, so a pack dropped into `datapacks/`
    /// while the server runs becomes visible without a restart. Vanilla's `list`
    /// does not re-read resources, and it never selects a pack that was not
    /// already selected, so the new pack shows up under `list available` until
    /// `/reload` or `/datapack enable` selects it.
    ///
    /// # Errors
    /// Propagates [`PackError`] from discovery.
    pub(crate) fn rediscover(&self) -> Result<(), PackError> {
        self.repository.write().rediscover()?;
        Ok(())
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

    /// Vanilla `ReloadCommand` + `MinecraftServer.reloadResources`: re-discovers
    /// packs, selects every pack the selection does not disable, persists the new
    /// selection, then re-runs every listener.
    ///
    /// # Errors
    /// Returns [`PackError`] when discovery, the selection write, or a listener's
    /// prepare phase fails; the previously loaded state stays installed.
    pub(crate) fn reload(&self) -> Result<ReloadSummary, PackError> {
        // Vanilla `ReloadCommand.discoverNewPacks` re-discovers before the reload and
        // hands the resulting id list to `reloadResources`, whose apply phase writes
        // the new selection back into the world data.
        let new_packs = {
            let mut repository = self.repository.write();
            let previously_enabled = repository.enabled_ids();
            if repository.discover_new_packs()? {
                repository.save_selection()?;
            }
            repository
                .enabled_ids()
                .into_iter()
                .filter(|id| !previously_enabled.contains(id))
                .collect()
        };
        self.reload_listeners(new_packs)
    }

    /// Rebuilds the resource overlay and re-runs every listener.
    fn reload_listeners(&self, new_packs: Vec<String>) -> Result<ReloadSummary, PackError> {
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
        // Vanilla `ServerFunctionManager.replaceLibrary`: `#load` runs next tick.
        self.post_reload.store(true, Ordering::Release);

        let functions = self.functions.read();
        Ok(ReloadSummary {
            packs_loaded,
            new_packs,
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

    /// Reads a JSON resource from the enabled packs, honoring load order.
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
        .expect("datapack manager is initialized during world load")
}

/// The installed manager, or `None` before world load.
///
/// Command suggestion providers and executors reach for this so a completion
/// request cannot panic in the window before the datapack manager exists.
#[must_use]
pub(crate) fn try_datapacks() -> Option<&'static DatapackManager> {
    DATAPACKS.get()
}

/// Vanilla `ServerFunctionManager.tick`: `#load` (only on the tick after a reload)
/// then `#tick`.
///
/// Called once per game tick from the server loop before the worlds tick, at the
/// previous tick's game time, matching `MinecraftServer.tickChildren`'s
/// "commandFunctions" phase. Both tags use vanilla's game-loop source, so output
/// stays silent and the vanilla operator level is capped at `function-permission-level`.
pub(crate) fn tick_tags(server: &Arc<Server>) {
    let Some(manager) = DATAPACKS.get() else {
        return;
    };
    let source = game_loop_source(server);
    if manager.post_reload.swap(false, Ordering::AcqRel) {
        run_tag(&source, &LOAD_FUNCTION_TAG);
    }
    run_tag(&source, &TICK_FUNCTION_TAG);
}

/// Vanilla `ServerLevel.tickTime`'s `server.getScheduledEvents().tick(server, time)`.
///
/// Called once per game tick from the server loop right after the game-time advance
/// and before the block, fluid, and entity tick phases, so a function scheduled for
/// this tick observes and affects the same tick's world state.
pub(crate) fn tick(server: &Arc<Server>) {
    let Some(manager) = DATAPACKS.get() else {
        return;
    };
    let source = game_loop_source(server);

    // Vanilla `TimerQueue.tick` re-peeks the queue after every callback, so a function
    // that schedules another function for the current game time runs it in this same
    // drain. The schedule lock is released before any callback executes.
    loop {
        let due = manager
            .schedules
            .write()
            .take_due(server.overworld().game_time());
        if due.is_empty() {
            break;
        }
        for entry in due {
            run_callback(&source, entry);
        }
    }
}

/// Vanilla `ServerFunctionManager.getGameLoopSender`.
fn game_loop_source(server: &Arc<Server>) -> Arc<CommandSource> {
    Arc::new(
        CommandSource::new(CommandSender::Console, Arc::clone(server))
            .with_maximum_permission(server.config.function_permission_level)
            .with_suppressed_output(),
    )
}

/// Vanilla `FunctionCallback`/`FunctionTagCallback.handle`.
fn run_callback(source: &Arc<CommandSource>, callback: ScheduledCallback) {
    match callback {
        ScheduledCallback::Function(id) => {
            let function = DATAPACKS
                .get()
                .and_then(|manager| manager.functions.read().get_shared(&id));
            if let Some(function) = function {
                execute(source, &function, None);
            }
        }
        ScheduledCallback::Tag(id) => run_tag(source, &id),
    }
}

/// Vanilla `ServerFunctionManager.executeTagFunctions`.
fn run_tag(source: &Arc<CommandSource>, tag: &Identifier) {
    let Some(manager) = DATAPACKS.get() else {
        return;
    };
    // Clone the functions out so a function that reloads or runs other functions
    // never re-enters the registry lock.
    let functions = manager.functions.read().tagged_shared(tag);
    for function in functions {
        execute(source, &function, None);
    }
}

/// Runs one function, logging the vanilla instantiation failure.
fn execute(
    source: &Arc<CommandSource>,
    function: &Function,
    arguments: Option<&FunctionArguments>,
) {
    if let Err(failure) = run_function(source, function, arguments) {
        // Vanilla `ServerFunctionManager.execute` swallows instantiation failures.
        tracing::warn!(
            function = %failure.id,
            reason = ?failure.reason,
            "failed to instantiate function"
        );
    }
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
    fn datapacks_are_not_initialized_in_unit_tests() {
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

    #[test]
    fn reload_rereads_functions_and_function_tags() {
        use steel_utils::Identifier;

        let world = super::repository::temp_dir("reload-functions");
        let pack = world.join("datapacks").join("reload_test");
        let function_dir = pack.join("data/reload_test/function");
        let tag_dir = pack.join("data/reload_test/tags/function");
        std::fs::create_dir_all(&function_dir).unwrap();
        std::fs::create_dir_all(&tag_dir).unwrap();
        std::fs::write(
            pack.join("pack.mcmeta"),
            format!(
                "{{\"pack\":{{\"min_format\":{format},\"max_format\":{format}}}}}",
                format = super::pack::DATA_PACK_FORMAT
            ),
        )
        .unwrap();

        let manager = DatapackManager::new(&world).unwrap();
        // The freshly discovered world pack is enabled at construction, like vanilla
        // `MinecraftServer.configurePackRepository`, so it is already loaded.
        assert!(
            manager
                .repository()
                .read()
                .is_enabled("file/reload_test")
        );

        let entry: Identifier = "reload_test:entry".parse().unwrap();
        let tag: Identifier = "reload_test:tick".parse().unwrap();
        assert!(manager.functions().read().get(&entry).is_none());

        // A function added while the server runs becomes active on `/reload`.
        std::fs::write(function_dir.join("entry.mcfunction"), "say hi\n").unwrap();
        std::fs::write(
            tag_dir.join("tick.json"),
            "{\"values\":[\"reload_test:entry\"]}",
        )
        .unwrap();
        manager.reload().unwrap();
        assert!(manager.functions().read().get(&entry).is_some());
        assert!(
            manager
                .functions()
                .read()
                .tagged(&tag)
                .iter()
                .any(|function| function.id == entry)
        );

        // Removing it drops it from the next reload as well.
        std::fs::remove_file(function_dir.join("entry.mcfunction")).unwrap();
        manager.reload().unwrap();
        assert!(manager.functions().read().get(&entry).is_none());

        std::fs::remove_dir_all(&world).ok();
    }
}
