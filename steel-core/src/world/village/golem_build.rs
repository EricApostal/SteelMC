//! Golem construction from placed blocks (vanilla `CarvedPumpkinBlock`).
//!
//! Vanilla runs this when a carved pumpkin or jack o'lantern is placed: it matches
//! multi-block patterns (snow, iron, copper) around the new pumpkin, tears the matching
//! pattern down to air, and spawns the golem at the pattern's spawn cell. This module
//! ported the two patterns whose golems Steel can build:
//!
//! - iron golem: `~^~ / ### / ~#~`, spawn cell `(1, 2, 0)` (vanilla
//!   `getOrCreateIronGolemFull`),
//! - snow golem: `^ / # / #`, spawn cell `(0, 2, 0)` (vanilla `getOrCreateSnowGolemFull`).
//!
//! The copper golem pattern is intentionally absent: building it also converts the
//! copper block into a chest and transfers the copper's weathering state (`WeatheringCopper`),
//! and Steel has no copper golem entity. [`try_spawn_golem`] leaves a matching copper
//! build in place instead of clearing it, and [`can_spawn_golem`] reports it so the
//! dispenser still refuses to consume a pumpkin there.
//!
//! [`can_spawn_golem`] mirrors `CarvedPumpkinBlock.canSpawnGolem`, which the dispenser
//! uses to decide whether a dispensed pumpkin would complete a golem.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::{
    level_events, vanilla_block_tags::BlockTag, vanilla_blocks, vanilla_entities,
};
use steel_utils::BlockPos;
use steel_utils::types::UpdateFlags;

use crate::entity::{
    ENTITIES, EntitySpawnReason, SharedEntity, add_spawned_entity, create_entity_instance,
};
use crate::world::{LevelReader, World};

/// A cell test in a pattern (vanilla `Predicate<BlockInWorld>`).
type CellPredicate = fn(&dyn LevelReader, BlockPos) -> bool;

/// One multi-block pattern (vanilla `BlockPattern`), built from `BlockPatternBuilder`
/// aisles with the first row at the top.
struct BlockPattern {
    /// Rows top to bottom; every row has the same width. Depth is always one.
    rows: &'static [&'static str],
    cells: &'static [(char, CellPredicate)],
}

/// A pattern match (vanilla `BlockPattern.BlockPatternMatch`).
#[derive(Clone, Copy)]
struct PatternMatch {
    front_top_left: BlockPos,
    forwards: Direction,
    up: Direction,
}

impl BlockPattern {
    fn width(&self) -> i32 {
        self.rows[0].len() as i32
    }

    fn height(&self) -> i32 {
        self.rows.len() as i32
    }

    fn predicate(&self, cell: char) -> CellPredicate {
        if cell == ' ' {
            return |_, _| true;
        }
        match self.cells.iter().find(|(name, _)| *name == cell) {
            Some((_, predicate)) => *predicate,
            // Unreachable for the patterns declared here; the `pattern_cells_all_have_predicates`
            // test asserts every non-space cell of every pattern has one.
            None => |_, _| false,
        }
    }

    /// Vanilla `BlockPattern.matches`.
    fn matches(
        &self,
        level: &dyn LevelReader,
        origin: BlockPos,
        forwards: Direction,
        up: Direction,
    ) -> bool {
        for (down, row) in self.rows.iter().enumerate() {
            for (right, cell) in row.chars().enumerate() {
                let pos = translate_and_rotate(origin, forwards, up, right as i32, down as i32, 0);
                if !self.predicate(cell)(level, pos) {
                    return false;
                }
            }
        }
        true
    }

    /// Vanilla `BlockPattern.find`: samples every origin in the `dist` cube after
    /// `start`, trying each valid `forwards`/`up` pair in `Direction` order.
    fn find(&self, level: &dyn LevelReader, start: BlockPos) -> Option<PatternMatch> {
        let dist = self.width().max(self.height()).max(1);

        for x in 0..dist {
            for y in 0..dist {
                for z in 0..dist {
                    let origin = start.offset(x, y, z);
                    for forwards in Direction::ALL {
                        for up in Direction::ALL {
                            if up == forwards || up == forwards.opposite() {
                                continue;
                            }
                            if self.matches(level, origin, forwards, up) {
                                return Some(PatternMatch {
                                    front_top_left: origin,
                                    forwards,
                                    up,
                                });
                            }
                        }
                    }
                }
            }
        }

        None
    }
}

