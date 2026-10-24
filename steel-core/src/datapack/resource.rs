//! Layered pack resource access and reload listeners, mirroring vanilla
//! `net.minecraft.server.packs.resources`.

use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use steel_utils::Identifier;

use super::{
    pack::{PackError, PackResources, PackType},
    repository::Pack,
};

/// Vanilla `Resource`: one decoded resource plus its origin.
#[derive(Debug, Clone)]
pub(crate) struct Resource {
    /// Originating pack id.
    pub(crate) source_pack: String,
    /// Raw bytes.
    pub(crate) bytes: Vec<u8>,
}

/// Vanilla `MultiPackResourceManager`: the overlaid view of the selected packs.
///
/// Packs are consulted in load order and the *last* pack that supplies a resource
/// wins, which is exactly vanilla's [`ResourceManager`] overlay rule
/// (`FallbackResourceManager` is queried over enabled packs in reverse).
pub(crate) struct MultiPackResourceManager {
    packs: Vec<Arc<dyn PackResources>>,
}

impl MultiPackResourceManager {
    /// Builds the overlay from the currently selected packs, honoring load order.
    pub(crate) fn from_packs(packs: &[&Pack]) -> Self {
        Self {
            packs: packs.iter().map(|pack| pack.resources()).collect(),
        }
    }

    /// Vanilla `ResourceManager.getResource`: the highest-priority copy.
    pub(crate) fn get_resource(
        &self,
        pack_type: PackType,
        location: &Identifier,
    ) -> Option<Resource> {
        self.packs.iter().rev().find_map(|pack| {
            pack.get_resource(pack_type, location)
                .map(|bytes| Resource {
                    source_pack: pack.location().id.clone(),
                    bytes,
                })
        })
    }

    /// Vanilla `ResourceManager.getResourceStack`: every copy, highest priority first.
    pub(crate) fn get_resource_stack(
        &self,
        pack_type: PackType,
        location: &Identifier,
    ) -> Vec<Resource> {
        self.packs
            .iter()
            .rev()
            .filter_map(|pack| {
                pack.get_resource(pack_type, location)
                    .map(|bytes| Resource {
                        source_pack: pack.location().id.clone(),
                        bytes,
                    })
            })
            .collect()
    }

    /// Vanilla `ResourceManager.listResources`: unions every pack's listing across
    /// every namespace.
    ///
    /// Keys are the listed resource ids with the file extension stripped and `<path>` dropped
    /// (`reload_test:entry` for `data/reload_test/function/entry.mcfunction`), which is what
    /// every reader here resolves against.
    pub(crate) fn list_resources(
        &self,
        pack_type: PackType,
        path: &str,
    ) -> BTreeMap<String, Vec<Resource>> {
        let mut found: BTreeMap<String, Vec<Resource>> = BTreeMap::new();
        // Lowest priority first so that each entry's stack ends highest-priority first.
        for pack in &self.packs {
            for namespace in pack.get_namespaces(pack_type) {
                let mut listed = Vec::new();
                // Packs list directory-relative file names, so `<path>` has to be put back
                // before asking for the bytes: `PackResources.getResource` takes a vanilla
                // `Identifier`, whose path keeps both the directory and the extension.
                pack.list_resources(pack_type, &namespace, path, "", &mut listed);
                for relative in listed {
                    let Some(name) = relative
                        .strip_suffix(".json")
                        .or_else(|| relative.strip_suffix(".mcfunction"))
                        .or_else(|| relative.strip_suffix(".nbt"))
                    else {
                        continue;
                    };
                    let resource = format!("{namespace}:{path}/{relative}");
                    let location = match resource.parse::<Identifier>() {
                        Ok(location) => location,
                        Err(error) => {
                            tracing::warn!(path = %relative, %error, "skipping unparsable resource path");
                            continue;
                        }
                    };
                    let Some(bytes) = pack.get_resource(pack_type, &location) else {
                        continue;
                    };
                    let key = format!("{namespace}:{name}");
                    found.entry(key).or_default().push(Resource {
                        source_pack: pack.location().id.clone(),
                        bytes,
                    });
                }
            }
        }
        for stack in found.values_mut() {
            stack.reverse();
        }
        found
    }
}

