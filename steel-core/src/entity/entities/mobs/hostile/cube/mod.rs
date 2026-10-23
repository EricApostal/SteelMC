//! Slime family (vanilla `net.minecraft.world.entity.monster.cubemob`).

pub mod abstract_cube_mob;
pub mod magma_cube;
pub mod slime;
pub mod sulfur_cube;

pub use magma_cube::MagmaCubeEntity;
pub use slime::SlimeEntity;
pub use sulfur_cube::SulfurCubeEntity;
