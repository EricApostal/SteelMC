//! Level access for structure template and structure-piece placement.
//!
//! Vanilla runs both `StructureTemplate.placeInWorld` and
//! `StructurePiece.postProcess` against a `ServerLevelAccessor`, so `/place
//! template` and `/place structure` can write straight into gameplay chunks.
//! Steel's template layer was written against
//! [`WorldGenRegion`](crate::worldgen::region::WorldGenRegion), which is a
//! generation-only view: it needs a `&WorldGenContext`, a `&ChunkStep` and a
//! `StaticCache2D<Arc<ChunkHolder>>`, and it enforces worldgen dependency and
//! write-radius rules. There is no way to construct one for a live chunk.
//!
//! [`TemplateLevelAccess`] is the shared surface: [`LevelAccessor`] plus the
//! template-specific operations vanilla reaches through `ServerLevel`
//! (`LevelAccessor.getRandom`, `Level.getRandom`, `ServerLevel.getNoiseBiome`,
//! `Level.setBlockEntity`, `ServerLevel.addFreshEntity`). Two implementors
//! exist:
//!
//! * [`WorldGenRegion`] — worldgen placement, unchanged behavior.
//! * [`LiveLevel`] — a live `World` with the placement owns its own
//!   `Level.random` equivalent, used by `/place template` and `/place
//!   structure`.
//!
//! Deliberate difference from vanilla, kept because Steel's live world
//! serializes all block mutations: `TemplateLevelAccess::set_block_state` on a
//! live world goes through `World::set_block`, which claims the state and then
//! runs vanilla's ordered callbacks, instead of a bare palette write. The
//! worldgen path keeps its direct-chunk write.

use std::sync::{Arc, Weak};

use simdnbt::owned::NbtCompound;
use steel_registry::Registry;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::properties::Direction;
use steel_registry::blocks::shapes::SupportType;
use steel_registry::fluid::FluidRef;
use steel_registry::game_events::GameEventRef;
use steel_registry::sound_event::SoundEventRef;
use steel_utils::random::RandomSource;
use steel_utils::random::worldgen_random::WorldgenRandom;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier, types::UpdateFlags};

use crate::block_entity::{BLOCK_ENTITIES, SharedBlockEntity};
use crate::chunk::heightmap::HeightmapType;
use crate::entity::SharedEntity;
use crate::world::game_event::GameEventContext;
use crate::world::spawn::natural::runtime_random;
use crate::world::{LevelAccessor, LevelReader, ScheduledTickAccess, World};
use crate::worldgen::feature::FeatureDecorationRunner;
use crate::worldgen::region::WorldGenRegion;

/// The level operations template and structure-piece placement need.
///
/// Read access (`get_block_state`, `height_at`, build limits, sea level) comes
/// from [`LevelReader`], write access (`set_block_state`, liquid placement) from
/// [`LevelAccessor`]; this trait adds the template-specific operations.
pub(crate) trait TemplateLevelAccess: LevelAccessor {
    /// Returns the world seed used by position-derived processor randomness.
    fn seed(&self) -> i64;

    /// Returns a weak handle to the owning world, for entity construction.
    fn weak_world(&self) -> Weak<World>;

    /// Returns the level-owned random source (vanilla `LevelAccessor.getRandom`).
    fn random_mut(&mut self) -> &mut RandomSource;

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

    /// Returns the biome id at quart coordinates (vanilla `LevelReader.getNoiseBiome`).
    fn noise_biome_id(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> u16;

    /// Marks a position for worldgen postprocessing.
    ///
    /// Vanilla applies mushroom/soul-sand postprocessing when a proto chunk is
    /// promoted to a full chunk. A live level has no proto chunk, so the
    /// default is a no-op and only [`WorldGenRegion`] overrides it.
    #[expect(
        unused_variables,
        reason = "live levels and lightweight test surfaces have no postprocessing stage"
    )]
    fn mark_pos_for_postprocessing(&self, pos: BlockPos) {}

    /// Vanilla `FeaturePoolElement.place`: places a structure-pool feature at `pos`.
    ///
    /// World-generation regions run the real placed-feature pipeline; live levels
    /// cannot (the feature layer is still generation-only), so the default warns and
    /// reports failure and callers skip the piece.
    #[expect(
        unused_variables,
        reason = "live levels have no placed-feature pipeline; see GAP(feature-pool-element-live-level)"
    )]
    fn place_structure_pool_feature(
        &mut self,
        registry: &Registry,
        random: &mut WorldgenRandom,
        pos: BlockPos,
        feature: &Identifier,
        biome_zoom_seed: i64,
    ) -> bool {
        log::warn!(
            "cannot place structure-pool feature {feature} at {pos:?}: feature placement requires a world-generation region"
        );
        false
    }
}

