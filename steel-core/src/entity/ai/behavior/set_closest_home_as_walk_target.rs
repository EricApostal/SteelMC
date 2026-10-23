//! Vanilla `SetClosestHomeAsWalkTarget`.

use std::cell::{Cell, RefCell};

use rustc_hash::FxHashMap;
use steel_utils::BlockPos;

use super::poi::{self, Occupancy, PoiPredicate};
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::WalkTarget;

/// Vanilla `SetClosestHomeAsWalkTarget.CACHE_TIMEOUT`.
const CACHE_TIMEOUT: i64 = 40;
/// Vanilla `SetClosestHomeAsWalkTarget.BATCH_SIZE`.
const BATCH_SIZE: i32 = 5;
/// Vanilla `SetClosestHomeAsWalkTarget.RATE`.
const RATE: i64 = 20;
/// Vanilla `SetClosestHomeAsWalkTarget.OK_DISTANCE_SQR`.
const OK_DISTANCE_SQR: f64 = 4.0;
/// Vanilla `SetClosestHomeAsWalkTarget` scan radius.
const SCAN_RANGE: i32 = 48;

/// Vanilla `SetClosestHomeAsWalkTarget`: paths toward the closest free home, retrying
/// every [`RATE`] ticks with a batch cache.
pub struct SetClosestHomeAsWalkTarget {
    speed_modifier: f32,
    batch_cache: FxHashMap<BlockPos, i64>,
    last_update: i64,
}

impl SetClosestHomeAsWalkTarget {
    /// Vanilla `SetClosestHomeAsWalkTarget.create(speedModifier)`.
    #[must_use]
    pub fn create(speed_modifier: f32) -> OneShot<Self> {
        OneShot::new(Self {
            speed_modifier,
            batch_cache: FxHashMap::default(),
            last_update: 0,
        })
        .with_required_memories(vec![&mem::WALK_TARGET, &mem::HOME])
    }
}

impl Trigger for SetClosestHomeAsWalkTarget {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::WALK_TARGET, MemoryStatus::ValueAbsent)
            || !ctx.check_memory(&mem::HOME, MemoryStatus::ValueAbsent)
        {
            return false;
        }

        if ctx.level.game_time() - self.last_update < RATE {
            return false;
        }

        let center = ctx.body.block_position();
        let closest = poi::find_all_closest_first_with_type(
            ctx.level,
            PoiPredicate::Type(&steel_registry::vanilla_poi_types::HOME),
            |_| true,
            center,
            SCAN_RANGE,
            Occupancy::Any,
        )
        .into_iter()
        .next();

        let Some((closest_pos, _)) = closest else {
            return false;
        };
        if dist_sqr(closest_pos, center) <= OK_DISTANCE_SQR {
            return false;
        }

        let tried_count = Cell::new(0);
        self.last_update = ctx.level.game_time() + i64::from(rand::random_range(0..RATE as i32));
        let deadline = self.last_update + CACHE_TIMEOUT;

        let mut cache = std::mem::take(&mut self.batch_cache);
        let pois = {
            let cache_cell = RefCell::new(cache);
            let found = poi::find_all_closest_first_with_type(
                ctx.level,
                PoiPredicate::Type(&steel_registry::vanilla_poi_types::HOME),
                |pos| {
                    let mut cache = cache_cell.borrow_mut();
                    if cache.contains_key(&pos) {
                        return false;
                    }
                    if tried_count.get() >= BATCH_SIZE {
                        return false;
                    }
                    tried_count.set(tried_count.get() + 1);
                    cache.insert(pos, deadline);
                    true
                },
                center,
                SCAN_RANGE,
                Occupancy::Any,
            );
            cache = cache_cell.into_inner();
            found
        };

        let path = ctx
            .body
            .as_pathfinder_mob()
            .and_then(|mob| poi::find_path_to_pois(mob, &pois));
        match path {
            Some(path) if path.can_reach() => {
                let target = path.target();
                if poi::poi_type_at(ctx.level, target).is_some() {
                    ctx.set_memory(
                        &mem::WALK_TARGET,
                        WalkTarget::from_block_pos(target, self.speed_modifier, 1),
                    );
                }
            }
            _ if tried_count.get() < BATCH_SIZE => {
                let last_update = self.last_update;
                cache.retain(|_, expiry| *expiry >= last_update);
            }
            _ => {}
        }

        self.batch_cache = cache;
        true
    }
}

/// Vanilla `BlockPos.distSqr`, squared block-center distance.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    let (ax, ay, az) = a.get_center();
    let (bx, by, bz) = b.get_center();
    (ax - bx).powi(2) + (ay - by).powi(2) + (az - bz).powi(2)
}
