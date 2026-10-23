//! Vanilla walk path-type classification.

mod collision;
pub mod fly_node_evaluator;
pub mod node_evaluator;
mod path_evaluator;
mod settings;

pub use collision::WalkNodeCollision;
pub use fly_node_evaluator::{FlightContext, FlyNeighbors, FlyNodeEvaluator};

/// Vanilla `NodeEvaluator`: the evaluator surface `PathFinder` drives.
///
/// Vanilla selects the evaluator by subclass (`WalkNodeEvaluator`,
/// `FlyNodeEvaluator`, …); Steel expresses the same selection with this trait so
/// [`crate::entity::ai::pathfinder::PathFinder::find_path`] stays generic and
/// monomorphised, exactly like vanilla's non-virtual calls after JIT inlining.
pub trait NodeEvaluator {
    /// Vanilla `NodeEvaluator.getStart`.
    fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32;

    /// Vanilla `NodeEvaluator.getNeighbors`.
    fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        collision: &mut impl WalkNodeCollision,
        pos_hash: i32,
    ) -> WalkNeighbors;

    /// Vanilla `NodeEvaluator.getNode`.
    fn node(&self, hash: i32) -> Option<&Node>;

    /// Vanilla `NodeEvaluator.getNode` (mutable).
    fn node_mut(&mut self, hash: i32) -> Option<&mut Node>;

    /// The node store the open set indexes into.
    fn nodes_mut(&mut self) -> &mut NodeStore;

    /// Vanilla `NodeEvaluator.done`'s search-state reset.
    fn reset_search_state(&mut self);
}
pub use node_evaluator::{WalkNeighbors, WalkNodeEvaluator};
pub use path_evaluator::WalkPathEvaluator;
use path_evaluator::does_block_have_partial_collision;
pub use settings::MobPathSettings;

use steel_math::fast_floor;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::fluid::FluidState;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, Direction, WorldAabb, axis::Axis};

use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext, BlockStateBehaviorExt as _};
use crate::entity::Mob;
use crate::entity::ai::node::{Node, NodeStore};
use crate::entity::ai::path::{
    PathComputationType, PathType, PathTypeSet, PathfindingContext, PathfindingMalus,
};
use crate::fluid::FluidStateExt as _;
use crate::world::LevelReader;

#[cfg(test)]
mod tests;

impl NodeEvaluator for WalkNodeEvaluator {
    fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        WalkNodeEvaluator::get_start(self, context)
    }

    fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        collision: &mut impl WalkNodeCollision,
        pos_hash: i32,
    ) -> WalkNeighbors {
        WalkNodeEvaluator::get_neighbors(self, context, collision, pos_hash)
    }

    fn node(&self, hash: i32) -> Option<&Node> {
        WalkNodeEvaluator::node(self, hash)
    }

    fn node_mut(&mut self, hash: i32) -> Option<&mut Node> {
        WalkNodeEvaluator::node_mut(self, hash)
    }

    fn nodes_mut(&mut self) -> &mut NodeStore {
        WalkNodeEvaluator::nodes_mut(self)
    }

    fn reset_search_state(&mut self) {
        WalkNodeEvaluator::reset_search_state(self);
    }
}

impl NodeEvaluator for FlyNodeEvaluator {
    fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        FlyNodeEvaluator::get_start(self, context)
    }

    fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        _collision: &mut impl WalkNodeCollision,
        pos_hash: i32,
    ) -> WalkNeighbors {
        // Vanilla's fly evaluator accepts nodes purely by path-type malus, so the
        // ground collision probe is unused.
        FlyNodeEvaluator::get_neighbors(self, context, pos_hash)
    }

    fn node(&self, hash: i32) -> Option<&Node> {
        FlyNodeEvaluator::node(self, hash)
    }

    fn node_mut(&mut self, hash: i32) -> Option<&mut Node> {
        FlyNodeEvaluator::inner_mut(self).node_mut(hash)
    }

    fn nodes_mut(&mut self) -> &mut NodeStore {
        FlyNodeEvaluator::inner_mut(self).nodes_mut()
    }

    fn reset_search_state(&mut self) {
        FlyNodeEvaluator::inner_mut(self).reset_search_state();
    }
}