impl PatternMatch {
    /// Vanilla `BlockPatternMatch.getBlock`.
    fn block(&self, right: i32, down: i32, forwards: i32) -> BlockPos {
        translate_and_rotate(
            self.front_top_left,
            self.forwards,
            self.up,
            right,
            down,
            forwards,
        )
    }

    /// Vanilla `BlockPatternMatch.getWidth` / `getHeight`: every pattern cell position.
    fn cells(&self, width: i32, height: i32) -> Vec<BlockPos> {
        let mut cells = Vec::new();
        for right in 0..width {
            for down in 0..height {
                cells.push(translate_and_rotate(
                    self.front_top_left,
                    self.forwards,
                    self.up,
                    right,
                    down,
                    0,
                ));
            }
        }
        cells
    }
}

/// Vanilla `BlockPattern.translateAndRotate` with the single-aisle depth.
fn translate_and_rotate(
    origin: BlockPos,
    forwards: Direction,
    up: Direction,
    right: i32,
    down: i32,
    forwards_offset: i32,
) -> BlockPos {
    let forwards_vec = forwards.offset_vec();
    let up_vec = up.offset_vec();
    let right_vec = forwards_vec.cross(up_vec);

    BlockPos(origin.0 + up_vec * -down + right_vec * right + forwards_vec * forwards_offset)
}

fn is_pumpkin(level: &dyn LevelReader, pos: BlockPos) -> bool {
    let block = level.get_block_state(pos).get_block();
    block == &vanilla_blocks::CARVED_PUMPKIN || block == &vanilla_blocks::JACK_O_LANTERN
}

fn is_air(level: &dyn LevelReader, pos: BlockPos) -> bool {
    level.get_block_state(pos).is_air()
}

fn is_iron_block(level: &dyn LevelReader, pos: BlockPos) -> bool {
    level.get_block_state(pos).get_block() == &vanilla_blocks::IRON_BLOCK
}

fn is_snow_block(level: &dyn LevelReader, pos: BlockPos) -> bool {
    level.get_block_state(pos).get_block() == &vanilla_blocks::SNOW_BLOCK
}

fn is_copper_block(level: &dyn LevelReader, pos: BlockPos) -> bool {
    level
        .get_block_state(pos)
        .get_block()
        .has_tag(&BlockTag::COPPER)
}

/// Vanilla `CarvedPumpkinBlock.getOrCreateIronGolemBase`.
const IRON_GOLEM_BASE: BlockPattern = BlockPattern {
    rows: &["~ ~", "###", "~#~"],
    cells: &[('~', is_air), ('#', is_iron_block)],
};

/// Vanilla `CarvedPumpkinBlock.getOrCreateIronGolemFull`.
const IRON_GOLEM_FULL: BlockPattern = BlockPattern {
    rows: &["~^~", "###", "~#~"],
    cells: &[('~', is_air), ('^', is_pumpkin), ('#', is_iron_block)],
};

/// Vanilla `CarvedPumpkinBlock.getOrCreateSnowGolemBase`.
const SNOW_GOLEM_BASE: BlockPattern = BlockPattern {
    rows: &[" ", "#", "#"],
    cells: &[('#', is_snow_block)],
};

/// Vanilla `CarvedPumpkinBlock.getOrCreateSnowGolemFull`.
const SNOW_GOLEM_FULL: BlockPattern = BlockPattern {
    rows: &["^", "#", "#"],
    cells: &[('^', is_pumpkin), ('#', is_snow_block)],
};

/// Vanilla `CarvedPumpkinBlock.getOrCreateCopperGolemBase`.
const COPPER_GOLEM_BASE: BlockPattern = BlockPattern {
    rows: &[" ", "#"],
    cells: &[('#', is_copper_block)],
};