impl TemplateLevelAccess for WorldGenRegion<'_> {
    fn seed(&self) -> i64 {
        WorldGenRegion::seed(self)
    }

    fn weak_world(&self) -> Weak<World> {
        WorldGenRegion::weak_world(self)
    }

    fn random_mut(&mut self) -> &mut RandomSource {
        WorldGenRegion::random_mut(self)
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

    fn noise_biome_id(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> u16 {
        WorldGenRegion::noise_biome_id(self, quart_x, quart_y, quart_z)
    }

    fn mark_pos_for_postprocessing(&self, pos: BlockPos) {
        WorldGenRegion::mark_pos_for_postprocessing(self, pos);
    }

    fn place_structure_pool_feature(
        &mut self,
        registry: &Registry,
        random: &mut WorldgenRandom,
        pos: BlockPos,
        feature: &Identifier,
        biome_zoom_seed: i64,
    ) -> bool {
        FeatureDecorationRunner::place_structure_pool_feature(
            self,
            registry,
            random,
            pos,
            feature,
            biome_zoom_seed,
        )
    }
}

/// A live world used as a structure-placement target.
///
/// Mirrors vanilla's `ServerLevel` role in `StructureTemplate.placeInWorld` and
/// `StructureStart.placeInChunk`: every level operation is delegated to the
/// world, and the adapter owns the placement random source because Steel's
/// `World` has no stored `Level.random` (see
/// [`runtime_random`](crate::world::spawn::natural::runtime_random)).
pub(crate) struct LiveLevel<'a> {
    world: &'a Arc<World>,
    random: RandomSource,
}

impl<'a> LiveLevel<'a> {
    /// Creates a placement target for `world` with a runtime-seeded random source.
    #[must_use]
    pub(crate) fn new(world: &'a Arc<World>) -> Self {
        Self {
            world,
            random: runtime_random(),
        }
    }
}

impl LevelReader for LiveLevel<'_> {
    fn get_block_state(&self, pos: BlockPos) -> BlockStateId {
        LevelReader::get_block_state(self.world, pos)
    }

    fn get_block_entity(&self, pos: BlockPos) -> Option<SharedBlockEntity> {
        LevelReader::get_block_entity(self.world, pos)
    }

    fn is_face_sturdy_for(
        &self,
        state: BlockStateId,
        pos: BlockPos,
        direction: Direction,
        support_type: SupportType,
    ) -> bool {
        LevelReader::is_face_sturdy_for(self.world, state, pos, direction, support_type)
    }

    fn raw_brightness(&self, pos: BlockPos, sky_darkening: u8) -> u8 {
        LevelReader::raw_brightness(self.world, pos, sky_darkening)
    }

    fn sky_darkening(&self) -> u8 {
        LevelReader::sky_darkening(self.world)
    }

    fn can_see_sky(&self, pos: BlockPos) -> bool {
        LevelReader::can_see_sky(self.world, pos)
    }

    fn ambient_light(&self) -> f32 {
        LevelReader::ambient_light(self.world)
    }

    fn height_at(&self, heightmap_type: HeightmapType, x: i32, z: i32) -> i32 {
        LevelReader::height_at(self.world, heightmap_type, x, z)
    }

    fn min_y(&self) -> i32 {
        LevelReader::min_y(self.world)
    }

    fn height(&self) -> i32 {
        LevelReader::height(self.world)
    }

    fn sea_level(&self) -> i32 {
        LevelReader::sea_level(self.world)
    }
}

