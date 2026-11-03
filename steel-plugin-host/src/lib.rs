//! Loader and Rust adapters for Steel extension libraries.
//!
//! Extensions are `cdylib`s exporting `steel_extension_entry`. They register
//! content during the [`STEEL_LEVEL_CONTENT`] phase, behaviour hooks and event
//! listeners during [`STEEL_LEVEL_BEHAVIOR`], and runtime callbacks during
//! [`STEEL_LEVEL_RUNTIME`]. See `steel-abi` for the C surface and
//! `steel-plugin-gen` for the generated schema/bindings.

#![allow(
    missing_docs,
    reason = "the generated API schema and C header document the extension surface"
)]
#![allow(
    clippy::absolute_paths,
    clippy::multiple_unsafe_ops_per_block,
    reason = "FFI plumbing spells ABI paths explicitly and performs several pointer operations per boundary call"
)]

mod adapter;
mod behavior;
mod convert;
mod interface;
mod state;

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use steel_abi::{STEEL_ABI_VERSION, SteelExtension, SteelExtensionEntryFn};
use steel_core::behavior::{BlockBehaviorRegistry, ItemBehaviorRegistry};
use steel_core::block_entity::BlockEntityRegistry;
use steel_core::entity::EntityRegistry;
use steel_registry::Registry;
use steel_registry::{RegistryEntry, RegistryExt};
use steel_utils::locks::SyncMutex;

use crate::behavior::{ForeignBlockBehavior, ForeignItemBehavior};
use crate::interface::INTERFACE;
use crate::state::HOST;

struct LoadedExtension {
    _library: libloading::Library,
    extension: *const SteelExtension,
    id: String,
    path: Arc<str>,
}

// SAFETY: `extension` points at a static manifest owned by the loaded library.
// The library is kept alive in the same struct and all hook tables are only
// called through this host.
unsafe impl Send for LoadedExtension {}

// SAFETY: see the `Send` implementation above.
unsafe impl Sync for LoadedExtension {}

static LOADED: LazyLock<SyncMutex<Vec<LoadedExtension>>> =
    LazyLock::new(|| SyncMutex::new(Vec::new()));

static ATTACHED_BLOCKS: LazyLock<SyncMutex<Vec<steel_utils::Identifier>>> =
    LazyLock::new(|| SyncMutex::new(Vec::new()));

/// Errors produced while loading an extension.
#[derive(Debug)]
pub enum ExtensionError {
    Io(std::io::Error),
    Library(libloading::Error),
    MissingEntry(PathBuf),
    NullManifest(PathBuf),
    InvalidManifest(String),
    IncompatibleApi {
        id: String,
        minimum: u32,
        maximum: u32,
        host: u32,
    },
    Duplicate(String),
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "extension I/O error: {error}"),
            Self::Library(error) => write!(formatter, "failed to load extension library: {error}"),
            Self::MissingEntry(path) => write!(
                formatter,
                "extension `{}` does not export steel_extension_entry",
                path.display()
            ),
            Self::NullManifest(path) => write!(
                formatter,
                "extension `{}` returned a null manifest",
                path.display()
            ),
            Self::InvalidManifest(message) => {
                write!(formatter, "invalid extension manifest: {message}")
            }
            Self::IncompatibleApi {
                id,
                minimum,
                maximum,
                host,
            } => write!(
                formatter,
                "extension `{id}` requires API {minimum}..={maximum}, host provides {host}"
            ),
            Self::Duplicate(id) => write!(formatter, "extension `{id}` is already loaded"),
        }
    }
}

