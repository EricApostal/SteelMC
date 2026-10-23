//! Zombie family entity implementations (vanilla
//! `net.minecraft.world.entity.monster.zombie`).

pub mod zombie;
pub mod zombie_villager;

pub use zombie::ZombieEntity;
pub use zombie_villager::ZombieVillagerEntity;

#[cfg(test)]
mod tests;
