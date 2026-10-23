//! Behavior for vanilla's miscellaneous "special" block classes.
//!
//! These mirror `net.minecraft.world.level.block` classes that do not fit the
//! vegetation/redstone/container families: explosives, spawner-family blocks,
//! decoration-adjacent entity blocks, and the admin-only blocks.

mod admin_blocks;
mod bonemealable_feature_placer_block;
mod creaking_heart_block;
mod decorated_pot_block;
mod dried_ghast_block;
mod frogspawn_block;
mod infested_block;
mod shelf_block;
mod sniffer_egg_block;
mod spawner_block;
mod tnt_block;
mod trial_spawner_block;
mod vault_block;

pub use admin_blocks::*;
pub use bonemealable_feature_placer_block::*;
pub use creaking_heart_block::*;
pub use decorated_pot_block::*;
pub use dried_ghast_block::*;
pub use frogspawn_block::*;
pub use infested_block::*;
pub use shelf_block::*;
pub use sniffer_egg_block::*;
pub use spawner_block::*;
pub use tnt_block::*;
pub use trial_spawner_block::*;
pub use vault_block::*;
