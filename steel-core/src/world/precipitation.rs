//! Precipitation: ice formation, snow accumulation and cauldron filling.
//!
//! Ports `ServerLevel.tickPrecipitation` (`/tmp/vanilla/src/server/level/ServerLevel.java:588-623`)
//! together with `Biome.shouldFreeze`/`shouldSnow`/`getPrecipitationAt`
//! (`/tmp/vanilla/src/world/level/biome/Biome.java:103-200`) and the
//! `CauldronBlock.handlePrecipitation` consumer.
//!
//! Driven once per random-tick iteration per block-ticking chunk by the chunk tick, matching
//! vanilla's `ServerChunkCache.tickChunks` → `ServerLevel.tickChunk` ordering.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::biome::BiomeRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_game_events::BLOCK_CHANGE;
use steel_registry::vanilla_game_rules::MAX_SNOW_ACCUMULATION_HEIGHT;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, WorldAabb};

use crate::chunk::light::LightLayer;
use crate::fluid::{get_fluid_state, is_water_fluid};
use crate::world::game_event::GameEventContext;
use crate::world::{HeightmapType, World};

/// Vanilla `Biome.Precipitation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precipitation {
    /// The biome does not precipitate at this position.
    None,
    /// Warm precipitation: fills cauldrons with water.
    Rain,
    /// Cold precipitation: accumulates snow and fills cauldrons with powder snow.
    Snow,
}

/// Vanilla `SnowLayerBlock.MAX_LAYERS`.
const MAX_SNOW_LAYERS: i32 = 8;

impl World {
    /// Vanilla `ServerLevel.tickPrecipitation`.
    pub fn tick_precipitation(self: &Arc<Self>, pos: BlockPos) {
        let top_pos = self.heightmap_pos(HeightmapType::MotionBlocking, pos);
        let below_pos = top_pos.below();
        let Some(biome) = self.biome_at(top_pos) else {
            return;
        };

        if self.biome_should_freeze(biome, below_pos) {
            self.set_block(
                below_pos,
                vanilla_blocks::ICE.default_state(),
                UpdateFlags::UPDATE_ALL,
            );
        }

        if !self.is_raining() {
            return;
        }

        let max_height = self.get_game_rule(&MAX_SNOW_ACCUMULATION_HEIGHT);
        if max_height > 0 && self.biome_should_snow(biome, top_pos) {
            let state = self.get_block_state(top_pos);
            if state.get_block() == &vanilla_blocks::SNOW {
                let layers = state.get_value(&BlockStateProperties::LAYERS);
                let cap = max_height.min(MAX_SNOW_LAYERS);
                if i32::from(layers) < cap {
                    let new_state = state.set_value(&BlockStateProperties::LAYERS, layers + 1);
                    self.push_entities_up_for_snow(
                        top_pos,
                        i32::from(layers),
                        i32::from(layers) + 1,
                    );
                    self.set_block(top_pos, new_state, UpdateFlags::UPDATE_ALL);
                }
            } else {
                self.set_block(
                    top_pos,
                    vanilla_blocks::SNOW.default_state(),
                    UpdateFlags::UPDATE_ALL,
                );
            }
        }

        if self.biome_precipitation(biome, below_pos) != Precipitation::None {
            self.handle_precipitation(below_pos);
        }
    }

    /// Vanilla `Block.pushEntitiesUp` for a snow layer growing from `old_layers` to `new_layers`.
    ///
    /// Deliberate divergence: `Block.pushEntitiesUp` lives on `behavior::block` (private to the
    /// behavior module), so the snow-only case is resolved here with the exact added collision
    /// slab (one eighth of a block per layer) and the same vertical displacement.
    fn push_entities_up_for_snow(
        self: &Arc<Self>,
        pos: BlockPos,
        old_layers: i32,
        new_layers: i32,
    ) {
        let x = f64::from(pos.x());
        let z = f64::from(pos.z());
        let bottom = f64::from(pos.y()) + f64::from(old_layers) / MAX_SNOW_LAYERS as f64;
        let top = f64::from(pos.y()) + f64::from(new_layers) / MAX_SNOW_LAYERS as f64;
        let added_collision = WorldAabb::new(x, bottom, z, x + 1.0, top, z + 1.0);

        for entity in self.get_entities_in_aabb(&added_collision) {
            let entity_bottom = entity.bounding_box().min_y();
            if entity_bottom >= top {
                continue;
            }
            let delta = top - entity_bottom;
            if let Err(error) =
                entity.try_set_position(entity.position() + DVec3::new(0.0, delta, 0.0))
            {
                log::debug!(
                    "Failed to push entity {} up after snow accumulation at {pos:?}: {error}",
                    entity.id()
                );
            }
        }
    }

