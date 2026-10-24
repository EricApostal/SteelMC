//! Spider family (vanilla `net.minecraft.world.entity.monster.spider`).

pub mod cave_spider;
#[expect(
    clippy::module_inception,
    reason = "vanilla mirrors one class per directory (spider); renaming would churn every consumer"
)]
pub mod spider;

pub use cave_spider::CaveSpiderEntity;
pub use spider::SpiderEntity;
