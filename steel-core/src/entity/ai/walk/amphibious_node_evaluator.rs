//! Vanilla `AmphibiousNodeEvaluator`: walk pathfinding plus a swim column.
//!
//! Vanilla subclasses `WalkNodeEvaluator`; Steel folds that subclass into
//! [`WalkNodeEvaluator`](super::node_evaluator::WalkNodeEvaluator): the
//! amphibious [`Self::get_start`] branch, the vertical swim-column
//! [`Self::get_neighbors`] extension, and the water-border
//! [`WalkNodeEvaluator::get_path_type`] classification live there behind the
//! `amphibious` flag. This type is the `AmphibiousNodeEvaluator`-shaped facade
//! over that shared search surface: it configures the flags vanilla's
//! constructor and `prepare`/`done` set, so searches read as their vanilla
//! subclasses (`Frog.FrogNodeEvaluator`'s prefer-jump-to tag and shallow
//! preference included).

use steel_utils::{BlockPos, Identifier};

use super::{
    MobPathSettings, NodeStore, PathType, PathfindingContext, WalkNeighbors, WalkNodeCollision,
    WalkNodeEvaluator, fast_floor,
};

/// Vanilla `AmphibiousNodeEvaluator(false)` / `Frog.FrogNodeEvaluator(true)`.
#[derive(Debug, Clone)]
pub struct AmphibiousNodeEvaluator {
    inner: WalkNodeEvaluator,
    /// Vanilla `AmphibiousNodeEvaluator.prefersShallowSwimming`.
    prefers_shallow_swimming: bool,
}

impl AmphibiousNodeEvaluator {
    /// Vanilla `AmphibiousNodeEvaluator(boolean prefersShallowSwimming)`; the
    /// frog subclass passes `true`.
    #[must_use]
    pub fn new(settings: MobPathSettings, prefers_shallow_swimming: bool) -> Self {
        let settings = settings
            .with_pathfinding_malus(PathType::Water, 0.0)
            .with_pathfinding_malus(PathType::Walkable, 6.0)
            .with_pathfinding_malus(PathType::WaterBorder, 4.0);
        Self {
            inner: WalkNodeEvaluator::new(settings).with_amphibious(true),
            prefers_shallow_swimming,
        }
    }

    /// Vanilla `Frog.FrogNodeEvaluator(boolean prefersShallowSwimming)`'s
    /// prefer-jump-to rule: positions above `FROG_PREFER_JUMP_TO` report
    /// `PathType::Open`.
    #[must_use]
    pub fn with_prefer_jump_to(mut self, tag: &'static Identifier) -> Self {
        let settings = self.inner.settings().clone();
        self.inner = WalkNodeEvaluator::new(settings)
            .with_amphibious(true)
            .with_prefer_jump_to_tag(Some(tag));
        self
    }

    /// Vanilla `AmphibiousNodeEvaluator.getStart`.
    pub fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        if !self.inner.settings().in_water() {
            return self.inner.get_start(context);
        }

        let position = self.inner.settings().mob_position_vec();
        self.inner.get_start_node(
            context,
            BlockPos::new(
                fast_floor(position.x),
                fast_floor(position.y + 0.5),
                fast_floor(position.z),
            ),
        )
    }

    /// Vanilla `Frog.FrogNodeEvaluator.getStart`'s in-water branch: floors the
    /// position without the +0.5 the shared branch adds.
    pub fn get_start_frog_swimming(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        if !self.inner.settings().in_water() {
            return self.inner.get_start(context);
        }

        let position = self.inner.settings().mob_position_vec();
        self.inner.get_start_node(
            context,
            BlockPos::new(
                fast_floor(position.x),
                fast_floor(position.y),
                fast_floor(position.z),
            ),
        )
    }

    /// Vanilla `AmphibiousNodeEvaluator.getNeighbors`: the walk neighborhood
    /// plus the swim-column nodes directly above and below the current node.
    pub fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        collision: &mut impl WalkNodeCollision,
        pos_hash: i32,
    ) -> WalkNeighbors {
        let mut neighbors = self.inner.get_neighbors(context, collision, pos_hash);
        let Some(pos) = self.inner.node(pos_hash) else {
            return neighbors;
        };
        let (pos_x, pos_y, pos_z) = (pos.x, pos.y, pos.z);
        let current = self
            .inner
            .get_cached_path_type(context, pos_x, pos_y, pos_z);

        let above = self
            .inner
            .get_cached_path_type(context, pos_x, pos_y + 1, pos_z);
        let jump_size = if self.inner.settings().pathfinding_malus(above) >= 0.0
            && current != PathType::StickyHoney
        {
            fast_floor(f64::from(self.inner.settings().max_up_step()).max(1.0))
        } else {
            0
        };
        let node_height = self
            .inner
            .get_floor_level(context, BlockPos::new(pos_x, pos_y, pos_z));

        let up = self.inner.find_accepted_node(
            context,
            collision,
            super::node_evaluator::AcceptedNodeRequest {
                pos: BlockPos::new(pos_x, pos_y + 1, pos_z),
                jump_size,
                node_height,
                travel_direction: super::Direction::Up,
                current_path_type: current,
            },
        );
        if up.is_some_and(|hash| self.is_vertical_neighbor_valid(hash, pos_hash)) {
            neighbors.push(up.expect("validated amphibious up node"));
        }

        let down = self.inner.find_accepted_node(
            context,
            collision,
            super::node_evaluator::AcceptedNodeRequest {
                pos: BlockPos::new(pos_x, pos_y - 1, pos_z),
                jump_size,
                node_height,
                travel_direction: super::Direction::Down,
                current_path_type: current,
            },
        );
        if down.is_some_and(|hash| {
            self.is_vertical_neighbor_valid(hash, pos_hash) && current != PathType::Trapdoor
        }) {
            neighbors.push(down.expect("validated amphibious down node"));
        }

        if self.prefers_shallow_swimming {
            let sea_level = context.level().sea_level();
            for hash in neighbors.iter() {
                let Some(node) = self.inner.node_mut(hash) else {
                    continue;
                };
                if node.path_type == PathType::Water && node.y < sea_level - 10 {
                    node.cost_malus += 1.0;
                }
            }
        }

        neighbors
    }

    /// Vanilla `AmphibiousNodeEvaluator.isVerticalNeighborValid`.
    fn is_vertical_neighbor_valid(&self, vertical_hash: i32, pos_hash: i32) -> bool {
        let Some(vertical) = self.inner.node(vertical_hash) else {
            return false;
        };
        let Some(pos) = self.inner.node(pos_hash) else {
            return false;
        };
        !vertical.closed
            && (vertical.cost_malus >= 0.0 || pos.cost_malus < 0.0)
            && vertical.path_type == PathType::Water
    }

    #[must_use]
    pub fn node(&self, hash: i32) -> Option<&super::Node> {
        self.inner.node(hash)
    }

    #[must_use]
    pub const fn nodes_mut(&mut self) -> &mut NodeStore {
        self.inner.nodes_mut()
    }

    pub fn reset_search_state(&mut self) {
        self.inner.reset_search_state();
    }

    /// The wrapped walk evaluator, for tests over the shared search surface.
    #[must_use]
    pub const fn inner(&self) -> &WalkNodeEvaluator {
        &self.inner
    }

    #[must_use]
    pub const fn inner_mut(&mut self) -> &mut WalkNodeEvaluator {
        &mut self.inner
    }
}
