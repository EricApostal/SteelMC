//! `WitherSkullBlock` behavior (`net.minecraft.world.level.block.WitherSkullBlock`).
//!
//! Placing a wither skeleton skull (floor or wall) runs the wither build check: a
//! `#wither_summon_base_blocks` T with three wither skeleton skulls on top is consumed and a
//! `WITHER` spawns at the pattern's bottom-center cell, invulnerable until its spawn countdown
//! ends (vanilla `WitherBoss.customServerAiStep` plays `SOUND_WITHER_BOSS_SPAWN`).
//!
//! Vanilla reads the placed skull's block entity to obtain its state; Steel reads the world state
//! at the placement position, which is the same value and does not depend on the block entity
//! having been created before `set_placed_by` runs.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::block_behavior;
use steel_registry::advancement::triggers::TriggerEvent;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entities;
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::types::{Difficulty, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Direction, Downcast as _, Identifier, axis::Axis};

use crate::behavior::blocks::SkullBlock;
use crate::behavior::blocks::decoration::skull::abstract_skull_block::AbstractSkullBlock;
use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockLootContext, BlockPlaceContext, PlacementSource,
};
use crate::block_entity::SharedBlockEntity;
use crate::entity::ai::path::PathComputationType;
use crate::entity::entities::WitherBossEntity;
use crate::entity::{
    Entity, add_spawned_entity, create_entity_instance, living_loot_equipment, loot_equipment,
};
use crate::player::advancement::triggers::{
    WorldLevelView, entity_view, event_base, player_view, raise,
};
use crate::world::{LevelReader, World};

/// Width of the wither build pattern (vanilla `BlockPatternBuilder.aisle`).
const PATTERN_WIDTH: i32 = 3;
/// Height of the wither build pattern.
const PATTERN_HEIGHT: i32 = 3;

/// Vanilla `WitherSkullBlock.getOrCreateWitherFull`: three skulls over a
/// `#wither_summon_base_blocks` T with air beside the bottom center.
const WITHER_FULL_ROWS: [&str; 3] = ["^^^", "###", "~#~"];

/// Vanilla `WitherSkullBlock.getOrCreateWitherBase`: the base T alone (used by the
/// dispenser to decide whether a dispensed skull would complete a wither).
const WITHER_BASE_ROWS: [&str; 3] = ["   ", "###", "~#~"];

/// A cell test in a pattern (vanilla `Predicate<BlockInWorld>`).
fn cell_matches(cell: char, world: &dyn LevelReader, pos: BlockPos) -> bool {
    match cell {
        // Vanilla maps `' '` to an always-true predicate.
        ' ' => true,
        '^' => {
            let block = world.get_block_state(pos).get_block();
            block == &vanilla_blocks::WITHER_SKELETON_SKULL
                || block == &vanilla_blocks::WITHER_SKELETON_WALL_SKULL
        }
        '#' => world
            .get_block_state(pos)
            .get_block()
            .has_tag(&BlockTag::WITHER_SUMMON_BASE_BLOCKS),
        '~' => world.get_block_state(pos).is_air(),
        _ => false,
    }
}

/// A matched pattern (vanilla `BlockPattern.BlockPatternMatch`).
struct PatternMatch {
    front_top_left: BlockPos,
    forwards: Direction,
    up: Direction,
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

