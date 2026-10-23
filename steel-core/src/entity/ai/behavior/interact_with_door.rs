//! Vanilla `InteractWithDoor`: open the doors along the current path and close the
//! ones the body has opened and passed through.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_utils::{BlockPos, BlockStateId, GlobalPos};

use crate::behavior::BLOCK_BEHAVIORS;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::node::Node;
use crate::entity::{Mob, SharedEntity};
use crate::world::World;

/// Vanilla `InteractWithDoor.COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE`.
const COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE: i32 = 20;
/// Vanilla `InteractWithDoor.SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN`.
const SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN: f64 = 3.0;
/// Vanilla `InteractWithDoor.MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS`.
const MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS: f64 = 2.0;

const OPEN: &BoolProperty = &BlockStateProperties::OPEN;

/// Vanilla `DoorBlock` class check, expressed with the door block-state properties.
fn is_door_state(state: BlockStateId) -> bool {
    state
        .try_get_value(&BlockStateProperties::DOOR_HINGE)
        .is_some()
        && state
            .try_get_value(&BlockStateProperties::DOUBLE_BLOCK_HALF)
            .is_some()
}

/// Vanilla `state.is(BlockTags.MOB_INTERACTABLE_DOORS, s -> s.getBlock() instanceof DoorBlock)`.
fn is_interactable_door(state: BlockStateId) -> bool {
    state.get_block().has_tag(&BlockTag::MOB_INTERACTABLE_DOORS) && is_door_state(state)
}

/// Vanilla `InteractWithDoor`.
pub struct InteractWithDoor {
    last_checked_node: Option<BlockPos>,
    remaining_cooldown: i32,
}

impl InteractWithDoor {
    /// Vanilla `InteractWithDoor.create`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self {
            last_checked_node: None,
            remaining_cooldown: 0,
        })
        .with_required_memories(vec![
            &mem::PATH,
            &mem::DOORS_TO_CLOSE,
            &mem::NEAREST_LIVING_ENTITIES,
        ])
    }
}

impl Trigger for InteractWithDoor {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(path) = ctx.get_memory(&mem::PATH) else {
            return false;
        };

        if path.not_started() || path.is_done() {
            return false;
        }

        let Some(next_node) = path.next_node() else {
            return false;
        };
        let next_pos = next_node.as_block_pos();

        if self.last_checked_node == Some(next_pos) {
            self.remaining_cooldown = COOLDOWN_BEFORE_RERUNNING_IN_SAME_NODE;
        } else {
            self.remaining_cooldown -= 1;
            if self.remaining_cooldown > 0 {
                return false;
            }
        }

        self.last_checked_node = Some(next_pos);

        let from_pos = path.previous_node().map(Node::as_block_pos);
        let Some(world) = ctx.body.level() else {
            return false;
        };

        let mut doors = ctx.get_memory(&mem::DOORS_TO_CLOSE).unwrap_or_default();

        if let Some(from_pos) = from_pos {
            open_path_door(&world, ctx.body, from_pos, &mut doors);
        }
        open_path_door(&world, ctx.body, next_pos, &mut doors);

        let nearest_entities = ctx.get_memory(&mem::NEAREST_LIVING_ENTITIES);
        close_doors_that_i_have_opened_or_passed_through(
            &world,
            ctx.body,
            from_pos,
            Some(next_pos),
            &mut doors,
            nearest_entities.as_deref(),
        );

        ctx.set_memory(&mem::DOORS_TO_CLOSE, doors);
        true
    }
}

/// Vanilla `InteractWithDoor`'s from/to node door handling: opens the door and
/// remembers it to close later.
fn open_path_door(
    world: &Arc<World>,
    body: &SharedEntity,
    pos: BlockPos,
    doors: &mut Vec<GlobalPos>,
) {
    let state = world.get_block_state(pos);
    if !is_interactable_door(state) {
        return;
    }

    if !state.get_value(OPEN) {
        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .set_door_open(state, world, pos, Some(body.as_ref()), true);
    }

    remember_door_to_close(world, pos, doors);
}

/// Vanilla `InteractWithDoor.rememberDoorToClose`.
fn remember_door_to_close(world: &World, pos: BlockPos, doors: &mut Vec<GlobalPos>) {
    let global_pos = GlobalPos::new(world.key.clone(), pos);
    if !doors.contains(&global_pos) {
        doors.push(global_pos);
    }
}

/// Vanilla `InteractWithDoor.closeDoorsThatIHaveOpenedOrPassedThrough`.
pub fn close_doors_that_i_have_opened_or_passed_through(
    world: &Arc<World>,
    body: &SharedEntity,
    moving_from_pos: Option<BlockPos>,
    moving_to_pos: Option<BlockPos>,
    doors: &mut Vec<GlobalPos>,
    nearest_entities: Option<&[SharedEntity]>,
) {
    doors.retain(|door_global_pos| {
        let door_pos = door_global_pos.pos;
        if moving_from_pos == Some(door_pos) || moving_to_pos == Some(door_pos) {
            return true;
        }

        if is_door_too_far_away(world, body, door_global_pos) {
            return false;
        }

        let state = world.get_block_state(door_pos);
        if !is_interactable_door(state) {
            return false;
        }

        if !state.get_value(OPEN) {
            return false;
        }

        if are_other_mobs_coming_through_door(body, door_pos, nearest_entities) {
            return false;
        }

        BLOCK_BEHAVIORS
            .get_behavior(state.get_block())
            .set_door_open(state, world, door_pos, Some(body.as_ref()), false);
        false
    });
}

/// Vanilla `InteractWithDoor.isDoorTooFarAway`.
fn is_door_too_far_away(world: &World, body: &SharedEntity, door_global_pos: &GlobalPos) -> bool {
    door_global_pos.dimension != world.key
        || !door_global_pos
            .pos
            .closer_to_center_than(body.position(), SKIP_CLOSING_DOOR_IF_FURTHER_AWAY_THAN)
}

/// Vanilla `InteractWithDoor.areOtherMobsComingThroughDoor`.
fn are_other_mobs_coming_through_door(
    body: &SharedEntity,
    door_pos: BlockPos,
    nearest_entities: Option<&[SharedEntity]>,
) -> bool {
    let Some(nearest_entities) = nearest_entities else {
        return false;
    };

    nearest_entities
        .iter()
        .filter(|other| other.entity_type() == body.entity_type())
        .filter(|other| {
            door_pos.closer_to_center_than(
                other.position(),
                MAX_DISTANCE_TO_HOLD_DOOR_OPEN_FOR_OTHER_MOBS,
            )
        })
        .any(|other| is_mob_coming_through_door(other, door_pos))
}

/// Vanilla `InteractWithDoor.isMobComingThroughDoor`.
fn is_mob_coming_through_door(other: &SharedEntity, door_pos: BlockPos) -> bool {
    let Some(brain) = other.as_mob().and_then(Mob::brain) else {
        return false;
    };

    let brain = brain.lock();
    let Some(path) = brain.get_memory(&mem::PATH) else {
        return false;
    };

    if path.is_done() {
        return false;
    }

    let Some(from_pos) = path.previous_node().map(Node::as_block_pos) else {
        return false;
    };

    let to_pos = path.next_node().map(Node::as_block_pos);
    Some(door_pos) == Some(from_pos) || Some(door_pos) == to_pos
}
