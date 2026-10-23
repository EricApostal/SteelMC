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
    /// Vanilla `CustomSpawner.tick(ServerLevel, boolean)` without the flag.
    ///
    /// The level always calls [`CustomSpawner::tick_with_settings`]; implement
    /// this one when the spawner does not read `spawnEnemies` (vanilla
    /// `WanderingTraderSpawner` ignores it).
    fn tick(&mut self, world: &Arc<World>);

    /// Vanilla `CustomSpawner.tick(ServerLevel, boolean spawnEnemies)`.
    ///
    /// `spawn_enemies` is `ServerChunkCache.spawnEnemies`, i.e.
    /// `ServerLevel.isSpawningMonsters()`. The default drops it and calls
    /// [`CustomSpawner::tick`], which matches the vanilla spawners that ignore
    /// the flag.
    fn tick_with_settings(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        let _ = spawn_enemies;
        self.tick(world);
    }

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
