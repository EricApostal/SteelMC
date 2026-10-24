//! Skeleton family entity implementations (vanilla
//! `net.minecraft.world.entity.monster.skeleton`).

pub mod abstract_skeleton;
pub mod bogged;
pub mod parched;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (skeleton); renaming would churn every consumer"
)]
pub mod skeleton;
pub mod stray;
pub mod wither_skeleton;

pub use bogged::BoggedEntity;
pub use parched::ParchedEntity;
pub use skeleton::SkeletonEntity;
pub use stray::StrayEntity;
pub use wither_skeleton::WitherSkeletonEntity;
