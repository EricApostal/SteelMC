//! Vanilla `AllayAi`.
//!
//! `Allay.makeBrain` is a `Brain.Provider` over three liked-target memories, four
//! sensors and the two `AllayAi` activities; `Allay.customServerAiStep` ticks the
//! brain and then calls `AllayAi.updateActivity`. Steel keeps that split:
//! [`make_brain`] is the provider, [`tick`] the brain half and [`update_activity`]
//! the activity switch.
//!
//! ## Vanilla state with no Steel counterpart
//!
//! `AllayAi.initIdleActivity` registers
//! `GoAndGiveItemsToTarget(getItemDepositPosition, 2.25F, 20, throwItem,
//! ITEM_PICKUP_COOLDOWN_TICKS, 60, hasItemToThrow)`. `throwItem` and
//! `hasItemToThrow` read the allay's one-slot inventory (`Allay.getInventory()`),
//! which `AllayEntity` does not expose yet, so that entry is omitted rather than
//! stubbed; every other `AllayAi` entry is registered below.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_utils::BlockPos;
use steel_utils::types::GameType;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::SharedEntity;
use crate::entity::ai::behavior::animal_panic::AnimalPanic;
use crate::entity::ai::behavior::count_down_cooldown_ticks::CountDownCooldownTicks;
use crate::entity::ai::behavior::do_nothing::DoNothing;
use crate::entity::ai::behavior::go_to_wanted_item::GoToWantedItem;
use crate::entity::ai::behavior::look_at_target_sink::LookAtTargetSink;
use crate::entity::ai::behavior::move_to_target_sink::MoveToTargetSink;
use crate::entity::ai::behavior::random_stroll::RandomStroll;
use crate::entity::ai::behavior::set_entity_look_target_sometimes::SetEntityLookTargetSometimes;
use crate::entity::ai::behavior::set_walk_target_from_look_target::SetWalkTargetFromLookTarget;
use crate::entity::ai::behavior::stay_close_to_target::StayCloseToTarget;
use crate::entity::ai::behavior::swim::Swim;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::behavior::BehaviorContext;
use crate::entity::ai::brain::behavior::builder::BehaviorBuilder;
use crate::entity::ai::brain::behavior::gate::RunOne;
use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{BlockPosTracker, EntityTracker, PositionTracker};
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::entities::mobs::passive::allay::AllayEntity;
use crate::world::World;

/// Vanilla `AllayAi.SPEED_MULTIPLIER_WHEN_IDLING`.
const SPEED_MULTIPLIER_WHEN_IDLING: f32 = 1.0;
/// Vanilla `AllayAi.SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET`.
const SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET: f32 = 2.25;
/// Vanilla `AllayAi.SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM`.
const SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM: f32 = 1.75;
/// Vanilla `AllayAi.SPEED_MULTIPLIER_WHEN_PANICKING`.
const SPEED_MULTIPLIER_WHEN_PANICKING: f32 = 2.5;
/// Vanilla `AllayAi.CLOSE_ENOUGH_TO_TARGET`.
const CLOSE_ENOUGH_TO_TARGET: i32 = 4;
/// Vanilla `AllayAi.TOO_FAR_FROM_TARGET`.
const TOO_FAR_FROM_TARGET: i32 = 16;
/// Vanilla `AllayAi.MAX_LOOK_DISTANCE`.
const MAX_LOOK_DISTANCE: f32 = 6.0;
/// Vanilla `AllayAi.DISTANCE_TO_WANTED_ITEM`.
const DISTANCE_TO_WANTED_ITEM: i32 = 32;
/// Vanilla `AllayAi.NOTEBLOCK_DEPOSIT_RANGE_SQR`, the `1024` passed to
/// `GlobalPos.isCloseEnough`.
const NOTEBLOCK_DEPOSIT_RANGE_SQR: f64 = 1024.0;
/// Vanilla `AllayAi.LIKED_PLAYER_DEPOSIT_RANGE`, `closerThan(player, 64.0)`.
const LIKED_PLAYER_DEPOSIT_RANGE: f64 = 64.0;

/// Vanilla `SetEntityLookTargetSometimes.create(EntityTypes.PLAYER, 6.0F,
/// UniformInt.of(30, 60))` interval.
const LOOK_INTERVAL: UniformIntProvider = UniformIntProvider {
    min_inclusive: 30,
    max_inclusive: 60,
};

/// Vanilla `Allay.BRAIN_PROVIDER` memory list.
static MEMORY_TYPES: &[MemoryModuleTypeRef] = &[
    &mem::LIKED_PLAYER,
    &mem::LIKED_NOTEBLOCK_POSITION,
    &mem::LIKED_NOTEBLOCK_COOLDOWN_TICKS,
];

/// Vanilla `Allay.BRAIN_PROVIDER` sensor list.
static SENSOR_TYPES: [&SensorType; 4] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::NEAREST_PLAYERS,
    &villager_sensors::HURT_BY,
    &villager_sensors::NEAREST_ITEMS,
];

/// Vanilla `Allay.BRAIN_PROVIDER.makeBrain`.
///
/// The provider itself reads no entity state; the parameter keeps the call shape
/// the allay's own `make_brain` uses, like the villager brain.
#[must_use]
pub(crate) fn make_brain(_allay: &AllayEntity) -> Brain {
    Brain::new(MEMORY_TYPES, &SENSOR_TYPES, activities())
}

/// Vanilla `AllayAi.getActivities`.
#[must_use]
fn activities() -> Vec<ActivityData> {
    vec![core_activity(), idle_activity()]
}

