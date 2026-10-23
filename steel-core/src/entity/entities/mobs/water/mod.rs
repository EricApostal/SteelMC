//! Vanilla water creatures: `Cod`, `Salmon`, `TropicalFish` and `Pufferfish`.
//!
//! Every type here extends vanilla `AbstractFish extends WaterAnimal extends
//! PathfinderMob`; Steel has no inheritance, so the shared `WaterAnimal`,
//! `AbstractFish` and `AbstractSchoolingFish` behaviour lives in
//! [`abstract_fish`] as free functions plus the [`abstract_fish::SchoolingState`]
//! runtime state each schooling fish owns.

pub mod abstract_fish;
pub mod cod;
pub mod pufferfish;
pub mod salmon;
pub mod tropical_fish;

pub use cod::CodEntity;
pub use pufferfish::PufferfishEntity;
pub use salmon::SalmonEntity;
pub use tropical_fish::TropicalFishEntity;