/// Vanilla `CarvedPumpkinBlock.canSpawnGolem`: would a pumpkin placed at `top_pos`
/// complete a golem build? Used by the dispenser's pumpkin behavior.
#[must_use]
pub fn can_spawn_golem(level: &dyn LevelReader, top_pos: BlockPos) -> bool {
    SNOW_GOLEM_BASE.find(level, top_pos).is_some()
        || IRON_GOLEM_BASE.find(level, top_pos).is_some()
        || COPPER_GOLEM_BASE.find(level, top_pos).is_some()
}

/// Vanilla `CarvedPumpkinBlock.trySpawnGolem`, called when a carved pumpkin or jack
/// o'lantern is placed at `top_pos`.
///
/// Returns the spawned golem, if a pattern matched and Steel can build that golem kind.
pub fn try_spawn_golem(level: &Arc<World>, top_pos: BlockPos) -> Option<SharedEntity> {
    if let Some(found) = SNOW_GOLEM_FULL.find(level.as_ref(), top_pos) {
        if let Some(golem) = spawn_golem_in_world(
            level,
            &found,
            &SNOW_GOLEM_FULL,
            &vanilla_entities::SNOW_GOLEM,
            found.block(0, 2, 0),
        ) {
            return Some(golem);
        }
    }

    if let Some(found) = IRON_GOLEM_FULL.find(level.as_ref(), top_pos) {
        if let Some(golem) = spawn_golem_in_world(
            level,
            &found,
            &IRON_GOLEM_FULL,
            &vanilla_entities::IRON_GOLEM,
            found.block(1, 2, 0),
        ) {
            return Some(golem);
        }
    }

    // The copper golem build is not implemented: it also replaces the copper block with a
    // chest and transfers the block's weathering state, and Steel has no copper golem yet.

    None
}

/// Vanilla `CarvedPumpkinBlock.spawnGolemInWorld`.
fn spawn_golem_in_world(
    level: &Arc<World>,
    found: &PatternMatch,
    pattern: &BlockPattern,
    entity_type: EntityTypeRef,
    spawn_pos: BlockPos,
) -> Option<SharedEntity> {
    if !ENTITIES.has_factory(entity_type) {
        // Steel has no implementation for this golem yet; leaving the build intact is
        // better than clearing it without spawning anything.
        return None;
    }

    clear_pattern_blocks(level, found, pattern);

    // Vanilla `spawnGolemInWorld`: `create(level, EntitySpawnReason.TRIGGERED)` then
    // `snapTo(x + 0.5, y + 0.05, z + 0.5, 0.0F, 0.0F)` then `addFreshEntity`. Steel's
    // `finalize_spawn` runs here too, because every other Steel spawn path relies on it
    // to set up the mob's spawn bonuses.
    let golem = create_entity_instance(
        level,
        entity_type,
        DVec3::new(
            f64::from(spawn_pos.x()) + 0.5,
            f64::from(spawn_pos.y()) + 0.05,
            f64::from(spawn_pos.z()) + 0.5,
        ),
    )
    .ok()?;

    golem.set_rotation((0.0, 0.0));
    golem.set_old_position_to_current();
    golem.base().set_old_rotation_to_current();
    if let Some(mob) = golem.as_mob() {
        let _ = mob.finalize_spawn(level, EntitySpawnReason::Triggered, None);
    }

    add_spawned_entity(level, Arc::clone(&golem)).ok()?;

    // TODO: trigger `CriteriaTriggers.SUMMONED_ENTITY` for players within 5 blocks once
    // Steel has the advancement foundation.

    update_pattern_blocks(level, found, pattern);
    Some(golem)
}

/// Vanilla `CarvedPumpkinBlock.clearPatternBlocks`: breaks every pattern block down to
/// air and plays the block-break effect.
fn clear_pattern_blocks(level: &Arc<World>, found: &PatternMatch, pattern: &BlockPattern) {
    let air = vanilla_blocks::AIR.default_state();

    for pos in found.cells(pattern.width(), pattern.height()) {
        let state = level.get_block_state(pos);
        level.set_block(pos, air, UpdateFlags::UPDATE_CLIENTS);
        level.level_event(
            level_events::PARTICLES_AND_SOUND_DESTROY_BLOCK,
            pos,
            i32::from(state.0),
            None,
        );
    }
}

