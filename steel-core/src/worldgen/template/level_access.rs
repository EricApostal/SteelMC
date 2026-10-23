//! Level-access abstraction for structure template placement.
//!
//! Vanilla's `StructureTemplate.placeInWorld` takes a live `ServerLevel` (a
//! `LevelAccessor`), so `/place template` can write straight into gameplay
//! chunks. Steel's template layer is currently written against
//! [`WorldGenRegion`](crate::worldgen::region::WorldGenRegion), which is a
//! generation-only view: it requires a `&WorldGenContext`, a `&ChunkStep` and a
//! `StaticCache2D<Arc<ChunkHolder>>`, and it enforces worldgen dependency and
//! write-radius rules. There is no way to construct one for a live chunk, and
//! that is deliberate.
//!
//! This trait captures exactly the eight level operations the template layer
//! uses (measured over `worldgen/template/**` and
//! `worldgen/structure/piece_placer/**`), so `place_in_world` and the piece
//! placers can be genericized over `L: TemplateLevelAccess` with
//! `WorldGenRegion` as the first implementor and `World` (once `SpawnerWorld`
//! lands `set_block_entity_data`, `add_fresh_entity`, and a stored
//! `Weak<World>`) as the second.
//!
//! This file is intentionally unwired: no `mod` declaration references it yet,
//! so the crate builds identically with or without it. Wiring (`mod
//! level_access;` + generic bounds on `place_in_world`) lands in one step once
//! `WorldgenDepth.WgJigsawVillage` clears the `placement.rs` verification
//! window and `SpawnerWorld` lands the three `World` primitives.
//!
//! One deliberate difference from vanilla, documented here so the genericizing
//! lane does not "fix" it: `set_block_state` on a live `World` must go through
//! the serialized world-mutation path (`World::set_block_with_limit`-family),
//! not a direct palette write, because block callbacks run after the state
//! claim. The `WorldGenRegion` path keeps its current direct-chunk behavior.

use std::sync::Weak;

use simdnbt::owned::NbtCompound;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_utils::{BlockPos, BlockStateId, types::UpdateFlags};

use crate::chunk::heightmap::HeightmapType;
use crate::entity::SharedEntity;
use crate::world::World;
use crate::worldgen::region::WorldGenRegion;

/// The eight level operations template and piece placement need.
///
/// Signatures mirror `WorldGenRegion` exactly (`worldgen/region.rs`):
/// `seed` (:281), `weak_world` (:287), `block_state` (:445),
/// `set_block_state` (:515), `set_block_entity_data` (:570),
/// `remove_block_entity` (:608), `add_fresh_entity` (:628), `height_at` (:716).
pub trait TemplateLevelAccess {
    /// Returns the world seed, for processor randomness.
    fn seed(&self) -> i64;

    /// Returns a weak handle to the owning world, for entity construction.
    fn weak_world(&self) -> Weak<World>;

    /// Returns the block state at `pos`.
    fn block_state(&self, pos: BlockPos) -> BlockStateId;

    /// Writes `state` at `pos` with vanilla update flags.
    ///
    /// On a live world this must use the serialized world-mutation path; in
    /// worldgen it keeps the current direct-chunk write.
    fn set_block_state(&self, pos: BlockPos, state: BlockStateId, flags: UpdateFlags) -> bool;

    /// Creates or replaces the block entity at `pos` from owned NBT.
    fn set_block_entity_data(
        &self,
        pos: BlockPos,
        block_entity_type: BlockEntityTypeRef,
        state: BlockStateId,
        nbt: NbtCompound,
    ) -> bool;

    /// Removes the block entity at `pos`.
    fn remove_block_entity(&self, pos: BlockPos) -> bool;

    /// Adds a template-spawned entity.
    fn add_fresh_entity(&self, entity: SharedEntity) -> bool;

    /// Returns the heightmap height at (`x`, `z`).
    fn height_at(&self, heightmap_type: HeightmapType, x: i32, z: i32) -> i32;
}

impl TemplateLevelAccess for WorldGenRegion<'_> {
    fn seed(&self) -> i64 {
        WorldGenRegion::seed(self)
    }

    fn weak_world(&self) -> Weak<World> {
        WorldGenRegion::weak_world(self)
    }

    fn block_state(&self, pos: BlockPos) -> BlockStateId {
        WorldGenRegion::block_state(self, pos)
    }

    fn set_block_state(&self, pos: BlockPos, state: BlockStateId, flags: UpdateFlags) -> bool {
        WorldGenRegion::set_block_state(self, pos, state, flags)
    }

    fn set_block_entity_data(
        &self,
        pos: BlockPos,
        block_entity_type: BlockEntityTypeRef,
        state: BlockStateId,
        nbt: NbtCompound,
    ) -> bool {
        WorldGenRegion::set_block_entity_data(self, pos, block_entity_type, state, nbt)
    }

    fn remove_block_entity(&self, pos: BlockPos) -> bool {
        WorldGenRegion::remove_block_entity(self, pos)
    }

    fn add_fresh_entity(&self, entity: SharedEntity) -> bool {
        WorldGenRegion::add_fresh_entity(self, entity)
    }

    fn height_at(&self, heightmap_type: HeightmapType, x: i32, z: i32) -> i32 {
        WorldGenRegion::height_at(self, heightmap_type, x, z)
    }
}
