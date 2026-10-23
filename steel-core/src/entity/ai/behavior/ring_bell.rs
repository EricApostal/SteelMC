//! Vanilla `RingBell`: occasionally ring the meeting-point bell.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;

use crate::behavior::blocks::BellBlock;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `RingBell.BELL_RING_CHANCE`: the behavior only rings on the remaining 5%.
const BELL_RING_CHANCE: f32 = 0.95;
/// Vanilla `RingBell.RING_BELL_FROM_DISTANCE`.
pub const RING_BELL_FROM_DISTANCE: f64 = 3.0;

/// Vanilla `RingBell`.
pub struct RingBell;

impl RingBell {
    /// Vanilla `RingBell.create`.
    #[must_use]
    pub fn create() -> OneShot<Self> {
        OneShot::new(Self).with_required_memories(vec![&mem::MEETING_POINT])
    }
}

impl Trigger for RingBell {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if rand::random::<f32>() <= BELL_RING_CHANCE {
            return false;
        }

        let Some(meeting_point) = ctx.get_memory(&mem::MEETING_POINT) else {
            return false;
        };

        let body = ctx.body;
        let pos = meeting_point.pos;
        if !pos.closer_to_center_than(body.position(), RING_BELL_FROM_DISTANCE) {
            return true;
        }

        if ctx.level.get_block_state(pos).get_block() != &vanilla_blocks::BELL {
            return true;
        }

        let Some(world) = body.level() else {
            return true;
        };

        BellBlock::attempt_to_ring(Some(body.as_ref()), &world, pos, None);
        true
    }
}
