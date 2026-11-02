//! Global registry and behavior initialization.

use std::sync::Once;
use std::time::Instant;

use steel_registry::Registry;
use steel_registry::init_vanilla_registry_with;

use crate::behavior::init_behaviors_with;
use crate::behavior::{BlockBehaviorRegistry, ItemBehaviorRegistry};
use crate::block_entity::BlockEntityRegistry;
use crate::block_entity::init_block_entities_with;
use crate::entity::EntityRegistry;
use crate::entity::init_entities_with;

/// Initializes the vanilla registry and the behavior registries.
///
/// Idempotent, so an embedder loading several worlds in one process bootstraps once.
pub fn init_globals() {
    init_globals_with(|_| {}, |_, _| {}, |_| {}, |_| {});
}

/// Bootstraps the server with extension hooks in each phase.
///
/// `content` runs before the registry is frozen; `behaviors`, `block_entities`
/// and `entities` run after the registry is frozen but before their respective
/// registries are published.
pub fn init_globals_with<Content, Behaviors, BlockEntities, Entities>(
    content: Content,
    behaviors: Behaviors,
    block_entities: BlockEntities,
    entities: Entities,
) where
    Content: FnOnce(&mut Registry),
    Behaviors: FnMut(&mut BlockBehaviorRegistry, &mut ItemBehaviorRegistry),
    BlockEntities: FnOnce(&mut BlockEntityRegistry),
    Entities: FnOnce(&mut EntityRegistry),
{
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        let start = Instant::now();
        init_vanilla_registry_with(content);
        log::info!("Vanilla registry loaded in {:?}", start.elapsed());
        init_behaviors_with(behaviors);
        init_block_entities_with(block_entities);
        init_entities_with(entities);
        log::info!("Behavior registries initialized");
    });
}
