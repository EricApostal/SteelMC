//! Vanilla `FlyNodeEvaluator`: the 26-neighborhood flight evaluator.
//!
//! Vanilla subclasses `WalkNodeEvaluator` and overrides the start node, the
//! neighbour set, `findAcceptedNode` and `getPathType`. Steel keeps the same
//! relationship by composition: [`FlyNodeEvaluator`] owns a [`WalkNodeEvaluator`]
//! for node storage and mob-bounding-box path types, and replaces only the
//! ground-specific parts (jump heights, floor levels, water/ground fallbacks).

use glam::DVec3;
use steel_utils::BlockPos;

use super::WalkNodeEvaluator;
use super::path_evaluator::WalkPathEvaluator;
use super::settings::MobPathSettings;
use crate::entity::ai::node::Node;
use crate::entity::ai::path::{PathType, PathfindingContext};

/// Vanilla `FlyNodeEvaluator.SMALL_MOB_SIZE`.
const SMALL_MOB_SIZE: f64 = 1.0;
/// Vanilla `FlyNodeEvaluator.SMALL_MOB_INFLATED_START_NODE_BOUNDING_BOX`.
const SMALL_MOB_INFLATED_START_NODE_BOUNDING_BOX: f64 = 1.1;
/// Vanilla `FlyNodeEvaluator.MAX_START_NODE_CANDIDATES`.
const MAX_START_NODE_CANDIDATES: usize = 10;

/// Vanilla fly `getNeighbors` writes at most 26 nodes; Steel reuses the shared
/// [`WalkNeighbors`] buffer, which is sized for the larger set.
pub use super::node_evaluator::WalkNeighbors as FlyNeighbors;

/// Vanilla `FlyNodeEvaluator.getStart` reads `this.mob`; Steel's evaluators do not
/// hold the mob, so the flight-relevant parts are captured by [`prepare`].
///
/// [`prepare`]: FlyNodeEvaluator::prepare
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlightContext {
    /// Vanilla `mob.getX/Y/Z()`.
    pub position: DVec3,
    /// Vanilla `mob.getBoundingBox()` as `[min_x, min_y, min_z, max_x, max_y, max_z]`.
    pub bounding_box: [f64; 6],
    /// Vanilla `mob.isInWater()`.
    pub in_water: bool,
}

/// Vanilla `FlyNodeEvaluator extends WalkNodeEvaluator`.
#[derive(Debug, Clone)]
pub struct FlyNodeEvaluator {
    inner: WalkNodeEvaluator,
    flight: FlightContext,
}

impl FlyNodeEvaluator {
    #[must_use]
    pub fn new(settings: MobPathSettings) -> Self {
        Self {
            inner: WalkNodeEvaluator::new(settings),
            flight: FlightContext {
                position: DVec3::ZERO,
                bounding_box: [0.0; 6],
                in_water: false,
            },
        }
    }

    /// Vanilla `FlyNodeEvaluator.prepare`: records the mob state `getStart` reads.
    pub const fn prepare(&mut self, flight: FlightContext) {
        self.flight = flight;
    }

    /// The recorded flight context.
    #[must_use]
    pub const fn flight(&self) -> FlightContext {
        self.flight
    }

    /// The wrapped walk evaluator: shared settings, node store and collisions.
    #[must_use]
    pub const fn inner(&self) -> &WalkNodeEvaluator {
        &self.inner
    }

    #[must_use]
    pub const fn inner_mut(&mut self) -> &mut WalkNodeEvaluator {
        &mut self.inner
    }