/// Vanilla `CarvedPumpkinBlock.updatePatternBlocks`: notifies neighbors of every
/// pattern block that was cleared.
fn update_pattern_blocks(level: &Arc<World>, found: &PatternMatch, pattern: &BlockPattern) {
    for pos in found.cells(pattern.width(), pattern.height()) {
        level.update_neighbors_at(pos, &vanilla_blocks::AIR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pattern cell without a predicate would silently never match, so every declared
    /// pattern must cover its own characters.
    #[test]
    fn pattern_cells_all_have_predicates() {
        for pattern in [
            &IRON_GOLEM_BASE,
            &IRON_GOLEM_FULL,
            &SNOW_GOLEM_BASE,
            &SNOW_GOLEM_FULL,
            &COPPER_GOLEM_BASE,
        ] {
            for row in pattern.rows {
                for cell in row.chars() {
                    if cell == ' ' {
                        continue;
                    }
                    assert!(
                        pattern.cells.iter().any(|(name, _)| *name == cell),
                        "pattern row exposes an unmapped cell '{cell}'"
                    );
                }
            }
        }
    }

    /// The iron golem pattern is three blocks wide and three tall, with the spawn cell at
    /// `(1, 2, 0)` like vanilla's `getBlock(1, 2, 0)`.
    #[test]
    fn iron_golem_pattern_geometry_matches_vanilla() {
        assert_eq!(IRON_GOLEM_FULL.width(), 3);
        assert_eq!(IRON_GOLEM_FULL.height(), 3);
        assert_eq!(SNOW_GOLEM_FULL.width(), 1);
        assert_eq!(SNOW_GOLEM_FULL.height(), 3);
    }

    /// The live pumpkin build: placing the iron pattern and calling `try_spawn_golem`
    /// spawns a player-created iron golem at `(1, 2, 0)` of the match and clears the
    /// pattern blocks to air (vanilla `CarvedPumpkinBlock.trySpawnGolem`).
    #[test]
    fn pumpkin_build_spawns_a_player_created_iron_golem() {
        use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;
        use steel_utils::ChunkPos;

        init_vanilla_registry();
        crate::entity::init_entities();

        let fixture = fresh_test_world("pumpkin_iron_golem");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        // forwards = north, up = up: `right` is +x and `down` is -y, so the top-left
        // origin is the pumpkin's west neighbour.
        let origin = BlockPos::new(10, 70, 10);
        let pumpkin = BlockPos::new(11, 70, 10);
        let air = vanilla_blocks::AIR.default_state();
        world.set_block(
            pumpkin,
            vanilla_blocks::CARVED_PUMPKIN.default_state(),
            UpdateFlags::UPDATE_CLIENTS,
        );
        for x in 10..=12 {
            world.set_block(
                BlockPos::new(x, 69, 10),
                vanilla_blocks::IRON_BLOCK.default_state(),
                UpdateFlags::UPDATE_CLIENTS,
            );
        }
        world.set_block(
            BlockPos::new(11, 68, 10),
            vanilla_blocks::IRON_BLOCK.default_state(),
            UpdateFlags::UPDATE_CLIENTS,
        );

        let golem = try_spawn_golem(world, pumpkin).expect("the iron pattern builds a golem");
        assert_eq!(golem.entity_type(), &vanilla_entities::IRON_GOLEM);
        assert_eq!(
            golem.position(),
            DVec3::new(11.5, 68.05, 10.5),
            "vanilla spawns at `getBlock(1, 2, 0)` plus the 0.05 Y snap"
        );
        assert!(
            golem
                .downcast_ref::<crate::entity::entities::mobs::golem::IronGolemEntity>()
                .expect("iron golem downcast")
                .is_player_created(),
            "a pumpkin-built golem is player-created"
        );

        // Every pattern cell is cleared, including the pumpkin and the spawn cell.
        assert_eq!(world.get_block_state(pumpkin), air);
        for x in 10..=12 {
            assert_eq!(world.get_block_state(BlockPos::new(x, 69, 10)), air);
        }
        assert_eq!(world.get_block_state(BlockPos::new(11, 68, 10)), air);
        assert!(
            world.get_entity_by_id(golem.id()).is_some(),
            "the golem joins the level"
        );
        assert!(origin.x() < pumpkin.x());
    }

    /// The snow golem pattern spawns at `(0, 2, 0)` and is not player-created-flagged
    /// (vanilla only sets that flag on iron golems).
    #[test]
    fn pumpkin_build_spawns_a_snow_golem() {
        use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;
        use steel_utils::ChunkPos;

        init_vanilla_registry();
        crate::entity::init_entities();

        let fixture = fresh_test_world("pumpkin_snow_golem");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(1, 1));

        let pumpkin = BlockPos::new(20, 70, 20);
        world.set_block(
            pumpkin,
            vanilla_blocks::CARVED_PUMPKIN.default_state(),
            UpdateFlags::UPDATE_CLIENTS,
        );
        world.set_block(
            BlockPos::new(20, 69, 20),
            vanilla_blocks::SNOW_BLOCK.default_state(),
            UpdateFlags::UPDATE_CLIENTS,
        );
        world.set_block(
            BlockPos::new(20, 68, 20),
            vanilla_blocks::SNOW_BLOCK.default_state(),
            UpdateFlags::UPDATE_CLIENTS,
        );

        let golem = try_spawn_golem(world, pumpkin).expect("the snow pattern builds a golem");
        assert_eq!(golem.entity_type(), &vanilla_entities::SNOW_GOLEM);
        assert_eq!(golem.position(), DVec3::new(20.5, 68.05, 20.5));
        assert_eq!(
            world.get_block_state(pumpkin),
            vanilla_blocks::AIR.default_state()
        );
    }

    /// `can_spawn_golem` accepts a pumpkin whose placement completes a base pattern, and
    /// rejects one that completes nothing.
    #[test]
    fn can_spawn_golem_checks_the_base_patterns() {
        use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;
        use steel_utils::ChunkPos;

        init_vanilla_registry();
        let fixture = fresh_test_world("pumpkin_can_spawn");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(1, 1));

        // The pattern search spans a 3x3x3 cube from the placed pumpkin, so keep it
        // inside the loaded chunk (x/z 16..31).
        let top = BlockPos::new(18, 70, 18);
        assert!(
            !can_spawn_golem(world.as_ref(), top),
            "bare air matches nothing"
        );

        world.set_block(
            BlockPos::new(18, 69, 18),
            vanilla_blocks::IRON_BLOCK.default_state(),
            UpdateFlags::UPDATE_CLIENTS,
        );
        world.set_block(
            BlockPos::new(18, 68, 18),
            vanilla_blocks::IRON_BLOCK.default_state(),
            UpdateFlags::UPDATE_CLIENTS,
        );
        assert!(
            can_spawn_golem(world.as_ref(), top),
            "a column of two iron blocks completes the iron base pattern"
        );
    }

    /// `translateAndRotate` builds the pattern's right vector from `forwards × up`, which
    /// is what makes the same aisle literal build a golem in any horizontal rotation.
    #[test]
    fn pattern_cells_follow_the_vanilla_rotation() {
        let origin = BlockPos::new(10, 64, 10);
        // forwards = north, up = up: right is east, so `right` grows the x axis and `down`
        // descends the y axis.
        assert_eq!(
            translate_and_rotate(origin, Direction::North, Direction::Up, 1, 0, 0),
            BlockPos::new(11, 64, 10)
        );
        assert_eq!(
            translate_and_rotate(origin, Direction::North, Direction::Up, 0, 2, 0),
            BlockPos::new(10, 62, 10)
        );
        // forwards = east, up = up: right becomes south (east × up = south).
        assert_eq!(
            translate_and_rotate(origin, Direction::East, Direction::Up, 1, 0, 0),
            BlockPos::new(10, 64, 11)
        );
    }
}
