//! The vanilla `MemoryModuleType` constants used by villagers, iron golems and
//! their shared sensors/behaviors.
//!
//! Vanilla registers these in `BuiltInRegistries.MEMORY_MODULE_TYPE`; Steel keeps
//! them as statics and exposes [`ALL`] plus [`by_key`] for registry-style lookup.

use std::sync::Arc;

use glam::DVec3;
use steel_utils::{BlockPos, GlobalPos, Identifier};
use uuid::Uuid;

use super::MemoryModuleType;
use super::MemoryModuleTypeRef;
use super::nearest_visible::NearestVisibleLivingEntities;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::tracker::PositionTracker;
use crate::entity::ai::path::Path;
use crate::entity::damage::DamageSource;

macro_rules! memory_module_type {
    ($(#[$meta:meta])* $name:ident : $ty:ty = $key:literal) => {
        $(#[$meta])*
        pub static $name: MemoryModuleType<$ty> =
            MemoryModuleType::new(Identifier::vanilla_static($key));
    };
}

memory_module_type!(DUMMY: () = "dummy");
memory_module_type!(HOME: GlobalPos = "home");
memory_module_type!(JOB_SITE: GlobalPos = "job_site");
memory_module_type!(POTENTIAL_JOB_SITE: GlobalPos = "potential_job_site");
memory_module_type!(MEETING_POINT: GlobalPos = "meeting_point");
memory_module_type!(SECONDARY_JOB_SITE: Vec<GlobalPos> = "secondary_job_site");
memory_module_type!(NEAREST_LIVING_ENTITIES: Vec<SharedEntity> = "mobs");
memory_module_type!(
    NEAREST_VISIBLE_LIVING_ENTITIES: NearestVisibleLivingEntities = "visible_mobs"
);
memory_module_type!(VISIBLE_VILLAGER_BABIES: Vec<SharedEntity> = "visible_villager_babies");
memory_module_type!(NEAREST_PLAYERS: Vec<SharedEntity> = "nearest_players");
memory_module_type!(NEAREST_VISIBLE_PLAYER: SharedEntity = "nearest_visible_player");
memory_module_type!(
    NEAREST_VISIBLE_ATTACKABLE_PLAYER: SharedEntity = "nearest_visible_targetable_player"
);
memory_module_type!(
    NEAREST_VISIBLE_ATTACKABLE_PLAYERS: Vec<SharedEntity> = "nearest_visible_targetable_players"
);
memory_module_type!(WALK_TARGET: super::super::tracker::WalkTarget = "walk_target");
memory_module_type!(LOOK_TARGET: Arc<dyn PositionTracker> = "look_target");
memory_module_type!(ATTACK_TARGET: SharedEntity = "attack_target");
memory_module_type!(ATTACK_COOLING_DOWN: bool = "attack_cooling_down");
memory_module_type!(INTERACTION_TARGET: SharedEntity = "interaction_target");
memory_module_type!(BREED_TARGET: SharedEntity = "breed_target");
memory_module_type!(RIDE_TARGET: SharedEntity = "ride_target");
memory_module_type!(PATH: Path = "path");
memory_module_type!(DOORS_TO_CLOSE: Vec<GlobalPos> = "doors_to_close");
memory_module_type!(NEAREST_BED: BlockPos = "nearest_bed");
memory_module_type!(HURT_BY: DamageSource = "hurt_by");
memory_module_type!(HURT_BY_ENTITY: SharedEntity = "hurt_by_entity");
memory_module_type!(AVOID_TARGET: SharedEntity = "avoid_target");
memory_module_type!(NEAREST_HOSTILE: SharedEntity = "nearest_hostile");
memory_module_type!(NEAREST_ATTACKABLE: SharedEntity = "nearest_attackable");
memory_module_type!(HIDING_PLACE: GlobalPos = "hiding_place");
memory_module_type!(HEARD_BELL_TIME: i64 = "heard_bell_time");
memory_module_type!(CANT_REACH_WALK_TARGET_SINCE: i64 = "cant_reach_walk_target_since");
memory_module_type!(GOLEM_DETECTED_RECENTLY: bool = "golem_detected_recently");
memory_module_type!(DANGER_DETECTED_RECENTLY: bool = "danger_detected_recently");
memory_module_type!(LAST_SLEPT: i64 = "last_slept");
memory_module_type!(LAST_WOKEN: i64 = "last_woken");
memory_module_type!(LAST_WORKED_AT_POI: i64 = "last_worked_at_poi");
memory_module_type!(NEAREST_VISIBLE_ADULT: SharedEntity = "nearest_visible_adult");
memory_module_type!(
    NEAREST_VISIBLE_WANTED_ITEM: SharedEntity = "nearest_visible_wanted_item"
);
memory_module_type!(NEAREST_VISIBLE_NEMESIS: SharedEntity = "nearest_visible_nemesis");
memory_module_type!(PLAY_DEAD_TICKS: i32 = "play_dead_ticks");
memory_module_type!(TEMPTING_PLAYER: SharedEntity = "tempting_player");
memory_module_type!(TEMPTATION_COOLDOWN_TICKS: i32 = "temptation_cooldown_ticks");
memory_module_type!(GAZE_COOLDOWN_TICKS: i32 = "gaze_cooldown_ticks");
memory_module_type!(LONG_JUMP_COOLDOWN_TICKS: i32 = "long_jump_cooling_down");
memory_module_type!(LONG_JUMP_MID_JUMP: bool = "long_jump_mid_jump");
memory_module_type!(RAM_COOLDOWN_TICKS: i32 = "ram_cooldown_ticks");
memory_module_type!(RAM_TARGET: DVec3 = "ram_target");
memory_module_type!(IS_IN_WATER: () = "is_in_water");
memory_module_type!(IS_PREGNANT: () = "is_pregnant");
memory_module_type!(IS_PANICKING: bool = "is_panicking");
memory_module_type!(ITEM_PICKUP_COOLDOWN_TICKS: i32 = "item_pickup_cooldown_ticks");
memory_module_type!(TRANSPORT_ITEMS_COOLDOWN_TICKS: i32 = "transport_items_cooldown_ticks");
memory_module_type!(CHARGE_COOLDOWN_TICKS: i32 = "charge_cooldown_ticks");
memory_module_type!(VISITED_BLOCK_POSITIONS: Vec<GlobalPos> = "visited_block_positions");
memory_module_type!(
    UNREACHABLE_TRANSPORT_BLOCK_POSITIONS: Vec<GlobalPos> = "unreachable_transport_block_positions"
);
memory_module_type!(ANGRY_AT: Uuid = "angry_at");
memory_module_type!(UNIVERSAL_ANGER: bool = "universal_anger");
memory_module_type!(ADMIRING_ITEM: bool = "admiring_item");
memory_module_type!(ADMIRING_DISABLED: bool = "admiring_disabled");
memory_module_type!(HUNTED_RECENTLY: bool = "hunted_recently");
memory_module_type!(CELEBRATE_LOCATION: BlockPos = "celebrate_location");
memory_module_type!(DANCING: bool = "dancing");
memory_module_type!(PACIFIED: bool = "pacified");
memory_module_type!(ATE_RECENTLY: bool = "ate_recently");
memory_module_type!(NEAREST_REPELLENT: BlockPos = "nearest_repellent");
memory_module_type!(LIKED_PLAYER: Uuid = "liked_player");

/// Every memory module type Steel declares, in declaration order.
pub static ALL: &[MemoryModuleTypeRef] = &[
    &DUMMY,
    &HOME,
    &JOB_SITE,
    &POTENTIAL_JOB_SITE,
    &MEETING_POINT,
    &SECONDARY_JOB_SITE,
    &NEAREST_LIVING_ENTITIES,
    &NEAREST_VISIBLE_LIVING_ENTITIES,
    &VISIBLE_VILLAGER_BABIES,
    &NEAREST_PLAYERS,
    &NEAREST_VISIBLE_PLAYER,
    &NEAREST_VISIBLE_ATTACKABLE_PLAYER,
    &NEAREST_VISIBLE_ATTACKABLE_PLAYERS,
    &WALK_TARGET,
    &LOOK_TARGET,
    &ATTACK_TARGET,
    &ATTACK_COOLING_DOWN,
    &INTERACTION_TARGET,
    &BREED_TARGET,
    &RIDE_TARGET,
    &PATH,
    &DOORS_TO_CLOSE,
    &NEAREST_BED,
    &HURT_BY,
    &HURT_BY_ENTITY,
    &AVOID_TARGET,
    &NEAREST_HOSTILE,
    &NEAREST_ATTACKABLE,
    &HIDING_PLACE,
    &HEARD_BELL_TIME,
    &CANT_REACH_WALK_TARGET_SINCE,
    &GOLEM_DETECTED_RECENTLY,
    &DANGER_DETECTED_RECENTLY,
    &LAST_SLEPT,
    &LAST_WOKEN,
    &LAST_WORKED_AT_POI,
    &NEAREST_VISIBLE_ADULT,
    &NEAREST_VISIBLE_WANTED_ITEM,
    &NEAREST_VISIBLE_NEMESIS,
    &PLAY_DEAD_TICKS,
    &TEMPTING_PLAYER,
    &TEMPTATION_COOLDOWN_TICKS,
    &GAZE_COOLDOWN_TICKS,
    &LONG_JUMP_COOLDOWN_TICKS,
    &LONG_JUMP_MID_JUMP,
    &RAM_COOLDOWN_TICKS,
    &RAM_TARGET,
    &IS_IN_WATER,
    &IS_PREGNANT,
    &IS_PANICKING,
    &ITEM_PICKUP_COOLDOWN_TICKS,
    &TRANSPORT_ITEMS_COOLDOWN_TICKS,
    &CHARGE_COOLDOWN_TICKS,
    &VISITED_BLOCK_POSITIONS,
    &UNREACHABLE_TRANSPORT_BLOCK_POSITIONS,
    &ANGRY_AT,
    &UNIVERSAL_ANGER,
    &ADMIRING_ITEM,
    &ADMIRING_DISABLED,
    &HUNTED_RECENTLY,
    &CELEBRATE_LOCATION,
    &DANCING,
    &PACIFIED,
    &ATE_RECENTLY,
    &NEAREST_REPELLENT,
    &LIKED_PLAYER,
];

/// Looks up a memory module type by its registry key.
#[must_use]
pub fn by_key(key: &Identifier) -> Option<MemoryModuleTypeRef> {
    ALL.iter().copied().find(|ty| ty.key() == key)
}