    /// Vanilla `CauldronBlock.handlePrecipitation` plus the `Block.handlePrecipitation` dispatch.
    ///
    /// Deliberate divergence: Steel has no generic `Block::handlePrecipitation` hook, so only the
    /// vanilla cauldron consumer is implemented here.
    fn handle_precipitation(self: &Arc<Self>, pos: BlockPos) {
        if self.get_block_state(pos).get_block() != &vanilla_blocks::CAULDRON {
            return;
        }

        let Some(biome) = self.biome_at(pos) else {
            return;
        };
        let replacement = match self.biome_precipitation(biome, pos) {
            Precipitation::Rain => vanilla_blocks::WATER_CAULDRON.default_state(),
            Precipitation::Snow => vanilla_blocks::POWDER_SNOW_CAULDRON.default_state(),
            Precipitation::None => return,
        };

        self.set_block(pos, replacement, UpdateFlags::UPDATE_ALL);
        self.game_event(&BLOCK_CHANGE, pos, &GameEventContext::new(None, None));
    }

    /// Vanilla `Biome.getPrecipitationAt`.
    #[must_use]
    fn biome_precipitation(&self, biome: BiomeRef, pos: BlockPos) -> Precipitation {
        if !biome.has_precipitation {
            return Precipitation::None;
        }

        if self.warm_enough_to_rain(biome, pos) {
            Precipitation::Rain
        } else {
            Precipitation::Snow
        }
    }

    /// Vanilla `Biome.warmEnoughToRain`.
    #[must_use]
    fn warm_enough_to_rain(&self, biome: BiomeRef, pos: BlockPos) -> bool {
        self.biome_temperature(biome, pos) >= 0.15
    }

    /// Vanilla `Biome.shouldFreeze`: cold enough, dark enough, and a water liquid block that is
    /// not fully surrounded by water.
    #[must_use]
    fn biome_should_freeze(self: &Arc<Self>, biome: BiomeRef, pos: BlockPos) -> bool {
        if self.warm_enough_to_rain(biome, pos) {
            return false;
        }

        if self.light_value_at(LightLayer::Block, pos) >= 10 {
            return false;
        }

        let state = self.get_block_state(pos);
        let block = state.get_block();
        if !is_water_fluid(get_fluid_state(self, pos).fluid_id)
            || (block != &vanilla_blocks::WATER && block != &vanilla_blocks::LAVA)
        {
            return false;
        }

        let surrounded_by_water = self.is_water_at(pos.west())
            && self.is_water_at(pos.east())
            && self.is_water_at(pos.north())
            && self.is_water_at(pos.south());
        !surrounded_by_water
    }

    /// Vanilla `LevelReader.isWaterAt`.
    #[must_use]
    fn is_water_at(self: &Arc<Self>, pos: BlockPos) -> bool {
        is_water_fluid(get_fluid_state(self, pos).fluid_id)
    }

    /// Vanilla `Biome.shouldSnow`: cold precipitation, dark enough, and snow can survive there.
    #[must_use]
    fn biome_should_snow(self: &Arc<Self>, biome: BiomeRef, pos: BlockPos) -> bool {
        if self.biome_precipitation(biome, pos) != Precipitation::Snow {
            return false;
        }

        if self.is_outside_build_height(pos.y())
            || self.light_value_at(LightLayer::Block, pos) >= 10
        {
            return false;
        }

        let state = self.get_block_state(pos);
        let block = state.get_block();
        if block != &vanilla_blocks::AIR && block != &vanilla_blocks::SNOW {
            return false;
        }

        Self::snow_can_survive(self.as_ref(), pos)
    }

    /// Vanilla `SnowLayerBlock.canSurvive` for the default snow state.
    #[must_use]
    fn snow_can_survive<L: crate::world::LevelReader + ?Sized>(world: &L, pos: BlockPos) -> bool {
        let below = pos.below();
        let below_state = world.get_block_state(below);
        if world.is_face_sturdy(below_state, below, steel_utils::Direction::Up) {
            return true;
        }

        // Vanilla also allows snow on a full snow layer block.
        below_state.get_block() == &vanilla_blocks::SNOW
            && i32::from(below_state.get_value(&BlockStateProperties::LAYERS)) == MAX_SNOW_LAYERS
    }
}
