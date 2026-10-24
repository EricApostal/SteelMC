//! Vanilla `HarvestFarmland`.

use std::sync::Arc;

use steel_registry::REGISTRY;
use steel_registry::TaggedRegistryExt as _;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, IntProperty};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{sound_events, vanilla_blocks, vanilla_game_events};
use steel_utils::BlockPos;
use steel_utils::locks::SyncMutex;
use steel_utils::types::UpdateFlags;

use super::utils;
use super::villager;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus};
use crate::entity::ai::brain::tracker::BlockPosTracker;
use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `HarvestFarmland.HARVEST_DURATION`.
const HARVEST_DURATION: i32 = 200;
/// Vanilla `HarvestFarmland.SPEED_MODIFIER`.
const SPEED_MODIFIER: f32 = 0.5;
/// Vanilla `HarvestFarmland` harvest reach, `closerToCenterThan(..., 1.0)`.
const REACH_DISTANCE: f64 = 1.0;
/// Vanilla `nextOkStartTime = timestamp + 40L` after the behavior stops.
const STOP_COOLDOWN: i64 = 40;
/// Vanilla `nextOkStartTime = timestamp + 20L` after retargeting.
const RETARGET_COOLDOWN: i64 = 20;

/// Vanilla `HarvestFarmland`: a farmer harvests mature crops and replants seeds on
/// nearby farmland, walking to each target in turn.
pub struct HarvestFarmland {
    state: BehaviorState,
    // Vanilla mutates these in `checkExtraStartConditions`, which Steel hands out
    // as `&self`, so the fields sit behind a lock.
    inner: SyncMutex<HarvestFarmlandState>,
}

/// The mutable half of `HarvestFarmland`.
struct HarvestFarmlandState {
    above_farmland_pos: Option<BlockPos>,
    next_ok_start_time: i64,
    time_worked_so_far: i32,
    valid_farmland_around_villager: Vec<BlockPos>,
}

impl HarvestFarmland {
    /// Vanilla `HarvestFarmland()`.
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
                (
                    &mem::SECONDARY_JOB_SITE as MemoryModuleTypeRef,
                    MemoryStatus::ValuePresent,
                ),
            ]),
            inner: SyncMutex::new(HarvestFarmlandState {
                above_farmland_pos: None,
                next_ok_start_time: 0,
                time_worked_so_far: 0,
                valid_farmland_around_villager: Vec::new(),
            }),
        }
    }

    /// Vanilla `HarvestFarmland.getValidFarmland`.
    fn get_valid_farmland(valid: &[BlockPos]) -> Option<BlockPos> {
        if valid.is_empty() {
            None
        } else {
            Some(valid[rand::random_range(0..valid.len())])
        }
    }

    /// Vanilla `HarvestFarmland.validPos`.
    fn valid_pos(level: &World, pos: BlockPos) -> bool {
        let state = level.get_block_state(pos);
        let below = level.get_block_state(pos.below()).get_block();
        is_mature_crop(state) || (state.is_air() && below.key == vanilla_blocks::FARMLAND.key)
    }

    /// Vanilla `HarvestFarmland`'s `blockState.isAir() && blockBelow instanceof FarmlandBlock`
    /// branch: place the first seed the villager carries.
    fn place_seed(ctx: &mut BehaviorContext<'_>, pos: BlockPos) -> bool {
        let Some(villager) = villager::villager(ctx.level, ctx.body) else {
            return false;
        };

        let Some((slot, block)) = villager.with_inventory_mut(|inventory| {
            (0..inventory.size()).find_map(|slot| {
                let stack = inventory.get_item(slot);
                if stack.is_empty()
                    || !REGISTRY
                        .items
                        .is_in_tag(stack.item(), &ItemTag::VILLAGER_PLANTABLE_SEEDS)
                {
                    return None;
                }
                stack.item().block.map(|block| (slot, block))
            })
        }) else {
            return false;
        };

        let Some(world) = ctx.body.level() else {
            return false;
        };

        let placed = block.default_state();
        world.set_block(pos, placed, UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::BLOCK_PLACE,
            pos,
            &GameEventContext::new(Some(ctx.body.as_ref()), Some(placed)),
        );
        world.play_block_sound(&sound_events::ITEM_CROP_PLANT, pos, 1.0, 1.0, None);

        villager.with_inventory_mut(|inventory| {
            let mut stack = inventory.get_item(slot).clone();
            stack.shrink(1);
            if stack.is_empty() {
                inventory.set_item(slot, ItemStack::empty());
            } else {
                inventory.set_item(slot, stack);
            }
        });
        true
    }
}

