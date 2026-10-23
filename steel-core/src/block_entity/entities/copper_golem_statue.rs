//! `CopperGolemStatueBlockEntity`
//! (`net.minecraft.world.level.block.entity.CopperGolemStatueBlockEntity`).
//!
//! Vanilla's statue entity stores nothing of its own: the golem's custom name lives in the
//! block entity's data components, and `createStatue`/`removeStatue` convert between a live
//! `CopperGolem` and the statue. Steel keeps the custom-name component through the standard
//! component map, so the entity only needs the golem conversion hooks.

use std::sync::Weak;

use simdnbt::borrow::BaseNbtCompound as BorrowedNbtCompound;
use simdnbt::owned::NbtCompound;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// `CopperGolemStatueBlockEntity`.
pub struct CopperGolemStatueBlockEntity {
    base: BlockEntityBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CopperGolemStatueBlockEntity`.
unsafe impl DowncastType for CopperGolemStatueBlockEntity {
    const TYPE_KEY: DowncastTypeKey =
        DowncastTypeKey::new("steel:block_entity/copper_golem_statue");
}

impl CopperGolemStatueBlockEntity {
    /// Creates a copper-golem statue block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::COPPER_GOLEM_STATUE,
                level,
                pos,
                state,
            ),
        }
    }

    /// Vanilla `CopperGolemStatueBlockEntity.removeStatue`: converts the statue back into a live
    /// copper golem at the block centre with the statue's facing.
    ///
    /// Returns `None` while the entity lane has no `COPPER_GOLEM` factory.
    pub fn remove_statue(&self) -> Option<crate::entity::SharedEntity> {
        // TODO(Entities): create a `COPPER_GOLEM` at the block centre with the block's facing,
        // copy the stored custom name and play its spawn sound (vanilla
        // `CopperGolemStatueBlockEntity.removeStatue` + `initCopperGolem`). Blocked on the entity
        // lane registering a `vanilla_entities::COPPER_GOLEM` factory.
        None
    }
}

impl BlockEntity for CopperGolemStatueBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, _nbt: &BorrowedNbtCompound<'_>) {
        // Vanilla persists only the component map (`CustomName`), which the base block entity
        // already round-trips through `load_components`/`save_additional`.
    }

    fn save_additional(&self, _nbt: &mut NbtCompound) {
        // See `load_additional`: the statue has no additional payload of its own.
    }
}
