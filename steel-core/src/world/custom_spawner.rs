//! Vanilla `CustomSpawner`: per-level spawners ticked by the chunk cache.

use std::pin::Pin;
use std::sync::Arc;

use steel_utils::saved_data::SavedDataName;

use crate::world::World;

/// Future returned by [`CustomSpawner::save`].
pub type CustomSpawnerSaveFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<()>> + Send + 'a>>;

/// Vanilla `CustomSpawner`: a spawner driven once per level tick.
///
/// Vanilla returns `Optional<SavedDataName>` so each spawner persists its own
/// counters; Steel keeps the same contract with an explicit async save.
pub trait CustomSpawner: Send + Sync {
    /// Vanilla `CustomSpawner.tick(ServerLevel, boolean)`.
    ///
    /// The `spawnEnemies` flag is not modeled: the level tick does not currently
    /// track it and the registered spawners ignore it.
    fn tick(&mut self, world: &Arc<World>);

    /// Saved-data name this spawner persists, if any.
    fn saved_data_name(&self) -> Option<SavedDataName> {
        None
    }

    /// Persists this spawner's counters.
    fn save<'a>(&'a self, world: &'a World) -> CustomSpawnerSaveFuture<'a> {
        let _ = world;
        Box::pin(async { Ok(()) })
    }
}