    /// Vanilla `FlyNodeEvaluator.getPathType`: an open node inherits the block below,
    /// then the neighbour-block scan applies.
    #[must_use]
    pub fn get_path_type(context: &mut PathfindingContext<'_>, x: i32, y: i32, z: i32) -> PathType {
        let mut block_path_type = context.get_path_type_from_state(x, y, z);
        if block_path_type == PathType::Open && y > context.level().min_y() {
            let below = context.get_path_type_from_state(x, y - 1, z);
            block_path_type = match below {
                PathType::Fire | PathType::Lava => PathType::Fire,
                PathType::Damaging => PathType::Damaging,
                PathType::Cocoa => PathType::Cocoa,
                // Vanilla keeps `OPEN` for a fence directly under the mob.
                PathType::Fence => {
                    if context.mob_position() == BlockPos::new(x, y - 1, z) {
                        PathType::Open
                    } else {
                        PathType::Fence
                    }
                }
                PathType::Walkable | PathType::Open | PathType::Water => PathType::Open,
                _ => PathType::Walkable,
            };
        }

        if block_path_type == PathType::Walkable || block_path_type == PathType::Open {
            return WalkPathEvaluator::check_neighbour_blocks(context, x, y, z, block_path_type);
        }

        block_path_type
    }

    /// Vanilla `FlyNodeEvaluator.findAcceptedNode`: no jump height, floor level or
    /// water/ground fallback — a node is accepted when its malus is non-negative.
    fn find_accepted_node(
        &mut self,
        context: &mut PathfindingContext<'_>,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<i32> {
        let path_type = Self::get_path_type(context, x, y, z);
        let path_cost = self.inner.settings().pathfinding_malus(path_type);
        if path_cost < 0.0 {
            return None;
        }

        Some(
            self.inner
                .get_node_and_update_cost_to_max(x, y, z, path_type, path_cost),
        )
    }

    /// Vanilla `FlyNodeEvaluator.hasMalus`.
    fn has_malus(&self, node: Option<i32>) -> bool {
        node.and_then(|hash| self.inner.node(hash))
            .is_some_and(|node| node.cost_malus >= 0.0)
    }

    /// Vanilla `FlyNodeEvaluator.isOpen`.
    fn is_open(&self, node: Option<i32>) -> bool {
        node.and_then(|hash| self.inner.node(hash))
            .is_some_and(|node| !node.closed)
    }

