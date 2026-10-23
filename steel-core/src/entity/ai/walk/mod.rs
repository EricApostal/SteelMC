//! Vanilla walk path-type classification.

pub mod amphibious_node_evaluator;
mod collision;
pub mod fly_node_evaluator;
pub mod node_evaluator;
mod path_evaluator;
mod settings;
pub mod swim_node_evaluator;

pub use amphibious_node_evaluator::AmphibiousNodeEvaluator;
pub use collision::WalkNodeCollision;
pub use fly_node_evaluator::{FlightContext, FlyNeighbors, FlyNodeEvaluator};
pub use node_evaluator::{AcceptedNodeRequest, WalkNeighbors, WalkNodeEvaluator};
pub use path_evaluator::WalkPathEvaluator;
pub use settings::MobPathSettings;
pub use swim_node_evaluator::SwimNodeEvaluator;

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
use path_evaluator::does_block_have_partial_collision;

use steel_math::fast_floor;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::fluid::FluidState;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, Direction, Identifier, WorldAabb, axis::Axis};

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

impl NodeEvaluator for SwimNodeEvaluator {
    fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        SwimNodeEvaluator::get_start(self, context)
    }

    fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        _collision: &mut impl WalkNodeCollision,
        pos_hash: i32,
    ) -> WalkNeighbors {
        // Vanilla's swim evaluator has no collision probe: water nodes are either
        // free or blocked outright.
        SwimNodeEvaluator::get_neighbors(self, context, pos_hash)
    }

    fn node(&self, hash: i32) -> Option<&Node> {
        SwimNodeEvaluator::node(self, hash)
    }

    fn node_mut(&mut self, hash: i32) -> Option<&mut Node> {
        self.nodes_mut().get_mut(hash)
    }

    fn nodes_mut(&mut self) -> &mut NodeStore {
        SwimNodeEvaluator::nodes_mut(self)
    }

    fn reset_search_state(&mut self) {
        SwimNodeEvaluator::reset_search_state(self);
    }
}

impl NodeEvaluator for AmphibiousNodeEvaluator {
    fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        AmphibiousNodeEvaluator::get_start(self, context)
    }

    fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        collision: &mut impl WalkNodeCollision,
        pos_hash: i32,
    ) -> WalkNeighbors {
        AmphibiousNodeEvaluator::get_neighbors(self, context, collision, pos_hash)
    }

    fn node(&self, hash: i32) -> Option<&Node> {
        AmphibiousNodeEvaluator::node(self, hash)
    }

    fn node_mut(&mut self, hash: i32) -> Option<&mut Node> {
        AmphibiousNodeEvaluator::inner_mut(self).node_mut(hash)
    }

    fn nodes_mut(&mut self) -> &mut NodeStore {
        AmphibiousNodeEvaluator::nodes_mut(self)
    }

    fn reset_search_state(&mut self) {
        AmphibiousNodeEvaluator::reset_search_state(self);
    }
}

/// The evaluator a mob's navigation kind selects.
///
/// Vanilla picks the subclass inside `PathNavigation.createPathFinder`; Steel's
/// navigation stores one [`PathNavigation`](crate::entity::ai::navigation::PathNavigation)
/// per mob, so the choice is made when a search starts and monomorphised
/// through this enum, mirroring vanilla's per-subclass instance field.
#[derive(Debug, Clone)]
pub enum MobNodeEvaluator {
    Walk(WalkNodeEvaluator),
    Fly(FlyNodeEvaluator),
    Swim(SwimNodeEvaluator),
    Amphibious(AmphibiousNodeEvaluator),
}

impl NodeEvaluator for MobNodeEvaluator {
    fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        match self {
            Self::Walk(evaluator) => evaluator.get_start(context),
            Self::Fly(evaluator) => evaluator.get_start(context),
            Self::Swim(evaluator) => evaluator.get_start(context),
            Self::Amphibious(evaluator) => evaluator.get_start(context),
        }
    }

    fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        collision: &mut impl WalkNodeCollision,
        pos_hash: i32,
    ) -> WalkNeighbors {
        match self {
            Self::Walk(evaluator) => evaluator.get_neighbors(context, collision, pos_hash),
            Self::Fly(evaluator) => evaluator.get_neighbors(context, collision, pos_hash),
            Self::Swim(evaluator) => evaluator.get_neighbors(context, collision, pos_hash),
            Self::Amphibious(evaluator) => evaluator.get_neighbors(context, collision, pos_hash),
        }
    }

    fn node(&self, hash: i32) -> Option<&Node> {
        match self {
            Self::Walk(evaluator) => evaluator.node(hash),
            Self::Fly(evaluator) => evaluator.node(hash),
            Self::Swim(evaluator) => evaluator.node(hash),
            Self::Amphibious(evaluator) => evaluator.node(hash),
        }
    }

    fn node_mut(&mut self, hash: i32) -> Option<&mut Node> {
        match self {
            Self::Walk(evaluator) => evaluator.node_mut(hash),
            Self::Fly(evaluator) => evaluator.node_mut(hash),
            Self::Swim(evaluator) => evaluator.node_mut(hash),
            Self::Amphibious(evaluator) => evaluator.node_mut(hash),
        }
    }

    fn nodes_mut(&mut self) -> &mut NodeStore {
        match self {
            Self::Walk(evaluator) => evaluator.nodes_mut(),
            Self::Fly(evaluator) => evaluator.nodes_mut(),
            Self::Swim(evaluator) => evaluator.nodes_mut(),
            Self::Amphibious(evaluator) => evaluator.nodes_mut(),
        }
    }

    fn reset_search_state(&mut self) {
        match self {
            Self::Walk(evaluator) => evaluator.reset_search_state(),
            Self::Fly(evaluator) => evaluator.reset_search_state(),
            Self::Swim(evaluator) => evaluator.reset_search_state(),
            Self::Amphibious(evaluator) => evaluator.reset_search_state(),
        }
    }
}