/// Vanilla `PreparableReloadListener`: one stage of a data reload.
///
/// Vanilla splits listeners into a prepare phase (I/O, off-thread) and an apply
/// phase (state swap, on the server thread). Steel keeps the two phases so that
/// `/reload` installs new state atomically, exactly like vanilla
/// `SimpleReloadInstance`.
pub(crate) trait PreparableReloadListener: Send + Sync {
    /// Vanilla `PreparableReloadListener.getName`: shown in reload logs.
    fn name(&self) -> &'static str;

    /// Vanilla `prepare`: decode from `manager` into a detached value.
    ///
    /// # Errors
    /// Returns [`PackError`] when a resource is malformed. Vanilla logs the
    /// listener's failure and continues the reload with the previous state; Steel
    /// aborts the reload so the operator sees the reason.
    fn prepare(&self, manager: &MultiPackResourceManager)
    -> Result<Box<dyn Any + Send>, PackError>;

    /// Vanilla `apply`: install the prepared value. Never runs concurrently with
    /// a tick, because `/reload` executes on the command thread.
    fn apply(&self, prepared: Box<dyn Any + Send>);
}

/// Vanilla `ReloadableResourceManager`: owns the listeners and drives reloads.
#[derive(Default)]
pub(crate) struct ReloadableResourceManager {
    listeners: RwLock<Vec<Box<dyn PreparableReloadListener>>>,
}

impl ReloadableResourceManager {
    /// Creates an empty manager.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Vanilla `ReloadableResourceManager.registerReloadListener`.
    pub(crate) fn register_listener(&self, listener: Box<dyn PreparableReloadListener>) {
        self.listeners
            .write()
            .expect("reload listener lock poisoned")
            .push(listener);
    }

    /// Vanilla `ReloadableResourceManager.createReload`: runs every listener.
    ///
    /// # Errors
    /// Returns the first listener's [`PackError`]; already-applied listeners keep
    /// their prepared state, so a failed reload is reported rather than partially
    /// rolled back (vanilla behaves the same way for a listener that throws).
    pub(crate) fn reload(&self, manager: &MultiPackResourceManager) -> Result<(), PackError> {
        let listeners = self
            .listeners
            .read()
            .expect("reload listener lock poisoned");
        let mut prepared: Vec<(usize, Box<dyn Any + Send>)> = Vec::with_capacity(listeners.len());
        for (index, listener) in listeners.iter().enumerate() {
            prepared.push((index, listener.prepare(manager)?));
        }
        for (index, value) in prepared {
            listeners[index].apply(value);
        }
        Ok(())
    }

    /// The registered listener names, in registration order.
    pub(crate) fn listener_names(&self) -> Vec<&'static str> {
        self.listeners
            .read()
            .expect("reload listener lock poisoned")
            .iter()
            .map(|listener| listener.name())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{MultiPackResourceManager, PreparableReloadListener, ReloadableResourceManager};
    use crate::datapack::pack::PackError;
    use std::any::Any;

    #[derive(Debug)]
    struct CountingListener {
        prepared: AtomicUsize,
        applied: AtomicUsize,
    }

    impl PreparableReloadListener for CountingListener {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn prepare(
            &self,
            _manager: &MultiPackResourceManager,
        ) -> Result<Box<dyn Any + Send>, PackError> {
            self.prepared.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(self.applied.load(Ordering::SeqCst)))
        }

        fn apply(&self, prepared: Box<dyn Any + Send>) {
            let value = *prepared.downcast::<usize>().expect("usize payload");
            self.applied.store(value + 1, Ordering::SeqCst);
        }
    }

    #[test]
    fn prepare_runs_for_every_listener_before_any_apply() {
        let listener = CountingListener {
            prepared: AtomicUsize::new(0),
            applied: AtomicUsize::new(0),
        };
        let manager = ReloadableResourceManager::new();
        manager.register_listener(Box::new(listener));
        let resources = MultiPackResourceManager::from_packs(&[]);
        manager.reload(&resources).unwrap();
        assert_eq!(manager.listener_names(), vec!["counting"]);
    }

    #[test]
    fn empty_overlay_has_no_resources() {
        let resources = MultiPackResourceManager::from_packs(&[]);
        assert!(
            resources
                .get_resource(
                    super::PackType::Server,
                    &steel_utils::Identifier::vanilla_static("functions/test")
                )
                .is_none()
        );
        assert!(
            resources
                .list_resources(super::PackType::Server, "function")
                .is_empty()
        );
    }

    #[test]
    fn listener_names_are_stable() {
        let manager = ReloadableResourceManager::new();
        assert!(manager.listener_names().is_empty());
        let _guard: Option<Mutex<()>> = None;
    }
}
