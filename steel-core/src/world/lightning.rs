//! Lightning strikes: target selection, bolt spawning and strike effects.
//!
//! Ports `ServerLevel.tickThunder`/`findLightningTargetAround`/`findLightningRod`
//! (`/tmp/vanilla/src/server/level/ServerLevel.java:544-660`) and the server half of
//! `LightningBolt.tick` (`/tmp/vanilla/src/world/entity/LightningBolt.java`): fire ignition,
//! lightning-rod powering, copper de-oxidation and wax removal.
//!
//! Entity damage and mob conversion run from the bolt entity
//! ([`crate::entity::entities::objects::technical::lightning_bolt`]) because vanilla dispatches
//! them through `Entity.thunderHit`.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::Direction;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_events::LIGHTNING_STRIKE;
use steel_registry::vanilla_game_rules::SPAWN_MOBS;
use steel_registry::vanilla_poi_types;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, ChunkPos, Downcast as _, WorldAabb};
use uuid::Uuid;

use crate::behavior::blocks::FireBlock;
use crate::behavior::blocks::LightningRodBlock;
use crate::behavior::blocks::WeatherState;
use crate::behavior::waxables::get_normal_from_waxed_variant;
use crate::behavior::weathering::{get_weather_state, previous_copper_stage};
use crate::entity::entities::mobs::passive::equine::SkeletonHorseEntity;
use crate::entity::entities::objects::technical::LightningBoltEntity;
use crate::entity::{AgeableMob as _, ENTITIES, SharedEntity, next_entity_id};
use crate::poi::OccupationStatus;
use crate::world::game_event::GameEventContext;
use crate::world::{HeightmapType, LevelReader, World};

/// Every vanilla block class that extends `LightningRodBlock`.
static LIGHTNING_ROD_VARIANTS: [steel_registry::blocks::BlockRef; 8] = [
    &vanilla_blocks::LIGHTNING_ROD,
    &vanilla_blocks::EXPOSED_LIGHTNING_ROD,
    &vanilla_blocks::WEATHERED_LIGHTNING_ROD,
    &vanilla_blocks::OXIDIZED_LIGHTNING_ROD,
    &vanilla_blocks::WAXED_LIGHTNING_ROD,
    &vanilla_blocks::WAXED_EXPOSED_LIGHTNING_ROD,
    &vanilla_blocks::WAXED_WEATHERED_LIGHTNING_ROD,
    &vanilla_blocks::WAXED_OXIDIZED_LIGHTNING_ROD,
];

impl World {
    /// Vanilla `ServerLevel.tickThunder`: rolls the per-chunk lightning chance and spawns a bolt
    /// (plus the optional skeleton trap) at the chosen target.
    pub fn tick_thunder(self: &Arc<Self>, chunk_pos: ChunkPos) {
        if !self.is_raining()
            || !self.is_thundering()
            || rand::random::<i32>().rem_euclid(100_000) != 0
        {
            return;
        }

        let pos = self.find_lightning_target_around(Self::random_block_pos_in_chunk(chunk_pos));
        if !self.is_raining_at(pos) {
            return;
        }

        let is_trap = self.get_game_rule(&SPAWN_MOBS)
            && rand::random::<f64>()
                < f64::from(self.current_difficulty_at(pos).effective_difficulty()) * 0.01
            && !self
                .get_block_state(pos.below())
                .get_block()
                .has_tag(&BlockTag::LIGHTNING_RODS);

        if is_trap {
            self.spawn_lightning_trap_horse(pos);
        }

        let _ = self.spawn_lightning_bolt(pos, is_trap);
    }

