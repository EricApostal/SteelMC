//! Skeleton family entity implementations (vanilla
//! `net.minecraft.world.entity.monster.skeleton`).

pub mod abstract_skeleton;
pub mod bogged;
pub mod parched;
pub mod skeleton;
pub mod stray;
pub mod wither_skeleton;

pub use bogged::BoggedEntity;
pub use parched::ParchedEntity;
pub use skeleton::SkeletonEntity;
pub use stray::StrayEntity;
pub use wither_skeleton::WitherSkeletonEntity;
