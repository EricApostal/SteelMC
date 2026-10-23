//! Vanilla `UseBonemeal`.

use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use steel_utils::BlockPos;
use steel_utils::locks::SyncMutex;

use super::harvest_farmland::{is_crop, is_mature_crop};
use super::utils;
use super::villager;
use crate::behavior::BLOCK_BEHAVIORS;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};
use crate::world::World;

/// Vanilla `UseBonemeal.BONEMEALING_DURATION`.
const BONEMEALING_DURATION: i32 = 80;
/// Vanilla `UseBonemeal` work reach, `closerToCenterThan(..., 1.0)`.
const REACH_DISTANCE: f64 = 1.0;
/// Vanilla `UseBonemeal` pick-up interval gate, `tickCount % 10 == 0`.
const PICK_INTERVAL: i32 = 10;
/// Vanilla `lastBonemealingSession + 160L <= tickCount` session cooldown.
const SESSION_COOLDOWN: i64 = 160;
/// Vanilla `nextWorkCycleTime = timestamp + 40L` between crops.
const WORK_CYCLE: i64 = 40;
/// Vanilla walk speed to the crop.
const SPEED_MODIFIER: f32 = 0.5;

/// Vanilla `UseBonemeal`: a villager walks to a nearby immature crop and applies bone
/// meal from its inventory.
pub struct UseBonemeal {
    state: BehaviorState,
    // Vanilla mutates these in `checkExtraStartConditions`, which Steel hands out as
    // `&self`, plus from `tick`; a lock keeps both paths uniform.
    inner: SyncMutex<UseBonemealState>,
}

/// The mutable half of `UseBonemeal`.
struct UseBonemealState {
    next_work_cycle_time: i64,
    last_bonemealing_session: i64,
    time_worked_so_far: i32,
    crop_pos: Option<BlockPos>,
}

impl UseBonemeal {
    /// Vanilla `UseBonemeal()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::with_entry_condition(vec![
                (
                    &mem::LOOK_TARGET as MemoryModuleTypeRef,
                    MemoryStatus::ValueAbsent,
                ),
                (
                    &mem::WALK_TARGET as MemoryModuleTypeRef,
                    MemoryStatus::ValueAbsent,
                ),
            ]),
            inner: SyncMutex::new(UseBonemealState {
                next_work_cycle_time: 0,
                last_bonemealing_session: 0,
                time_worked_so_far: 0,
                crop_pos: None,
            }),
        }
    }

    /// Vanilla `UseBonemeal.pickNextTarget`.
    fn pick_next_target(level: &World, body: &SharedEntity) -> Option<BlockPos> {
        let base = body.block_position();
        let mut result = None;
        let mut count = 0;

        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let pos = BlockPos::new(base.x() + x, base.y() + y, base.z() + z);
                    if Self::valid_pos(level, pos) {
                        count += 1;
                        if rand::random_range(0..count) == 0 {
                            result = Some(pos);
                        }
                    }
                }
            }
        }

        result
    }

    /// Vanilla `UseBonemeal.validPos`.
    fn valid_pos(level: &World, pos: BlockPos) -> bool {
        let state = level.get_block_state(pos);
        is_crop(state) && !is_mature_crop(state)
    }

    /// Vanilla `UseBonemeal.setCurrentCropAsTarget`.
    fn set_current_crop_as_target(ctx: &mut BehaviorContext<'_>, pos: BlockPos) {
        utils::set_walk_and_look_target_to_pos(ctx, pos, SPEED_MODIFIER, 1);
    }

    /// Vanilla `BoneMealItem.growCrop` without the stack shrink.
    fn grow_crop(ctx: &BehaviorContext<'_>, pos: BlockPos) -> bool {
        let state = ctx.level.get_block_state(pos);
        let Some(behavior) = BLOCK_BEHAVIORS.get_behavior_for_state(state) else {
            return false;
        };
        let Some(bonemealable) = behavior.as_bonemealable() else {
            return false;
        };
        if !bonemealable.is_valid_bonemeal_target(state, ctx.level, pos) {
            return false;
        }

        let Some(world) = ctx.body.level() else {
            return false;
        };

        let mut rng = rand::rng();
        if bonemealable.is_bonemeal_success(state, &world, &mut rng, pos) {
            bonemealable.perform_bonemeal(state, &world, &mut rng, pos);
        }
        true
    }

    /// Vanilla `SimpleContainer.countItem(Items.BONE_MEAL)`.
    fn count_bone_meal(body: &SharedEntity) -> i32 {
        let Some(villager) = villager::as_villager(body) else {
            return 0;
        };

        villager.with_inventory_mut(|inventory| {
            (0..inventory.size())
                .map(|slot| inventory.get_item(slot))
                .filter(|stack| stack.is(&vanilla_items::BONE_MEAL))
                .map(ItemStack::count)
                .sum()
        })
    }
}

impl Default for UseBonemeal {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for UseBonemeal {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let tick_count = ctx.body.tick_count();
        let mut inner = self.inner.lock();

        if tick_count % PICK_INTERVAL != 0
            || (inner.last_bonemealing_session != 0
                && inner.last_bonemealing_session + SESSION_COOLDOWN > i64::from(tick_count))
        {
            return false;
        }

        if Self::count_bone_meal(ctx.body) <= 0 {
            return false;
        }

        inner.crop_pos = Self::pick_next_target(ctx.level, ctx.body);
        inner.crop_pos.is_some()
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        let inner = self.inner.lock();
        inner.time_worked_so_far < BONEMEALING_DURATION && inner.crop_pos.is_some()
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let mut inner = self.inner.lock();
        if let Some(pos) = inner.crop_pos {
            Self::set_current_crop_as_target(ctx, pos);
        }
        // Vanilla also equips a bone meal stack in the main hand; Steel has no
        // `setItemSlot` on the villager body.
        inner.next_work_cycle_time = ctx.timestamp;
        inner.time_worked_so_far = 0;
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        // Vanilla clears the main-hand bone meal here; not reachable in Steel.
        self.inner.lock().last_bonemealing_session = i64::from(ctx.body.tick_count());
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let mut inner = self.inner.lock();
        let Some(target) = inner.crop_pos else {
            return;
        };

        if ctx.timestamp < inner.next_work_cycle_time
            || !target.closer_to_center_than(ctx.body.position(), REACH_DISTANCE)
        {
            return;
        }

        let Some(villager) = villager::as_villager(ctx.body) else {
            return;
        };
        let slot = villager.with_inventory_mut(|inventory| {
            (0..inventory.size())
                .find(|slot| inventory.get_item(*slot).is(&vanilla_items::BONE_MEAL))
        });
        let Some(slot) = slot else {
            return;
        };

        if Self::grow_crop(ctx, target) {
            ctx.level.level_event(1505, target, 15, None);

            villager.with_inventory_mut(|inventory| {
                let mut stack = inventory.get_item(slot).clone();
                stack.shrink(1);
                if stack.is_empty() {
                    inventory.set_item(slot, ItemStack::empty());
                } else {
                    inventory.set_item(slot, stack);
                }
            });

            inner.crop_pos = Self::pick_next_target(ctx.level, ctx.body);
            if let Some(next) = inner.crop_pos {
                Self::set_current_crop_as_target(ctx, next);
            }
            inner.next_work_cycle_time = ctx.timestamp + WORK_CYCLE;
        }

        inner.time_worked_so_far += 1;
    }
}