impl std::error::Error for ExtensionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Library(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ExtensionError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<libloading::Error> for ExtensionError {
    fn from(value: libloading::Error) -> Self {
        Self::Library(value)
    }
}

fn manifest_string(view: steel_abi::SteelStrView, field: &str) -> Result<String, ExtensionError> {
    // SAFETY: the manifest is owned by the loaded library.
    let value = unsafe { view.as_str() }
        .ok_or_else(|| ExtensionError::InvalidManifest(format!("{field} is not valid UTF-8")))?;
    if value.is_empty() {
        return Err(ExtensionError::InvalidManifest(format!("{field} is empty")));
    }
    Ok(value.to_owned())
}

/// Runs `f` with the host extension context pointing at `loaded`, so ABI calls
/// such as `extension_path` and `register_command` can identify their caller.
fn with_extension_context<R>(loaded: &LoadedExtension, f: impl FnOnce() -> R) -> R {
    HOST.set_extension_context(
        Arc::clone(&loaded.path),
        Some(Arc::from(loaded.id.as_str())),
    );
    let result = f();
    HOST.clear_extension_context();
    result
}

/// Handle used by the server to load extensions and drive the init phases.
#[derive(Default)]
pub struct ExtensionManager;

impl ExtensionManager {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Loads one extension library. The library stays resident for the process
    /// lifetime; unloading is not supported because registry entries are
    /// `&'static` and event listeners cannot be revoked safely.
    pub fn load(&self, path: impl AsRef<Path>) -> Result<(), ExtensionError> {
        let path = path.as_ref();
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let path_text: Arc<str> = Arc::from(canonical.to_string_lossy().as_ref());
        // SAFETY: loading arbitrary native code is inherently unsafe; callers
        // are responsible for trusting the extension.
        unsafe {
            let library = libloading::Library::new(&canonical)?;
            let entry: libloading::Symbol<'_, SteelExtensionEntryFn> = library
                .get(b"steel_extension_entry\0")
                .map_err(|_| ExtensionError::MissingEntry(path.to_path_buf()))?;
            HOST.set_extension_context(Arc::clone(&path_text), None);
            let manifest = entry(std::ptr::from_ref(&INTERFACE));
            HOST.clear_extension_context();
            if manifest.is_null() {
                return Err(ExtensionError::NullManifest(path.to_path_buf()));
            }
            let manifest = &*manifest;
            let id = manifest_string(manifest.id, "id")?;
            let minimum = manifest.api_min;
            let maximum = manifest.api_max.max(minimum);
            if STEEL_ABI_VERSION < minimum || STEEL_ABI_VERSION > maximum {
                return Err(ExtensionError::IncompatibleApi {
                    id,
                    minimum,
                    maximum,
                    host: STEEL_ABI_VERSION,
                });
            }
            if LOADED.lock().iter().any(|loaded| loaded.id == id) {
                return Err(ExtensionError::Duplicate(id));
            }
            LOADED.lock().push(LoadedExtension {
                _library: library,
                extension: manifest,
                id,
                path: path_text,
            });
        }
        Ok(())
    }

    /// Loads every shared library in `directory` in deterministic path order.
    pub fn load_dir(&self, directory: impl AsRef<Path>) -> Result<usize, ExtensionError> {
        let directory = directory.as_ref();
        if !directory.exists() {
            return Ok(0);
        }
        let mut paths = Vec::new();
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let supported = path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| matches!(extension, "so" | "dylib" | "dll"));
            if supported {
                paths.push(path);
            }
        }
        paths.sort();
        let mut loaded = 0;
        for path in paths {
            self.load(path)?;
            loaded += 1;
        }
        Ok(loaded)
    }

    /// Runs the content phase. The registry is mutable and not yet frozen.
    pub fn content_phase(&self, registry: &mut Registry) {
        HOST.set_phase(steel_abi::STEEL_LEVEL_CONTENT);
        for loaded in LOADED.lock().iter() {
            // SAFETY: the manifest is valid for the lifetime of the library.
            if let Some(initialize) = unsafe { &*loaded.extension }.initialize {
                interface::with_registry(registry, || {
                    // SAFETY: the extension owns its initialization state.
                    let status = with_extension_context(loaded, || unsafe {
                        initialize(
                            steel_abi::STEEL_LEVEL_CONTENT,
                            (*loaded.extension).user_data,
                        )
                    });
                    if status != steel_abi::STEEL_OK {
                        state::set_last_error(format!(
                            "extension `{}` failed content initialization with status {status}",
                            loaded.id
                        ));
                    }
                });
            }
        }
    }

    /// Runs the behaviour phase and installs extension block/item behaviours.
    pub fn behavior_phase(
        &self,
        blocks: &mut BlockBehaviorRegistry,
        items: &mut ItemBehaviorRegistry,
    ) {
        HOST.set_phase(steel_abi::STEEL_LEVEL_BEHAVIOR);
        for loaded in LOADED.lock().iter() {
            // SAFETY: see `content_phase`.
            if let Some(initialize) = unsafe { &*loaded.extension }.initialize {
                // SAFETY: the extension owns its initialization state.
                let status = with_extension_context(loaded, || unsafe {
                    initialize(
                        steel_abi::STEEL_LEVEL_BEHAVIOR,
                        (*loaded.extension).user_data,
                    )
                });
                if status != steel_abi::STEEL_OK {
                    state::set_last_error(format!(
                        "extension `{}` failed behavior initialization with status {status}",
                        loaded.id
                    ));
                }
            }
        }

        let registrations = HOST.registrations.lock();
        for registration in &registrations.blocks {
            let Some(block) = steel_registry::REGISTRY.blocks.by_key(&registration.key) else {
                state::set_last_error(format!(
                    "block `{}` vanished before behavior attachment",
                    registration.key
                ));
                continue;
            };
            blocks.set_behavior(
                block,
                Box::new(ForeignBlockBehavior::new(block, registration.hooks)),
            );
            ATTACHED_BLOCKS.lock().push(registration.key.clone());
        }
        for registration in &registrations.items {
            let Some(item) = steel_registry::REGISTRY.items.by_key(&registration.key) else {
                state::set_last_error(format!(
                    "item `{}` vanished before behavior attachment",
                    registration.key
                ));
                continue;
            };
            items.set_behavior(item, Box::new(ForeignItemBehavior::new(registration.hooks)));
        }
    }

    /// Installs extension block-entity factories.
    pub fn block_entity_phase(&self, registry: &mut BlockEntityRegistry) {
        let registrations = HOST.registrations.lock();
        for registration in &registrations.block_entities {
            let Some(kind) = steel_registry::REGISTRY
                .block_entity_types
                .by_key(&registration.key)
            else {
                state::set_last_error(format!(
                    "block entity type `{}` vanished before factory attachment",
                    registration.key
                ));
                continue;
            };
            registry.register(kind, adapter::foreign_block_entity_factory);
            adapter::bind_block_entity_hooks(kind.id(), registration.hooks);
            for block in kind.valid_blocks {
                adapter::bind_block_entity_type(block.id(), kind.id());
            }
        }
    }

    /// Installs extension entity factories.
    pub fn entity_phase(&self, registry: &mut EntityRegistry) {
        let registrations = HOST.registrations.lock();
        for registration in &registrations.entities {
            let Some(kind) = steel_registry::REGISTRY
                .entity_types
                .by_key(&registration.key)
            else {
                state::set_last_error(format!(
                    "entity type `{}` vanished before factory attachment",
                    registration.key
                ));
                continue;
            };
            registry.register(kind, adapter::foreign_entity_factory);
            registry.register_load(kind, adapter::foreign_entity_load_factory);
            adapter::bind_entity_hooks(kind.id(), registration.hooks);
        }
    }

    /// Runs the runtime phase.
    pub fn runtime_phase(&self) {
        HOST.set_phase(steel_abi::STEEL_LEVEL_RUNTIME);
        for loaded in LOADED.lock().iter() {
            // SAFETY: see `content_phase`.
            if let Some(initialize) = unsafe { &*loaded.extension }.initialize {
                // SAFETY: the extension owns its initialization state.
                let status = with_extension_context(loaded, || unsafe {
                    initialize(
                        steel_abi::STEEL_LEVEL_RUNTIME,
                        (*loaded.extension).user_data,
                    )
                });
                if status != steel_abi::STEEL_OK {
                    state::set_last_error(format!(
                        "extension `{}` failed runtime initialization with status {status}",
                        loaded.id
                    ));
                }
            }
        }
    }

    /// Tears down extensions in reverse level order. Registry entries remain
    /// registered for the process lifetime.
    pub fn shutdown(&self) {
        let loaded = LOADED.lock();
        for level in [
            steel_abi::STEEL_LEVEL_RUNTIME,
            steel_abi::STEEL_LEVEL_BEHAVIOR,
            steel_abi::STEEL_LEVEL_CONTENT,
        ] {
            for extension in loaded.iter().rev() {
                // SAFETY: the manifest is valid for the lifetime of the library.
                let deinitialize = unsafe { (*extension.extension).deinitialize };
                if let Some(deinitialize) = deinitialize {
                    // SAFETY: the extension owns its initialization state.
                    unsafe {
                        deinitialize(level, (*extension.extension).user_data);
                    }
                }
            }
        }
    }
}

/// Number of currently loaded extensions.
#[must_use]
pub fn loaded_extension_count() -> usize {
    LOADED.lock().len()
}

/// Identifiers of blocks whose behaviour was replaced by an extension.
#[must_use]
pub fn attached_block_identifiers() -> Vec<steel_utils::Identifier> {
    ATTACHED_BLOCKS.lock().clone()
}

/// Names of commands registered by extensions through the ABI.
#[must_use]
pub fn registered_command_names() -> Vec<String> {
    HOST.commands.lock().clone()
}