/// Vanilla `AllayAi.initCoreActivity`.
fn core_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Core,
        0,
        vec![
            BehaviorBuilder::boxed(Swim::new(0.8)),
            BehaviorBuilder::boxed(AnimalPanic::new(SPEED_MULTIPLIER_WHEN_PANICKING)),
            BehaviorBuilder::boxed(LookAtTargetSink::new(45, 90)),
            BehaviorBuilder::boxed(MoveToTargetSink::new()),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(
                &mem::LIKED_NOTEBLOCK_COOLDOWN_TICKS,
            )),
            BehaviorBuilder::boxed(CountDownCooldownTicks::new(
                &mem::ITEM_PICKUP_COOLDOWN_TICKS,
            )),
        ],
    )
}

/// Vanilla `AllayAi.initIdleActivity`.
///
/// The `GoAndGiveItemsToTarget` entry is omitted; see the module docs.
fn idle_activity() -> ActivityData {
    ActivityData::create_priority(
        Activity::Idle,
        0,
        vec![
            BehaviorBuilder::boxed(GoToWantedItem::create(
                SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM,
                true,
                DISTANCE_TO_WANTED_ITEM,
            )),
            BehaviorBuilder::boxed(StayCloseToTarget::create(
                item_deposit_position,
                has_no_wanted_item,
                CLOSE_ENOUGH_TO_TARGET,
                TOO_FAR_FROM_TARGET,
                SPEED_MULTIPLIER_WHEN_FOLLOWING_DEPOSIT_TARGET,
            )),
            BehaviorBuilder::boxed(SetEntityLookTargetSometimes::for_type(
                &vanilla_entities::PLAYER,
                MAX_LOOK_DISTANCE,
                LOOK_INTERVAL,
            )),
            BehaviorBuilder::boxed(RunOne::new(vec![
                (
                    BehaviorBuilder::boxed(RandomStroll::fly(SPEED_MULTIPLIER_WHEN_IDLING)),
                    2,
                ),
                (
                    BehaviorBuilder::boxed(SetWalkTargetFromLookTarget::create(
                        SPEED_MULTIPLIER_WHEN_IDLING,
                        3,
                    )),
                    2,
                ),
                (BehaviorBuilder::boxed(DoNothing::new(30, 60)), 1),
            ])),
        ],
    )
}

/// Vanilla `Allay.customServerAiStep` brain half.
pub(crate) fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    update_activity(brain, body);
}

/// Vanilla `AllayAi.updateActivity`: outside `CORE` an allay only ever idles.
pub(crate) fn update_activity(brain: &mut Brain, _body: &SharedEntity) {
    brain.set_active_activity_to_first_valid(&[Activity::Idle]);
}

/// Vanilla `Predicate.not(AllayAi::hasWantedItem)`.
fn has_no_wanted_item(ctx: &BehaviorContext<'_>) -> bool {
    !ctx.has_memory_value(&mem::NEAREST_VISIBLE_WANTED_ITEM)
}

/// Vanilla `AllayAi.getItemDepositPosition`.
fn item_deposit_position(ctx: &mut BehaviorContext<'_>) -> Option<Arc<dyn PositionTracker>> {
    if let Some(noteblock) = ctx.get_memory(&mem::LIKED_NOTEBLOCK_POSITION) {
        if should_deposit_items_at_liked_noteblock(ctx, &noteblock) {
            let above = BlockPos::new(noteblock.pos.x(), noteblock.pos.y() + 1, noteblock.pos.z());
            return Some(Arc::new(BlockPosTracker::from_block_pos(above)));
        }

        // Vanilla drops a noteblock whose cooldown ran out (or that stopped being a
        // note block) instead of walking to it.
        ctx.erase_memory(&mem::LIKED_NOTEBLOCK_POSITION);
    }

    liked_player_position_tracker(ctx)
}

/// Vanilla `AllayAi.shouldDepositItemsAtLikedNoteblock`.
fn should_deposit_items_at_liked_noteblock(
    ctx: &BehaviorContext<'_>,
    liked_noteblock_pos: &steel_utils::GlobalPos,
) -> bool {
    let body_pos = ctx.body.block_position();
    let dx = f64::from(body_pos.x() - liked_noteblock_pos.pos.x());
    let dy = f64::from(body_pos.y() - liked_noteblock_pos.pos.y());
    let dz = f64::from(body_pos.z() - liked_noteblock_pos.pos.z());

    // Vanilla `GlobalPos.isCloseEnough(dimension, blockPosition, 1024)`.
    liked_noteblock_pos.dimension == ctx.level.key
        && dx * dx + dy * dy + dz * dz <= NOTEBLOCK_DEPOSIT_RANGE_SQR
        && ctx
            .level
            .get_block_state(liked_noteblock_pos.pos)
            .get_block()
            .key
            == vanilla_blocks::NOTE_BLOCK.key
        && ctx.has_memory_value(&mem::LIKED_NOTEBLOCK_COOLDOWN_TICKS)
}

/// Vanilla `AllayAi.getLikedPlayerPositionTracker`.
fn liked_player_position_tracker(ctx: &BehaviorContext<'_>) -> Option<Arc<dyn PositionTracker>> {
    let liked_player = ctx.get_memory(&mem::LIKED_PLAYER)?;
    let player = ctx.level.players.get_by_uuid(&liked_player)?;

    if !matches!(player.game_mode(), GameType::Survival | GameType::Creative) {
        return None;
    }

    let player: SharedEntity = player;
    if villager_sensors::closer_than(ctx.body, &player, LIKED_PLAYER_DEPOSIT_RANGE) {
        Some(Arc::new(EntityTracker::new(player, true)))
    } else {
        None
    }
}
