//! Vanilla `SwimNodeEvaluator`: the water-only path evaluator.
//!
//! Vanilla subclasses `NodeEvaluator` directly (it does not reuse the walk
//! neighborhood) and keeps its own path-type cache. Steel mirrors that: this
//! evaluator owns its node store and a per-search path-type map, and accepts
//! only `WATER` nodes (plus `BREACH` for dolphins).

use rustc_hash::FxHashMap;
use steel_utils::{BlockPos, PackedBlockPos};

use super::node_evaluator::VANILLA_HORIZONTAL_DIRECTIONS;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;

use crate::behavior::BlockStateBehaviorExt as _;
use crate::fluid::FluidStateExt as _;

use super::{
    Direction, MobPathSettings, Node, NodeStore, PathComputationType, PathType, PathfindingContext,
    WalkNeighbors, fast_floor,
};

/// Vanilla `SwimNodeEvaluator`.
#[derive(Debug, Clone)]
pub struct SwimNodeEvaluator {
    settings: MobPathSettings,
    nodes: NodeStore,
    allow_breaching: bool,
    path_types_by_pos_cache: FxHashMap<i64, PathType>,
}

impl SwimNodeEvaluator {
    /// Vanilla `SwimNodeEvaluator(boolean allowBreaching)`.
    #[must_use]
    pub fn new(settings: MobPathSettings, allow_breaching: bool) -> Self {
        Self {
            settings,
            nodes: NodeStore::new(),
            allow_breaching,
            path_types_by_pos_cache: FxHashMap::default(),
        }
    }

    #[must_use]
    pub const fn settings(&self) -> &MobPathSettings {
        &self.settings
    }

    /// Vanilla `SwimNodeEvaluator.getStart`.
    pub fn get_start(&mut self, _context: &mut PathfindingContext<'_>) -> i32 {
        let bounding_box = self.settings.bounding_box();
        let x = fast_floor(bounding_box.min_x());
        let y = fast_floor(bounding_box.min_y() + 0.5);
        let z = fast_floor(bounding_box.min_z());
        self.nodes.get_node(x, y, z).hash()
    }

    /// Vanilla `SwimNodeEvaluator.getNeighbors`: the six axis neighbors plus
    /// horizontal diagonals gated on both straight legs.
    pub fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        pos_hash: i32,
    ) -> WalkNeighbors {
        let Some(pos) = self.node(pos_hash) else {
            return WalkNeighbors::new();
        };
        let (pos_x, pos_y, pos_z) = (pos.x, pos.y, pos.z);

        let mut neighbors = WalkNeighbors::new();
        let mut straight: [Option<i32>; 6] = [None; 6];
        for direction in Direction::ALL {
            let (step_x, step_y, step_z) = direction.offset();
            let node =
                self.find_accepted_node(context, pos_x + step_x, pos_y + step_y, pos_z + step_z);
            straight[direction.get_3d_data_value() as usize] = node;
            if self.is_node_valid(node)
                && let Some(hash) = node
            {
                neighbors.push(hash);
            }
        }

        for direction in VANILLA_HORIZONTAL_DIRECTIONS {
            let second_direction = direction.rotate_y_clockwise();
            if !self.has_malus(straight[direction.get_3d_data_value() as usize])
                || !self.has_malus(straight[second_direction.get_3d_data_value() as usize])
            {
                continue;
            }

            let (step_x, _, step_z) = direction.offset();
            let (second_step_x, _, second_step_z) = second_direction.offset();
            let diagonal = self.find_accepted_node(
                context,
                pos_x + step_x + second_step_x,
                pos_y,
                pos_z + step_z + second_step_z,
            );
            if self.is_node_valid(diagonal)
                && let Some(hash) = diagonal
            {
                neighbors.push(hash);
            }
        }

        neighbors
    }

    /// Vanilla `SwimNodeEvaluator.isNodeValid`.
    fn is_node_valid(&self, node: Option<i32>) -> bool {
        node.and_then(|hash| self.node(hash))
            .is_some_and(|node| !node.closed)
    }

    /// Vanilla `SwimNodeEvaluator.hasMalus`.
    fn has_malus(&self, node: Option<i32>) -> bool {
        node.and_then(|hash| self.node(hash))
            .is_some_and(|node| node.cost_malus >= 0.0)
    }

    /// Vanilla `SwimNodeEvaluator.findAcceptedNode`.
    fn find_accepted_node(
        &mut self,
        context: &mut PathfindingContext<'_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<i32> {
        let path_type = self.get_cached_block_type(context, x, y, z);
        if !(self.allow_breaching && path_type == PathType::Breach || path_type == PathType::Water)
        {
            return None;
        }

        let path_cost = self.settings.pathfinding_malus(path_type);
        if path_cost < 0.0 {
            return None;
        }

        let fluid_is_empty = context
            .get_block_state(BlockPos::new(x, y, z))
            .get_fluid_state()
            .is_empty();
        let node = self.nodes.get_node(x, y, z);
        node.path_type = path_type;
        node.cost_malus = node.cost_malus.max(path_cost);
        if fluid_is_empty {
            node.cost_malus += 8.0;
        }
        Some(node.hash())
    }

    /// Vanilla `SwimNodeEvaluator.getCachedBlockType`.
    fn get_cached_block_type(
        &mut self,
        context: &mut PathfindingContext<'_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> PathType {
        let key = PackedBlockPos::from(BlockPos::new(x, y, z)).as_raw();
        if let Some(path_type) = self.path_types_by_pos_cache.get(&key) {
            return *path_type;
        }

        let path_type = self.get_path_type_of_mob(context, x, y, z);
        self.path_types_by_pos_cache.insert(key, path_type);
        path_type
    }

    /// Vanilla `SwimNodeEvaluator.getPathTypeOfMob`.
    ///
    /// Mirrors vanilla exactly, including reading the *last* visited block for
    /// the trailing pathfindable check.
    #[must_use]
    pub fn get_path_type_of_mob(
        &self,
        context: &mut PathfindingContext<'_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> PathType {
        for xx in x..x + self.settings.entity_width() {
            for yy in y..y + self.settings.entity_height() {
                for zz in z..z + self.settings.entity_depth() {
                    let block_state = context.get_block_state(BlockPos::new(xx, yy, zz));
                    let fluid_state = block_state.get_fluid_state();
                    let below_state = context.get_block_state(BlockPos::new(xx, yy - 1, zz));
                    if fluid_state.is_empty()
                        && below_state.is_pathfindable(PathComputationType::Water)
                        && block_state.is_air()
                    {
                        return PathType::Breach;
                    }

                    if !fluid_state.is_water() {
                        return PathType::Blocked;
                    }
                }
            }
        }

        let last = BlockPos::new(
            x + self.settings.entity_width() - 1,
            y + self.settings.entity_height() - 1,
            z + self.settings.entity_depth() - 1,
        );
        if context
            .get_block_state(last)
            .is_pathfindable(PathComputationType::Water)
        {
            PathType::Water
        } else {
            PathType::Blocked
        }
    }

    #[must_use]
    pub fn node(&self, hash: i32) -> Option<&Node> {
        self.nodes.get(hash)
    }

    #[must_use]
    pub const fn nodes_mut(&mut self) -> &mut NodeStore {
        &mut self.nodes
    }

    pub fn reset_search_state(&mut self) {
        self.nodes.reset_search_state();
        // Vanilla `SwimNodeEvaluator.done` clears the per-position path-type map.
        self.path_types_by_pos_cache.clear();
    }
}