impl ScheduledTickAccess for LiveLevel<'_> {
    fn fluid_tick_delay(&self, fluid: FluidRef) -> i32 {
        ScheduledTickAccess::fluid_tick_delay(self.world, fluid)
    }

    fn schedule_block_tick_default(&self, pos: BlockPos, block: BlockRef, delay: i32) -> bool {
        ScheduledTickAccess::schedule_block_tick_default(self.world, pos, block, delay)
    }

    fn has_scheduled_block_tick(&self, pos: BlockPos, block: BlockRef) -> bool {
        ScheduledTickAccess::has_scheduled_block_tick(self.world, pos, block)
    }

    fn will_tick_block_this_tick(&self, pos: BlockPos, block: BlockRef) -> bool {
        ScheduledTickAccess::will_tick_block_this_tick(self.world, pos, block)
    }

    fn schedule_fluid_tick_default(&self, pos: BlockPos, fluid: FluidRef, delay: i32) -> bool {
        ScheduledTickAccess::schedule_fluid_tick_default(self.world, pos, fluid, delay)
    }

    fn will_tick_fluid_this_tick(&self, pos: BlockPos, fluid: FluidRef) -> bool {
        ScheduledTickAccess::will_tick_fluid_this_tick(self.world, pos, fluid)
    }
}

impl LevelAccessor for LiveLevel<'_> {
    fn set_block_state(&self, pos: BlockPos, state: BlockStateId, flags: UpdateFlags) -> bool {
        LevelAccessor::set_block_state(self.world, pos, state, flags)
    }

    fn can_write_to_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
        LevelAccessor::can_write_to_chunk(self.world, chunk_x, chunk_z)
    }

    fn requires_live_write_preflight(&self) -> bool {
        LevelAccessor::requires_live_write_preflight(self.world)
    }

    fn destroy_block(&self, pos: BlockPos, drop_items: bool) -> bool {
        LevelAccessor::destroy_block(self.world, pos, drop_items)
    }

    fn play_block_sound(
        &self,
        sound: SoundEventRef,
        pos: BlockPos,
        volume: f32,
        pitch: f32,
        exclude: Option<i32>,
    ) {
        LevelAccessor::play_block_sound(self.world, sound, pos, volume, pitch, exclude);
    }

    fn game_event(&self, event: GameEventRef, pos: BlockPos, context: &GameEventContext<'_>) {
        LevelAccessor::game_event(self.world, event, pos, context);
    }
}

impl TemplateLevelAccess for LiveLevel<'_> {
    fn seed(&self) -> i64 {
        self.world.seed()
    }

    fn weak_world(&self) -> Weak<World> {
        Arc::downgrade(self.world)
    }

    fn random_mut(&mut self) -> &mut RandomSource {
        &mut self.random
    }

    fn set_block_entity_data(
        &self,
        pos: BlockPos,
        block_entity_type: BlockEntityTypeRef,
        state: BlockStateId,
        nbt: NbtCompound,
    ) -> bool {
        if !self.world.is_in_valid_bounds(pos) {
            return false;
        }

        let entity = BLOCK_ENTITIES.create_and_load_owned_or_unimplemented(
            block_entity_type,
            Arc::downgrade(self.world),
            pos,
            state,
            nbt,
        );
        self.world.set_block_entity(entity)
    }

    fn remove_block_entity(&self, pos: BlockPos) -> bool {
        self.world
            .chunk_map
            .with_full_chunk(ChunkPos::from_block_pos(pos), |chunk| {
                chunk.remove_block_entity(pos)
            })
            .unwrap_or(false)
    }

    fn add_fresh_entity(&self, entity: SharedEntity) -> bool {
        self.world.try_add_entity(entity).is_ok()
    }

    fn noise_biome_id(&self, quart_x: i32, quart_y: i32, quart_z: i32) -> u16 {
        let Some(biome_id) = self.world.noise_biome_id(quart_x, quart_y, quart_z) else {
            panic!("live level has no biome id at quart ({quart_x}, {quart_y}, {quart_z})");
        };
        biome_id
    }
}
