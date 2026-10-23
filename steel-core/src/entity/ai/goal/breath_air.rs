use std::sync::Arc;

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;
use steel_utils::BlockPos;

use super::selector::{Goal, GoalControls};
use crate::behavior::BlockStateBehaviorExt as _;
use crate::entity::ai::path::PathComputationType;
use crate::entity::{PathfinderMob, SharedEntity};
use crate::physics::MoverType;
use crate::world::LevelReader;

const BREATH_AIR_THRESHOLD: i32 = 140;
/// Vanilla `findAirPosition`'s `getBlockY() + 8` scan ceiling.
const AIR_SEARCH_VERTICAL_ABOVE: i32 = 8;
/// Vanilla `BlockPos.neighborColumn` steps: the mob's column, north, east, south, west.
const NEIGHBOR_COLUMN_STEPS: [(i32, i32); 5] = [(0, 0), (0, -1), (1, 0), (0, 1), (-1, 0)];
const BREATH_MOVE_SPEED: f32 = 0.02;
const AIR_NAVIGATION_SPEED_MODIFIER: f64 = 1.0;

pub struct BreathAirGoal;

impl BreathAirGoal {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self
    }
}

impl Goal for BreathAirGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.air_supply() < BREATH_AIR_THRESHOLD
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
        find_air_position(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        find_air_position(mob);
        let input = mob.travel_input();
        mob.move_relative(
            BREATH_MOVE_SPEED,
            DVec3::new(
                f64::from(input.sideways()),
                f64::from(input.vertical()),
                f64::from(input.forward()),
            ),
        );
        Arc::clone(entity).move_entity(MoverType::SelfMovement, mob.velocity());
    }
}

fn find_air_position(mob: &dyn PathfinderMob) {
    let position = mob.position();
    let destination_pos = mob
        .level()
        .and_then(|world| first_air_position(world.as_ref(), mob))
        .unwrap_or_else(|| {
            BlockPos::containing(
                position.x,
                position.y + f64::from(AIR_SEARCH_VERTICAL_ABOVE),
                position.z,
            )
        });

    mob.move_to_pos(
        DVec3::new(
            f64::from(destination_pos.x()),
            f64::from(destination_pos.y() + 1),
            f64::from(destination_pos.z()),
        ),
        AIR_NAVIGATION_SPEED_MODIFIER,
    );
}

/// Vanilla `BreathAirGoal.findAirPosition`'s scan over
/// `BlockPos.neighborColumn(x, y, z, y + 8)`.
fn first_air_position(level: &dyn LevelReader, mob: &dyn PathfinderMob) -> Option<BlockPos> {
    let origin = mob.block_position();
    let top = origin.y() + AIR_SEARCH_VERTICAL_ABOVE;
    neighbor_column(origin, top, |pos| gives_air(level, pos))
}

/// Vanilla `BlockPos.neighborColumn(startX, startY, startZ, endY)`: the mob's own column
/// first, then north, east, south and west, each scanned bottom-up.
fn neighbor_column(
    origin: BlockPos,
    top: i32,
    mut predicate: impl FnMut(BlockPos) -> bool,
) -> Option<BlockPos> {
    for (step_x, step_z) in NEIGHBOR_COLUMN_STEPS {
        for y in origin.y()..=top {
            let pos = BlockPos::new(origin.x() + step_x, y, origin.z() + step_z);
            if predicate(pos) {
                return Some(pos);
            }
        }
    }
    None
}

fn gives_air(level: &dyn LevelReader, pos: BlockPos) -> bool {
    let state = level.get_block_state(pos);
    (!state.has_fluid() || state.get_block() == &vanilla_blocks::BUBBLE_COLUMN)
        && state.is_pathfindable(PathComputationType::Land)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::*;
    use crate::entity::entities::PigEntity;
    use crate::entity::{Entity, LivingEntity, LivingTravelInput};

    #[test]
    fn breath_air_goal_uses_move_and_look_controls() {
        let goal = BreathAirGoal::new();

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::LOOK);
        assert!(!goal.is_interruptable());
    }

    #[test]
    fn breath_air_goal_uses_vanilla_air_threshold() {
        init_vanilla_registry();
        let mut goal = BreathAirGoal::new();
        let mob = PigEntity::new(&vanilla_entities::PIG, 1, DVec3::ZERO, Weak::new());

        mob.set_air_supply(BREATH_AIR_THRESHOLD);
        assert!(!goal.can_use(&mob));

        mob.set_air_supply(BREATH_AIR_THRESHOLD - 1);
        assert!(goal.can_use(&mob));
        assert!(goal.can_continue_to_use(&mob));
    }

    #[test]
    fn breath_air_goal_tick_applies_travel_input_to_velocity() {
        init_vanilla_registry();
        let mut goal = BreathAirGoal::new();
        let mob = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));
        let mob_entity: SharedEntity = Arc::<PigEntity>::clone(&mob);
        mob.set_travel_input(LivingTravelInput::new(1.0, 0.0, 0.0));

        goal.tick(mob.as_ref(), &mob_entity);

        assert!(mob.velocity().length_squared() > 0.0);
    }

    #[test]
    fn neighbor_column_scans_the_mobs_own_column_first() {
        // Vanilla `BlockPos.neighborColumn`: own column bottom-up, then north, east, south,
        // west. The first air position wins.
        let own_column_air = BlockPos::new(5, 66, 5);
        let north_air = BlockPos::new(5, 64, 4);
        let found = scan_order(|pos| pos == north_air || pos == own_column_air);
        assert_eq!(found, Some(own_column_air));

        let found = scan_order(|pos| pos == BlockPos::new(5, 64, 4));
        assert_eq!(found, Some(BlockPos::new(5, 64, 4)));

        let found = scan_order(|pos| pos == BlockPos::new(6, 64, 5));
        assert_eq!(found, Some(BlockPos::new(6, 64, 5)));

        assert_eq!(scan_order(|_| false), None);
    }

    fn scan_order(predicate: impl FnMut(BlockPos) -> bool) -> Option<BlockPos> {
        neighbor_column(BlockPos::new(5, 64, 5), 64 + 8, predicate)
    }
}
