//! Vanilla `world.level.levelgen` custom spawners.
//!
//! Vanilla registers these per level from `MinecraftServer`'s overworld spawner
//! list; Steel registers the same set from the server bootstrap through
//! [`World::register_custom_spawner`](crate::world::World::register_custom_spawner).

pub mod cat;
pub mod phantom;
pub mod village_siege;

pub use cat::CatSpawner;
pub use phantom::PhantomSpawner;
pub use village_siege::VillageSiege;