    /// Vanilla `ServerLevel.tickThunder`'s `SKELETON_HORSE` creation (`EntitySpawnReason.EVENT`).
    ///
    /// The trap horse sits at the integer block coordinates, not the block centre, matching
    /// `setPos(pos.getX(), pos.getY(), pos.getZ())`.
    fn spawn_lightning_trap_horse(self: &Arc<Self>, pos: BlockPos) {
        let Some(entity) = ENTITIES.create(
            &vanilla_entities::SKELETON_HORSE,
            next_entity_id(),
            DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z())),
            Arc::downgrade(self),
        ) else {
            return;
        };
        let Some(horse) = entity.as_ref().downcast_ref::<SkeletonHorseEntity>() else {
            return;
        };
        horse.set_trap(true);
        horse.set_age(0);
        let _ = self.try_add_entity(entity);
    }

    /// Vanilla `ServerLevel.findLightningTargetAround`.
    #[must_use]
    pub fn find_lightning_target_around(self: &Arc<Self>, pos: BlockPos) -> BlockPos {
        let center = self.heightmap_pos(HeightmapType::MotionBlocking, pos);

        if let Some(rod) = self.find_lightning_rod(center) {
            return rod;
        }

        let search = Self::lightning_target_area(center, self.max_y_exclusive());
        let candidates = self.get_entities_in_aabb_matching(&search, |entity| {
            entity.is_alive()
                && entity.is_living_entity()
                && self.can_see_sky(entity.block_position())
        });
        if !candidates.is_empty() {
            let index = rand::random_range(0..candidates.len());
            return candidates[index].block_position();
        }

        if center.y() == self.min_y() - 1 {
            return center.above().above();
        }
        center
    }

    /// Vanilla `AABB.encapsulatingFullBlocks(center, center.atY(maxY + 1)).inflate(3.0)`.
    ///
    /// `max_y_exclusive` is vanilla's `getMaxY() + 1`, so the inflated box reaches
    /// four blocks past the exclusive build height.
    #[must_use]
    fn lightning_target_area(center: BlockPos, max_y_exclusive: i32) -> WorldAabb {
        WorldAabb::new(
            f64::from(center.x()) - 3.0,
            f64::from(center.y()) - 3.0,
            f64::from(center.z()) - 3.0,
            f64::from(center.x()) + 4.0,
            f64::from(max_y_exclusive) + 4.0,
            f64::from(center.z()) + 4.0,
        )
    }

    /// Vanilla `ServerLevel.findLightningRod`: the nearest lightning rod whose top is exposed.
    #[must_use]
    pub fn find_lightning_rod(&self, center: BlockPos) -> Option<BlockPos> {
        let storage = self.poi_storage.lock();
        let is_lightning_rod = |poi_type: steel_registry::poi::PoiTypeRef| {
            poi_type.key == vanilla_poi_types::LIGHTNING_ROD.key
        };
        storage
            .get_in_range(&is_lightning_rod, center, 128, OccupationStatus::Any)
            .into_iter()
            .filter(|(_, rod_pos)| {
                self.height_at(HeightmapType::WorldSurface, rod_pos.x(), rod_pos.z())
                    .is_some_and(|height| rod_pos.y() == height - 1)
            })
            .min_by_key(|(_, rod_pos)| {
                let dx = i64::from(rod_pos.x() - center.x());
                let dy = i64::from(rod_pos.y() - center.y());
                let dz = i64::from(rod_pos.z() - center.z());
                dx * dx + dy * dy + dz * dz
            })
            .map(|(_, rod_pos)| rod_pos.above())
    }

    /// Vanilla `EntityTypes.LIGHTNING_BOLT` creation in `tickThunder`.
    ///
    /// Returns the spawned bolt, or `None` when the entity type has no registered factory or the
    /// chunk is unloaded.
    pub fn spawn_lightning_bolt(
        self: &Arc<Self>,
        pos: BlockPos,
        visual_only: bool,
    ) -> Option<SharedEntity> {
        self.spawn_lightning_bolt_with_cause(pos, visual_only, None)
    }

    /// Vanilla `LightningBolt` creation with an explicit `cause` (the channeling trident thrower).
    ///
    /// Returns the spawned bolt so callers can record further state, or `None` when the entity type
    /// has no registered factory or the chunk is unloaded.
    pub fn spawn_lightning_bolt_with_cause(
        self: &Arc<Self>,
        pos: BlockPos,
        visual_only: bool,
        cause: Option<Uuid>,
    ) -> Option<SharedEntity> {
        let entity = ENTITIES.create(
            &vanilla_entities::LIGHTNING_BOLT,
            next_entity_id(),
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()),
                f64::from(pos.z()) + 0.5,
            ),
            Arc::downgrade(self),
        )?;

        let bolt = entity.as_ref().downcast_ref::<LightningBoltEntity>()?;
        bolt.set_visual_only(visual_only);
        bolt.set_cause(cause);
        self.try_add_entity(Arc::clone(&entity)).ok()?;
        Some(entity)
    }

    /// Server-side `LightningBolt.tick` strike effects, run once on the bolt's first tick.
    pub(crate) fn lightning_bolt_strike_effects(
        self: &Arc<Self>,
        pos: BlockPos,
        visual_only: bool,
    ) {
        if visual_only {
            return;
        }

        if matches!(
            self.difficulty(),
            steel_utils::types::Difficulty::Normal | steel_utils::types::Difficulty::Hard
        ) {
            self.lightning_spawn_fire(pos, 4);
        }

        let strike_pos = BlockPos::new(pos.x(), pos.y() - 1, pos.z());
        let strike_state = self.get_block_state(strike_pos);
        let strike_block = strike_state.get_block();
        // Vanilla's `instanceof LightningRodBlock` also covers `WeatheringLightningRodBlock`,
        // which extends it. Steel composes instead of inheriting, so match every rod variant.
        if LIGHTNING_ROD_VARIANTS.contains(&strike_block) {
            LightningRodBlock::on_lightning_strike(strike_block, strike_state, self, strike_pos);
        }
        self.clear_copper_on_lightning_strike(strike_pos);
        self.game_event(&LIGHTNING_STRIKE, pos, &GameEventContext::new(None, None));
    }

    /// Vanilla `LightningBolt.spawnFire`.
    pub(crate) fn lightning_spawn_fire(self: &Arc<Self>, pos: BlockPos, additional_sources: i32) {
        if !self.can_spread_fire_around(pos) {
            return;
        }

        self.ignite_fire_at(pos);
        for _ in 0..additional_sources {
            self.ignite_fire_at(pos.offset(
                rand::random_range(-1..2),
                rand::random_range(-1..2),
                rand::random_range(-1..2),
            ));
        }
    }

    /// Places fire at `pos` when vanilla's air + survival checks pass.
    fn ignite_fire_at(self: &Arc<Self>, pos: BlockPos) {
        if !self.get_block_state(pos).is_air() {
            return;
        }
        let fire = FireBlock::get_state(self.as_ref(), pos);
        if FireBlock::can_be_placed_at(self, pos, Direction::Up) {
            self.set_block(pos, fire, UpdateFlags::UPDATE_ALL);
        }
    }

    /// Vanilla `LightningBolt.clearCopperOnLightningStrike`: de-oxidizes struck copper and runs
    /// randomized walks that clean nearby copper and remove wax.
    fn clear_copper_on_lightning_strike(self: &Arc<Self>, struck_pos: BlockPos) {
        let struck_state = self.get_block_state(struck_pos);
        let struck_block = struck_state.get_block();
        let is_weathering_copper = get_weather_state(struck_block).is_some();
        let is_waxed = get_normal_from_waxed_variant(struck_block).is_some();

        if !is_weathering_copper && !is_waxed {
            return;
        }

        if is_weathering_copper
            && let Some(unaffected) = Self::copper_unaffected_variant(struck_block)
        {
            self.set_block(
                struck_pos,
                unaffected.default_state(),
                UpdateFlags::UPDATE_ALL,
            );
        }

        for _ in 0..rand::random_range(3..6) {
            let mut work_pos = struck_pos;
            for _ in 0..rand::random_range(1..9) {
                match self.random_step_cleaning_copper(work_pos) {
                    Some(next) => work_pos = next,
                    None => break,
                }
            }
        }
    }

    /// Vanilla `WeatheringCopper.getFirst`: the unoxidised variant of a weathering copper block.
    fn copper_unaffected_variant(
        block: steel_registry::blocks::BlockRef,
    ) -> Option<steel_registry::blocks::BlockRef> {
        let mut current = block;
        while let Some(previous) = previous_copper_stage(current) {
            current = previous;
        }
        (get_weather_state(current) == Some(WeatherState::Unaffected)).then_some(current)
    }

    /// Vanilla `LightningBolt.randomStepCleaningCopper`: one random cube probe.
    ///
    /// Vanilla only matches `instanceof WeatheringCopper`, and waxed copper blocks are
    /// plain `Block`s that do not implement that interface, so a waxed block is never
    /// cleaned by the walk (only the directly struck block's waxed/unwaxed state gates
    /// whether the walk runs at all).
    fn random_step_cleaning_copper(self: &Arc<Self>, pos: BlockPos) -> Option<BlockPos> {
        for _ in 0..10 {
            let candidate = pos.offset(
                rand::random_range(-1..2),
                rand::random_range(-1..2),
                rand::random_range(-1..2),
            );
            let block = self.get_block_state(candidate).get_block();
            let Some(cleaned) = previous_copper_stage(block) else {
                continue;
            };
            self.set_block(candidate, cleaned.default_state(), UpdateFlags::UPDATE_ALL);
            // Vanilla emits level event 3002 (`PARTICLES_ELECTRIC_SPARK`) for cleaned copper.
            self.level_event(
                steel_registry::level_events::PARTICLES_ELECTRIC_SPARK,
                candidate,
                -1,
                None,
            );
            return Some(candidate);
        }
        None
    }

    /// Vanilla `Level.getBlockRandomPos(minX, 0, minZ, 15)` for a chunk.
    ///
    /// Deliberate divergence: vanilla advances the level's own LCG (`Level.randValue`); Steel has no
    /// per-level seeded RNG, so this uses the runtime RNG like the chunk random-tick driver.
    fn random_block_pos_in_chunk(chunk_pos: ChunkPos) -> BlockPos {
        let val = rand::random::<i32>() >> 2;
        BlockPos::new(
            (chunk_pos.0.x << 4) + (val & 15),
            val >> 16 & 15,
            (chunk_pos.0.y << 4) + (val >> 8 & 15),
        )
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec3;

    use steel_registry::init_vanilla_registry;

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::entity::entities::PigEntity;
    use crate::test_support::{TestEntity, fresh_test_world, insert_ready_full_chunk};

    /// Vanilla `ServerLevel.findLightningTargetAround` searches
    /// `AABB.encapsulatingFullBlocks(center, center.atY(getMaxY() + 1)).inflate(3.0)`.
    #[test]
    fn lightning_target_area_matches_vanilla_inflated_full_block_box() {
        let area = World::lightning_target_area(BlockPos::new(8, 64, 8), 320);
        assert_eq!(area.min_x(), 5.0);
        assert_eq!(area.min_y(), 61.0);
        assert_eq!(area.min_z(), 5.0);
        assert_eq!(area.max_x(), 12.0);
        assert_eq!(area.max_y(), 324.0);
        assert_eq!(area.max_z(), 12.0);
    }

    /// Vanilla only strikes `LivingEntity`s, so a non-living entity inside the search box
    /// must not become the target.
    #[test]
    fn lightning_target_ignores_non_living_entities() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("lightning_target_non_living");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        // A lone stone block puts MOTION_BLOCKING at y = 64 for the queried column.
        world.set_block(
            BlockPos::new(8, 63, 8),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        );

        let boat = TestEntity::shared(
            1,
            DVec3::new(10.5, 64.0, 10.5),
            Arc::downgrade(world),
            &vanilla_entities::MINECART,
        );
        world.try_add_entity(boat).expect("boat must be added");

        assert_eq!(
            world.find_lightning_target_around(BlockPos::new(8, 64, 8)),
            BlockPos::new(8, 64, 8),
        );
    }

    /// Vanilla's search box reaches three blocks below the heightmap position, so a
    /// living entity there is a valid target.
    #[test]
    fn lightning_target_includes_living_entities_below_the_heightmap_position() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("lightning_target_below_heightmap");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        world.set_block(
            BlockPos::new(8, 63, 8),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        );

        let pig: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            2,
            DVec3::new(8.5, 62.0, 8.5),
            Arc::downgrade(world),
        ));
        world.try_add_entity(pig).expect("pig must be added");

        assert_eq!(
            world.find_lightning_target_around(BlockPos::new(8, 64, 8)),
            BlockPos::new(8, 62, 8),
        );
    }

    /// Vanilla `randomStepCleaningCopper` only matches `WeatheringCopper`; waxed copper is
    /// a plain `Block` there and must be left untouched by the walk.
    #[test]
    fn copper_walk_does_not_unwax_waxed_blocks() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("lightning_copper_walk_waxed");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let center = BlockPos::new(8, 64, 8);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    world.set_block(
                        center.offset(dx, dy, dz),
                        vanilla_blocks::WAXED_COPPER_BLOCK.default_state(),
                        UpdateFlags::UPDATE_ALL,
                    );
                }
            }
        }

        assert_eq!(world.random_step_cleaning_copper(center), None);
        assert_eq!(
            world.get_block_state(center).get_block(),
            &vanilla_blocks::WAXED_COPPER_BLOCK,
        );
    }

    /// The walk still de-oxidises ordinary weathering copper one stage.
    #[test]
    fn copper_walk_deoxidises_weathering_copper() {
        init_vanilla_registry();
        init_behaviors();
        let fixture = fresh_test_world("lightning_copper_walk_weathering");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let center = BlockPos::new(8, 64, 8);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    world.set_block(
                        center.offset(dx, dy, dz),
                        vanilla_blocks::EXPOSED_COPPER.default_state(),
                        UpdateFlags::UPDATE_ALL,
                    );
                }
            }
        }

        let cleaned = world
            .random_step_cleaning_copper(center)
            .expect("exposed copper must be cleaned");
        assert_eq!(
            world.get_block_state(cleaned).get_block(),
            &vanilla_blocks::COPPER_BLOCK,
        );
    }
}
