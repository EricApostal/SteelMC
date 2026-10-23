//! Zombie family entity implementations (vanilla
//! `net.minecraft.world.entity.monster.zombie`).

pub mod drowned;
pub mod husk;
pub mod zombie;
pub mod zombie_villager;

pub use drowned::DrownedEntity;
pub use husk::HuskEntity;
pub use zombie::ZombieEntity;
pub use zombie_villager::ZombieVillagerEntity;

#[cfg(test)]
mod tests;