    /// Vanilla `BlockPatternMatch.getWidth`/`getHeight` cell enumeration.
    fn cells(&self, width: i32, height: i32) -> Vec<BlockPos> {
        let mut cells = Vec::with_capacity((width * height) as usize);
        for right in 0..width {
            for down in 0..height {
                cells.push(self.block(right, down, 0));
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

/// Vanilla `BlockPattern.matches`.
fn matches(
    rows: &[&str; 3],
    world: &dyn LevelReader,
    origin: BlockPos,
    forwards: Direction,
    up: Direction,
) -> bool {
    for (down, row) in rows.iter().enumerate() {
        for (right, cell) in row.chars().enumerate() {
            let pos = translate_and_rotate(origin, forwards, up, right as i32, down as i32, 0);
            if !cell_matches(cell, world, pos) {
                return false;
            }
        }
    }
    true
}

/// Vanilla `BlockPattern.find`: samples every origin in the `dist` cube after `start`,
/// trying each valid `forwards`/`up` pair in `Direction` order.
fn find(rows: &[&str; 3], world: &dyn LevelReader, start: BlockPos) -> Option<PatternMatch> {
    let dist = PATTERN_WIDTH.max(PATTERN_HEIGHT);

    for z in 0..dist {
        for y in 0..dist {
            for x in 0..dist {
                let origin = start.offset(x, y, z);
                for forwards in Direction::ALL {
                    for up in Direction::ALL {
                        if up == forwards || up == forwards.opposite() {
                            continue;
                        }
                        if matches(rows, world, origin, forwards, up) {
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

/// Vanilla `CarvedPumpkinBlock.clearPatternBlocks`: breaks every pattern block down to air
/// and plays the block-break effect.
fn clear_pattern_blocks(world: &Arc<World>, found: &PatternMatch) {
    let air = vanilla_blocks::AIR.default_state();

    for pos in found.cells(PATTERN_WIDTH, PATTERN_HEIGHT) {
        let state = world.get_block_state(pos);
        world.set_block(pos, air, UpdateFlags::UPDATE_CLIENTS);
        world.level_event(
            steel_registry::level_events::PARTICLES_AND_SOUND_DESTROY_BLOCK,
            pos,
            i32::from(state.0),
            None,
        );
    }
}

/// Vanilla `CarvedPumpkinBlock.updatePatternBlocks`: notifies neighbors of every cleared block.
fn update_pattern_blocks(world: &Arc<World>, found: &PatternMatch) {
    for pos in found.cells(PATTERN_WIDTH, PATTERN_HEIGHT) {
        world.update_neighbors_at(pos, &vanilla_blocks::AIR);
    }
}

/// Vanilla `CriteriaTriggers.SUMMONED_ENTITY.trigger` for every player within 50 blocks.
fn raise_summoned_entity(world: &Arc<World>, summoned: &dyn Entity) {
    let area = summoned.bounding_box().inflate(50.0);
    let players = world.get_entities_in_aabb_matching(&area, |entity| entity.as_player().is_some());
    if players.is_empty() {
        return;
    }

    let origin = summoned.block_position();
    let level = WorldLevelView::new(world, origin);
    let summoned_equipment = loot_equipment(summoned);
    let summoned_view = entity_view(summoned, &level, summoned_equipment.as_ref());
    for player_entity in players {
        let Some(player) = player_entity.as_player() else {
            continue;
        };
        let equipment = living_loot_equipment(player);
        let known = player.recipe_book().unlocked_identifiers().to_vec();
        let player_entity_view = player_view(player, &level, Some(&equipment), &known);
        let base = event_base(&player_entity_view, &level, origin);
        raise(
            player,
            &Identifier::vanilla_static("summoned_entity"),
            &TriggerEvent::SummonedEntity {
                base,
                entity: &summoned_view,
            },
        );
    }
}

/// Behavior for wither skull blocks.
#[block_behavior]
pub struct WitherSkullBlock {
    base: SkullBlock,
}

impl WitherSkullBlock {
    /// Creates a new wither skull block behavior for the given block.
    pub const fn new(block: BlockRef) -> Self {
        Self {
            base: SkullBlock::new(block),
        }
    }

    /// Vanilla `WitherSkullBlock.checkSpawn`: a placed wither skeleton skull that completes
    /// the wither build consumes the pattern and spawns a `WITHER`.
    pub fn check_spawn(world: &Arc<World>, pos: BlockPos) {
        let block = world.get_block_state(pos).get_block();
        if block != &vanilla_blocks::WITHER_SKELETON_SKULL
            && block != &vanilla_blocks::WITHER_SKELETON_WALL_SKULL
        {
            return;
        }
        if pos.y() < world.min_y() || world.difficulty() == Difficulty::Peaceful {
            return;
        }

        let Some(found) = find(&WITHER_FULL_ROWS, world.as_ref(), pos) else {
            return;
        };

        // Vanilla creates the wither before clearing the pattern; a missing factory (or a
        // rejected position) leaves the build intact.
        let spawn_pos = found.block(1, 2, 0);
        let spawn_at = DVec3::new(
            f64::from(spawn_pos.x()) + 0.5,
            f64::from(spawn_pos.y()) + 0.55,
            f64::from(spawn_pos.z()) + 0.5,
        );
        let Ok(wither) = create_entity_instance(world, &vanilla_entities::WITHER, spawn_at) else {
            return;
        };

        clear_pattern_blocks(world, &found);

        let rotation = if found.forwards.get_axis() == Axis::X {
            0.0
        } else {
            90.0
        };
        wither.snap_to(spawn_at, rotation, 0.0);
        if let Some(living) = wither.as_living_entity() {
            living.set_y_body_rot(rotation);
        }
        if let Some(wither_boss) = wither.as_ref().downcast_ref::<WitherBossEntity>() {
            wither_boss.make_invulnerable();
        }

        raise_summoned_entity(world, wither.as_ref());

        let _ = add_spawned_entity(world, Arc::clone(&wither));
        update_pattern_blocks(world, &found);
    }

    /// Vanilla `WitherSkullBlock.canSpawnMob`: would a wither skeleton skull placed at `pos`
    /// complete a wither build? Used by the dispenser's skull behavior.
    #[must_use]
    pub fn can_spawn_mob(world: &Arc<World>, pos: BlockPos, stack: &ItemStack) -> bool {
        std::ptr::eq(stack.item(), &*vanilla_items::WITHER_SKELETON_SKULL)
            && pos.y() >= world.min_y() + 2
            && world.difficulty() != Difficulty::Peaceful
            && find(&WITHER_BASE_ROWS, world.as_ref(), pos).is_some()
    }
}

impl BlockBehavior for WitherSkullBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.base.get_state_for_placement(context)
    }

    fn set_placed_by(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source: &PlacementSource<'_>,
    ) {
        // Vanilla `WitherSkullBlock.setPlacedBy`.
        Self::check_spawn(world, pos);
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        source_block: BlockRef,
        moved_by_piston: bool,
    ) {
        self.base
            .handle_neighbor_changed(state, world, pos, source_block, moved_by_piston);
    }

    fn is_pathfindable(&self, state: BlockStateId, computation_type: PathComputationType) -> bool {
        self.base.is_pathfindable(state, computation_type)
    }

    fn new_block_entity(
        &self,
        level: std::sync::Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        self.base.new_block_entity(level, pos, state)
    }

    fn get_clone_item_stack(
        &self,
        block: BlockRef,
        state: BlockStateId,
        block_entity: Option<SharedBlockEntity>,
        include_data: bool,
    ) -> Option<ItemStack> {
        self.base
            .get_clone_item_stack(block, state, block_entity, include_data)
    }

    fn get_drops(
        &self,
        state: BlockStateId,
        context: &BlockLootContext<'_>,
    ) -> Option<Vec<ItemStack>> {
        self.base.get_drops(state, context)
    }
}

impl AbstractSkullBlock for WitherSkullBlock {
    fn state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.base.state_for_placement(context)
    }
}
