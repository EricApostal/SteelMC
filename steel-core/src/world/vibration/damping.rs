//! `#dampens_vibrations` / `#occludes_vibration_signals` filters.
//!
//! Vanilla `Entity.dampensVibrations` is `false` for most entities, `true` for the warden and
//! `item.is(ItemTags.DAMPENS_VIBRATIONS)` for item entities. `BlockState.is(BlockTags.DAMPENS_VIBRATIONS)`
//! covers the wool family, which also makes up `#occludes_vibration_signals`.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_utils::{BlockStateId, Downcast as _};

use crate::entity::Entity;
use crate::entity::entities::ItemEntity;
use crate::world::World;

/// Vanilla `BlockState.is(BlockTags.DAMPENS_VIBRATIONS)`.
#[must_use]
pub fn state_dampens_vibrations(state: BlockStateId) -> bool {
    state.get_block().has_tag(&BlockTag::DAMPENS_VIBRATIONS)
}

/// Vanilla `BlockState.is(BlockTags.OCCLUDES_VIBRATION_SIGNALS)` used by `Listener.isOccluded`.
#[must_use]
pub fn occludes_vibrations(state: BlockStateId) -> bool {
    state
        .get_block()
        .has_tag(&BlockTag::OCCLUDES_VIBRATION_SIGNALS)
}

/// Vanilla `Entity.dampensVibrations`: the warden and wool-carrying item entities.
///
/// Vanilla declares the hook on `Entity` (default `false`), so entities are resolved without a
/// trait method: the `ItemEntity` case reads the carried stack's `#dampens_vibrations` item tag.
#[must_use]
pub fn entity_dampens_vibrations(_world: &World, entity: &dyn Entity) -> bool {
    if entity.entity_type() == &vanilla_entities::WARDEN {
        return true;
    }
    entity
        .downcast_ref::<ItemEntity>()
        .is_some_and(|item| item.get_item().item().has_tag(&ItemTag::DAMPENS_VIBRATIONS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    #[test]
    fn wool_dampens_and_occludes_while_stone_does_not() {
        init_vanilla_registry();
        assert!(state_dampens_vibrations(
            vanilla_blocks::WHITE_WOOL.default_state()
        ));
        assert!(occludes_vibrations(
            vanilla_blocks::WHITE_WOOL.default_state()
        ));
        assert!(!state_dampens_vibrations(
            vanilla_blocks::STONE.default_state()
        ));
        assert!(!occludes_vibrations(vanilla_blocks::STONE.default_state()));
    }
}