    /// Vanilla `FlyNodeEvaluator.getNeighbors`: the full 26-neighborhood, each
    /// diagonal gated on the straight legs that lead to it.
    #[expect(
        clippy::too_many_lines,
        reason = "keeping vanilla's ordered 26-neighborhood gating together makes parity auditable"
    )]
    pub fn get_neighbors(
        &mut self,
        context: &mut PathfindingContext<'_>,
        pos_hash: i32,
    ) -> FlyNeighbors {
        let Some(pos) = self.inner.node(pos_hash) else {
            return FlyNeighbors::new();
        };
        let (pos_x, pos_y, pos_z) = (pos.x, pos.y, pos.z);

        let mut neighbors = FlyNeighbors::new();

        macro_rules! push_if {
            ($x:expr, $y:expr, $z:expr, $gate:expr) => {{
                let candidate = self.find_accepted_node(context, $x, $y, $z);
                if self.is_open(candidate) && $gate {
                    match candidate {
                        Some(hash) => neighbors.push(hash),
                        None => {}
                    }
                }
                candidate
            }};
        }

        let south = push_if!(pos_x, pos_y, pos_z + 1, true);
        let west = push_if!(pos_x - 1, pos_y, pos_z, true);
        let east = push_if!(pos_x + 1, pos_y, pos_z, true);
        let north = push_if!(pos_x, pos_y, pos_z - 1, true);
        let up = push_if!(pos_x, pos_y + 1, pos_z, true);
        let down = push_if!(pos_x, pos_y - 1, pos_z, true);

        let south_up = push_if!(
            pos_x,
            pos_y + 1,
            pos_z + 1,
            self.has_malus(south) && self.has_malus(up)
        );
        let west_up = push_if!(
            pos_x - 1,
            pos_y + 1,
            pos_z,
            self.has_malus(west) && self.has_malus(up)
        );
        let east_up = push_if!(
            pos_x + 1,
            pos_y + 1,
            pos_z,
            self.has_malus(east) && self.has_malus(up)
        );
        let north_up = push_if!(
            pos_x,
            pos_y + 1,
            pos_z - 1,
            self.has_malus(north) && self.has_malus(up)
        );
        let south_down = push_if!(
            pos_x,
            pos_y - 1,
            pos_z + 1,
            self.has_malus(south) && self.has_malus(down)
        );
        let west_down = push_if!(
            pos_x - 1,
            pos_y - 1,
            pos_z,
            self.has_malus(west) && self.has_malus(down)
        );
        let east_down = push_if!(
            pos_x + 1,
            pos_y - 1,
            pos_z,
            self.has_malus(east) && self.has_malus(down)
        );
        let north_down = push_if!(
            pos_x,
            pos_y - 1,
            pos_z - 1,
            self.has_malus(north) && self.has_malus(down)
        );

        let north_east = push_if!(
            pos_x + 1,
            pos_y,
            pos_z - 1,
            self.has_malus(north) && self.has_malus(east)
        );
        let south_east = push_if!(
            pos_x + 1,
            pos_y,
            pos_z + 1,
            self.has_malus(south) && self.has_malus(east)
        );
        let north_west = push_if!(
            pos_x - 1,
            pos_y,
            pos_z - 1,
            self.has_malus(north) && self.has_malus(west)
        );
        let south_west = push_if!(
            pos_x - 1,
            pos_y,
            pos_z + 1,
            self.has_malus(south) && self.has_malus(west)
        );

        push_if!(
            pos_x + 1,
            pos_y + 1,
            pos_z - 1,
            self.has_malus(north_east)
                && self.has_malus(north)
                && self.has_malus(east)
                && self.has_malus(up)
                && self.has_malus(north_up)
                && self.has_malus(east_up)
        );
        push_if!(
            pos_x + 1,
            pos_y + 1,
            pos_z + 1,
            self.has_malus(south_east)
                && self.has_malus(south)
                && self.has_malus(east)
                && self.has_malus(up)
                && self.has_malus(south_up)
                && self.has_malus(east_up)
        );
        push_if!(
            pos_x - 1,
            pos_y + 1,
            pos_z - 1,
            self.has_malus(north_west)
                && self.has_malus(north)
                && self.has_malus(west)
                && self.has_malus(up)
                && self.has_malus(north_up)
                && self.has_malus(west_up)
        );
        push_if!(
            pos_x - 1,
            pos_y + 1,
            pos_z + 1,
            self.has_malus(south_west)
                && self.has_malus(south)
                && self.has_malus(west)
                && self.has_malus(up)
                && self.has_malus(south_up)
                && self.has_malus(west_up)
        );
        push_if!(
            pos_x + 1,
            pos_y - 1,
            pos_z - 1,
            self.has_malus(north_east)
                && self.has_malus(north)
                && self.has_malus(east)
                && self.has_malus(down)
                && self.has_malus(north_down)
                && self.has_malus(east_down)
        );
        push_if!(
            pos_x + 1,
            pos_y - 1,
            pos_z + 1,
            self.has_malus(south_east)
                && self.has_malus(south)
                && self.has_malus(east)
                && self.has_malus(down)
                && self.has_malus(south_down)
                && self.has_malus(east_down)
        );
        push_if!(
            pos_x - 1,
            pos_y - 1,
            pos_z - 1,
            self.has_malus(north_west)
                && self.has_malus(north)
                && self.has_malus(west)
                && self.has_malus(down)
                && self.has_malus(north_down)
                && self.has_malus(west_down)
        );
        push_if!(
            pos_x - 1,
            pos_y - 1,
            pos_z + 1,
            self.has_malus(south_west)
                && self.has_malus(south)
                && self.has_malus(west)
                && self.has_malus(down)
                && self.has_malus(south_down)
                && self.has_malus(west_down)
        );

        neighbors
    }

    /// Vanilla `FlyNodeEvaluator.canStartAt`.
    fn can_start_at(&mut self, context: &mut PathfindingContext<'_>, pos: BlockPos) -> bool {
        let path_type = Self::get_path_type(context, pos.x(), pos.y(), pos.z());
        self.inner.settings().pathfinding_malus(path_type) >= 0.0
    }

    /// Vanilla `FlyNodeEvaluator.iteratePathfindingStartNodeCandidatePositions`.
    ///
    /// Vanilla draws these from the mob's `RandomSource`; Steel's server RNG is
    /// shared, and the candidate order is only observable through which start node
    /// wins, so a local source is equivalent.
    fn start_node_candidates(mob_position: DVec3, bounding_box: [f64; 6]) -> Vec<BlockPos> {
        let block_y = mob_position.y.floor() as i32;
        let x_size = bounding_box[3] - bounding_box[0];
        let y_size = bounding_box[4] - bounding_box[1];
        let z_size = bounding_box[5] - bounding_box[2];
        let is_small_mob = (x_size + y_size + z_size) / 3.0 < SMALL_MOB_SIZE;

        if !is_small_mob {
            return vec![
                BlockPos::containing(bounding_box[0], f64::from(block_y), bounding_box[2]),
                BlockPos::containing(bounding_box[0], f64::from(block_y), bounding_box[5]),
                BlockPos::containing(bounding_box[3], f64::from(block_y), bounding_box[2]),
                BlockPos::containing(bounding_box[3], f64::from(block_y), bounding_box[5]),
            ];
        }

        let x_padding = (SMALL_MOB_INFLATED_START_NODE_BOUNDING_BOX - x_size).max(0.0);
        let y_padding = (SMALL_MOB_INFLATED_START_NODE_BOUNDING_BOX - y_size).max(0.0);
        let z_padding = (SMALL_MOB_INFLATED_START_NODE_BOUNDING_BOX - z_size).max(0.0);

        let min_x = (bounding_box[0] - x_padding).floor() as i32;
        let max_x = (bounding_box[3] + x_padding).floor() as i32;
        let min_y = (bounding_box[1] - y_padding).floor() as i32;
        let max_y = (bounding_box[4] + y_padding).floor() as i32;
        let min_z = (bounding_box[2] - z_padding).floor() as i32;
        let max_z = (bounding_box[5] + z_padding).floor() as i32;

        (0..MAX_START_NODE_CANDIDATES)
            .map(|_| {
                BlockPos::new(
                    rand::random_range(min_x..=max_x),
                    rand::random_range(min_y..=max_y),
                    rand::random_range(min_z..=max_z),
                )
            })
            .collect()
    }

    /// Vanilla `FlyNodeEvaluator.getStart`.
    ///
    /// `bounding_box` is `[min_x, min_y, min_z, max_x, max_y, max_z]`, matching the
    /// vanilla `AABB` accessors the start-node candidates read.
    pub fn get_start(&mut self, context: &mut PathfindingContext<'_>) -> i32 {
        let FlightContext {
            position: mob_position,
            bounding_box,
            in_water,
        } = self.flight;
        self.get_start_with(context, mob_position, bounding_box, in_water)
    }

    /// Vanilla `FlyNodeEvaluator.getStart` with the mob state passed explicitly.
    pub fn get_start_with(
        &mut self,
        context: &mut PathfindingContext<'_>,
        mob_position: DVec3,
        bounding_box: [f64; 6],
        in_water: bool,
    ) -> i32 {
        let start_y = if self.inner.settings().can_float() && in_water {
            let mut start_y = mob_position.y.floor() as i32;
            while context.get_path_type_from_state(
                mob_position.x.floor() as i32,
                start_y,
                mob_position.z.floor() as i32,
            ) == PathType::Water
            {
                start_y += 1;
            }
            start_y
        } else {
            (mob_position.y + 0.5).floor() as i32
        };

        let start_pos = BlockPos::containing(mob_position.x, f64::from(start_y), mob_position.z);
        if self.can_start_at(context, start_pos) {
            return self.inner.get_start_node(context, start_pos);
        }

        for candidate in Self::start_node_candidates(mob_position, bounding_box) {
            if self.can_start_at(context, candidate) {
                return self.inner.get_start_node(context, candidate);
            }
        }

        self.inner.get_start_node(context, start_pos)
    }

    /// Vanilla `FlyNodeEvaluator.getTarget`.
    #[expect(clippy::unused_self, reason = "this is an api function")]
    #[must_use]
    pub const fn get_target(&self, x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(x, y, z)
    }

    /// The node behind `hash`, for the path finder's neighbour iteration.
    #[must_use]
    pub fn node(&self, hash: i32) -> Option<&Node> {
        self.inner.node(hash)
    }
}
