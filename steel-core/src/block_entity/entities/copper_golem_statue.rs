//! `CopperGolemStatueBlockEntity`
//! (`net.minecraft.world.level.block.entity.CopperGolemStatueBlockEntity`).
//!
//! Vanilla's statue entity stores nothing of its own: the golem's custom name lives in the
//! block entity's data components, and `createStatue`/`removeStatue` convert between a live
//! `CopperGolem` and the statue. Steel keeps the custom-name component through the standard
//! component map, so the entity only needs the golem conversion hooks.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::BaseNbtCompound as BorrowedNbtCompound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::{sound_events, vanilla_block_entity_types, vanilla_entities};
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::{ENTITIES, Entity, SharedEntity, next_entity_id};
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

    /// Vanilla `CopperGolemStatueBlockEntity.createStatue`: stores the golem's custom name in
    /// the block entity's component map and marks the entity changed.
    pub fn create_statue(&self, golem: &dyn Entity) {
        let mut components = self.stored_components();
        components.set(CUSTOM_NAME, golem.custom_name());
        self.set_components(components);
        self.set_changed();
    }

    /// Vanilla `CopperGolemStatueBlockEntity.removeStatue` + `initCopperGolem`: builds the live
    /// copper golem the statue stands for, centered on the block with the statue's facing and
    /// carrying the statue's stored custom name.
    ///
    /// Like vanilla, the golem is only created here; the caller adds it to the world.
    pub fn remove_statue(&self) -> Option<SharedEntity> {
        let world = self.get_level()?;
        let pos = self.get_block_pos();
        let (center_x, _, center_z) = pos.get_center();
        let position = DVec3::new(center_x, f64::from(pos.y()), center_z);
        let entity = ENTITIES.create(
            &vanilla_entities::COPPER_GOLEM,
            next_entity_id(),
            position,
            Arc::downgrade(&world),
        )?;

        entity.set_custom_name(self.stored_components().get(CUSTOM_NAME));

        let yaw = self
            .get_block_state()
            .get_value(&BlockStateProperties::HORIZONTAL_FACING)
            .to_yaw();
        entity.snap_to(position, yaw, 0.0);
        if let Some(living) = entity.as_living_entity() {
            living.set_y_head_rot(yaw);
            living.set_y_body_rot(yaw);
        }
        entity.play_sound(&sound_events::ENTITY_COPPER_GOLEM_SPAWN, 1.0, 1.0);
        Some(entity)
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