impl Default for HarvestFarmland {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for HarvestFarmland {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.level.get_game_rule(&MOB_GRIEFING) {
            return false;
        }

        if villager::profession_of(ctx.body)
            .is_none_or(|profession| profession.key.path != "farmer")
        {
            return false;
        }

        let mut inner = self.inner.lock();
        inner.valid_farmland_around_villager.clear();

        let base = ctx.body.block_position();
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let pos = BlockPos::new(base.x() + x, base.y() + y, base.z() + z);
                    if Self::valid_pos(ctx.level, pos) {
                        inner.valid_farmland_around_villager.push(pos);
                    }
                }
            }
        }

        inner.above_farmland_pos = Self::get_valid_farmland(&inner.valid_farmland_around_villager);
        inner.above_farmland_pos.is_some()
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let inner = self.inner.lock();
        if ctx.timestamp <= inner.next_ok_start_time {
            return;
        }
        let Some(pos) = inner.above_farmland_pos else {
            return;
        };

        utils::set_look_target(ctx, Arc::new(BlockPosTracker::from_block_pos(pos)));
        utils::set_walk_target(
            ctx,
            Arc::new(BlockPosTracker::from_block_pos(pos)),
            SPEED_MODIFIER,
            1,
        );
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        utils::clear_look_target(ctx);
        utils::clear_walk_target(ctx);

        let mut inner = self.inner.lock();
        inner.time_worked_so_far = 0;
        inner.next_ok_start_time = ctx.timestamp + STOP_COOLDOWN;
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let mut inner = self.inner.lock();
        let Some(target) = inner.above_farmland_pos else {
            inner.time_worked_so_far += 1;
            return;
        };

        if !target.closer_to_center_than(ctx.body.position(), REACH_DISTANCE) {
            inner.time_worked_so_far += 1;
            return;
        }

        if ctx.timestamp > inner.next_ok_start_time {
            let state = ctx.level.get_block_state(target);
            let below = ctx.level.get_block_state(target.below()).get_block();

            if is_mature_crop(state)
                && let Some(world) = ctx.body.level()
            {
                world.destroy_block_by_entity(target, true, ctx.body.as_ref());
            }

            if state.is_air()
                && below.key == vanilla_blocks::FARMLAND.key
                && villager::villager(ctx.level, ctx.body)
                    .is_some_and(VillagerEntity::has_farm_seeds)
            {
                Self::place_seed(ctx, target);
            }

            if is_crop(state) && !is_mature_crop(state) {
                inner
                    .valid_farmland_around_villager
                    .retain(|pos| *pos != target);
                let next = Self::get_valid_farmland(&inner.valid_farmland_around_villager);
                inner.above_farmland_pos = next;
                if let Some(next) = next {
                    inner.next_ok_start_time = ctx.timestamp + RETARGET_COOLDOWN;
                    utils::set_walk_target(
                        ctx,
                        Arc::new(BlockPosTracker::from_block_pos(next)),
                        SPEED_MODIFIER,
                        1,
                    );
                    utils::set_look_target(ctx, Arc::new(BlockPosTracker::from_block_pos(next)));
                }
            }
        }

        inner.time_worked_so_far += 1;
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        self.inner.lock().time_worked_so_far < HARVEST_DURATION
    }
}

/// Whether `state` belongs to vanilla's crop block hierarchy.
///
/// Steel has no `CropBlock` capability accessor on `BlockBehavior`, so the
/// `#minecraft:crops` tag stands in for `block instanceof CropBlock`.
pub(super) fn is_crop(state: steel_utils::BlockStateId) -> bool {
    state.get_block().has_tag(&BlockTag::CROPS)
}

/// Vanilla `CropBlock.isMaxAge`.
///
/// The tag cannot distinguish `StemBlock` from `CropBlock`; an age property at its
/// maximum approximates both.
pub(super) fn is_mature_crop(state: steel_utils::BlockStateId) -> bool {
    const AGE_PROPERTIES: [&IntProperty; 4] = [
        &BlockStateProperties::AGE_7,
        &BlockStateProperties::AGE_4,
        &BlockStateProperties::AGE_3,
        &BlockStateProperties::AGE_2,
    ];

    if !is_crop(state) {
        return false;
    }

    AGE_PROPERTIES.iter().any(|property| {
        state
            .try_get_value(*property)
            .is_some_and(|age| age >= property.max)
    })
}
