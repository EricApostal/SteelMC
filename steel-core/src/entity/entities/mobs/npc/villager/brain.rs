//! Villager brain construction and ticking.
//!
//! Vanilla lives in `Villager.BRAIN_PROVIDER` + `Villager.makeBrain`/
//! `refreshBrain`/`registerBrainGoals`; Steel keeps the same provider shape as free
//! functions over the entity so the brain itself stays entity-agnostic.
//!
//! ## Activity transitions
//!
//! Vanilla switches activities from inside the behavior tree
//! (`body.getBrain().setActiveActivityIfPossible(...)` in `VillagerPanicTrigger`,
//! `ReactToBell`, `SetRaidStatus`, `ResetRaidStatus`, `VillagerCalmDown`,
//! `SetHiddenState`, and `UpdateActivityFromSchedule`). Steel behaviors only receive
//! the memory store ([`crate::entity::ai::brain::behavior::BehaviorContext`]), never
//! the owning [`Brain`], so those switches are applied centrally in [`tick`] from the
//! same memories and the same conditions the vanilla behaviors test: identical
//! activity transitions, one call site instead of seven.
//!
//! The memory-visible halves of those behaviors (erasing `HURT_BY`, the golem-spawn
//! pulse, the `1/20` raid rolls) stay inside the behaviors.

use steel_registry::villager_profession::VillagerProfessionRef;

use crate::entity::ai::behavior::villager_goal_packages as packages;
use crate::entity::ai::brain::SensorType;
use crate::entity::ai::brain::activity::Activity;
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};
use crate::entity::ai::brain::schedule::Schedule;
use crate::entity::ai::brain::{ActivityData, Brain};
use crate::entity::ai::sensing::villager as villager_sensors;
use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::entity::{Entity as _, LivingEntity as _, SharedEntity};
use crate::world::World;

/// Vanilla `Villager.SPEED_MODIFIER`.
pub const SPEED_MODIFIER: f32 = 0.5;

/// Vanilla `Villager.BRAIN_PROVIDER` sensor order.
static SENSOR_TYPES: [&SensorType; 9] = [
    &villager_sensors::NEAREST_LIVING_ENTITIES,
    &villager_sensors::NEAREST_PLAYERS,
    &villager_sensors::NEAREST_ITEMS,
    &villager_sensors::NEAREST_BED,
    &villager_sensors::HURT_BY,
    &villager_sensors::VILLAGER_HOSTILES,
    &villager_sensors::VILLAGER_BABIES,
    &villager_sensors::SECONDARY_POIS,
    &villager_sensors::GOLEM_DETECTED,
];

/// Vanilla `Villager.makeBrain`: the provider plus the schedule goals.
///
/// Vanilla registers the union of the sensor `requires()` sets and the behavior
/// entry conditions; Steel passes every declared villager memory so behaviors that
/// only *write* a memory (e.g. `LAST_SLEPT`, `HEARD_BELL_TIME`) always have a slot.
#[must_use]
pub fn make_brain(villager: &VillagerEntity) -> Brain {
    let mut brain = Brain::new(
        mem::ALL,
        &SENSOR_TYPES,
        villager_activities(villager.profession(), villager.is_baby()),
    );
    register_brain_goals(&mut brain, villager.is_baby(), villager.level().as_deref());
    brain
}

/// Vanilla `Villager.refreshBrain`: rebuild the behavior packages for the current
/// villager data while keeping the sensed memories.
///
/// Vanilla replaces the brain with `BRAIN_PROVIDER.makeBrain(this, oldBrain.pack())`,
/// which preserves memories and re-registers fresh behaviors. Steel rebuilds the
/// registered behaviors in place, which preserves more than memories (expiry
/// countdowns, sensors) with identical observable behavior. Callers that need
/// vanilla's `stopAll` (which requires the entity) run it themselves before calling
/// this, from the tick that has the `SharedEntity`.
pub fn refresh_brain(brain: &mut Brain, villager: &VillagerEntity) {
    brain.remove_all_behaviors();
    for activity in villager_activities(villager.profession(), villager.is_baby()) {
        brain.add_activity(activity);
    }
    register_brain_goals(brain, villager.is_baby(), villager.level().as_deref());
}

/// Vanilla `Mob.customServerAiStep` brain tick for villagers.
pub fn tick(brain: &mut Brain, level: &World, body: &SharedEntity) {
    brain.tick(level, body);
    apply_activity_transitions(brain, level, body);
}

/// Vanilla `Villager.registerBrainGoals`: schedule choice plus an immediate
/// schedule sample so the first tick already runs the scheduled activity.
fn register_brain_goals(brain: &mut Brain, is_baby: bool, level: Option<&World>) {
    let schedule = if is_baby {
        Schedule::baby_villager()
    } else {
        Schedule::villager()
    };

    brain.set_schedule(schedule);

    if let Some(level) = level {
        let total_ticks = level
            .clock_total_ticks(schedule.timeline().clock)
            .unwrap_or_else(|| level.game_time());
        brain.set_activity_from_schedule(total_ticks);
    }
}

/// Vanilla `Villager.BRAIN_PROVIDER` activity list.
///
/// Babies only get the `PLAY` package in place of `WORK`; every villager gets
/// `CORE`, `MEET` (gated on the meeting point), `REST`, `IDLE`, `PANIC`,
/// `PRE_RAID`, `RAID` and `HIDE`.
#[must_use]
pub fn villager_activities(profession: VillagerProfessionRef, is_baby: bool) -> Vec<ActivityData> {
    let mut activities = Vec::with_capacity(10);

    if is_baby {
        activities.push(ActivityData::create(
            Activity::Play,
            packages::get_play_package(SPEED_MODIFIER),
        ));
    } else {
        activities.push(ActivityData::create_full(
            Activity::Work,
            packages::get_work_package(profession, SPEED_MODIFIER),
            vec![(&mem::JOB_SITE, MemoryStatus::ValuePresent)],
            Vec::new(),
        ));
    }

    activities.push(ActivityData::create(
        Activity::Core,
        packages::get_core_package(profession, SPEED_MODIFIER),
    ));
    activities.push(ActivityData::create_full(
        Activity::Meet,
        packages::get_meet_package(SPEED_MODIFIER),
        vec![(&mem::MEETING_POINT, MemoryStatus::ValuePresent)],
        Vec::new(),
    ));
    activities.push(ActivityData::create(
        Activity::Rest,
        packages::get_rest_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::create(
        Activity::Idle,
        packages::get_idle_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::create(
        Activity::Panic,
        packages::get_panic_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::create(
        Activity::PreRaid,
        packages::get_pre_raid_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::create(
        Activity::Raid,
        packages::get_raid_package(SPEED_MODIFIER),
    ));
    activities.push(ActivityData::create(
        Activity::Hide,
        packages::get_hide_package(SPEED_MODIFIER),
    ));

    activities
}

/// Steel adaptation of the activity switches vanilla performs from inside behaviors
/// (see the module docs): the schedule, raid status, panic and bell/hide.
///
/// Order matters. Vanilla's `UpdateActivityFromSchedule` runs at priority 99 and can
/// flip the activity back to the scheduled one, after which the priority-0
/// `VillagerPanicTrigger`/`ReactToBell` re-assert `PANIC`/`HIDE` on the next tick.
/// Applying the schedule first and the threats last yields the same steady state
/// without the one-tick flap.
fn apply_activity_transitions(brain: &mut Brain, level: &World, body: &SharedEntity) {
    // Vanilla `UpdateActivityFromSchedule` (priority 99 in every package), plus the
    // reschedule performed by `VillagerCalmDown` and `SetHiddenState`.
    brain.update_activity_from_schedule(level);

    let raid = level.get_raid_at(body.block_position());

    // Vanilla `SetRaidStatus`/`ResetRaidStatus`: both roll `1/20` per tick.
    if rand::random_range(0..20) == 0 {
        match raid {
            Some(raid) if raid.is_stopped() || raid.is_loss() => {
                brain.set_default_activity(Activity::Idle);
            }
            Some(raid) if raid.has_first_wave_spawned() && !raid.is_between_waves() => {
                brain.set_default_activity(Activity::Raid);
                brain.set_active_activity_if_possible(Activity::Raid);
            }
            Some(_) => {
                brain.set_default_activity(Activity::PreRaid);
                brain.set_active_activity_if_possible(Activity::PreRaid);
            }
            None => brain.set_default_activity(Activity::Idle),
        }
    }

    // Vanilla `VillagerPanicTrigger`: a remembered threat panics the villager.
    if brain.has_memory_value(&mem::HURT_BY) || brain.has_memory_value(&mem::NEAREST_HOSTILE) {
        brain.set_active_activity_if_possible(Activity::Panic);
    }

    // Vanilla `ReactToBell`: a heard bell hides the villager when no raid is running.
    // `SetHiddenState` clears `HEARD_BELL_TIME` when the hide ends.
    if raid.is_none() && brain.has_memory_value(&mem::HEARD_BELL_TIME) {
        brain.set_active_activity_if_possible(Activity::Hide);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_villager_professions};
    use steel_utils::{BlockPos, GlobalPos, Identifier};

    use super::*;
    use crate::entity::ai::brain::memory::MemoryModuleTypeRef;
    use crate::entity::entities::PigEntity;
    use crate::test_support::fresh_test_world;

    fn test_brain(profession: VillagerProfessionRef, is_baby: bool) -> Brain {
        init_vanilla_registry();
        Brain::new(mem::ALL, &[], villager_activities(profession, is_baby))
    }

    fn activity_of(activities: &[ActivityData], activity: Activity) -> &ActivityData {
        activities
            .iter()
            .find(|data| data.activity_type == activity)
            .unwrap_or_else(|| panic!("{activity:?} is not registered"))
    }

    fn has_condition(data: &ActivityData, ty: MemoryModuleTypeRef) -> bool {
        data.conditions
            .iter()
            .any(|(condition, _)| condition.key() == ty.key())
    }

    #[test]
    fn schedule_switches_activity_when_requirements_hold() {
        let profession: VillagerProfessionRef = &vanilla_villager_professions::NONE;
        let mut brain = test_brain(profession, false);
        brain.set_schedule(Schedule::villager());

        // WORK requires a job site, so a fresh villager falls back to the default.
        brain.set_activity_from_schedule(2000);
        assert_eq!(brain.get_active_non_core_activity(), Some(Activity::Idle));

        brain.set_memory(
            &mem::JOB_SITE,
            Some(GlobalPos::new(
                Identifier::vanilla_static("overworld"),
                BlockPos::new(4, 65, 4),
            )),
        );
        brain.set_activity_from_schedule(2000);
        assert!(brain.is_active(Activity::Work));
        assert!(brain.is_active(Activity::Core));

        // MEET requires the meeting point.
        brain.set_activity_from_schedule(9000);
        assert_eq!(brain.get_active_non_core_activity(), Some(Activity::Idle));
        brain.set_memory(
            &mem::MEETING_POINT,
            Some(GlobalPos::new(
                Identifier::vanilla_static("overworld"),
                BlockPos::new(0, 64, 0),
            )),
        );
        brain.set_activity_from_schedule(9000);
        assert!(brain.is_active(Activity::Meet));
        assert!(!brain.is_active(Activity::Work));

        // REST needs no memory and covers the night window.
        brain.set_activity_from_schedule(12000);
        assert!(brain.is_active(Activity::Rest));
        assert!(!brain.is_active(Activity::Meet));
    }

    #[test]
    fn baby_brain_registers_play_instead_of_work() {
        let profession: VillagerProfessionRef = &vanilla_villager_professions::NONE;
        let baby = villager_activities(profession, true);
        let adult = villager_activities(profession, false);

        // PLAY and WORK are mutually exclusive: a baby has no job site to work at.
        assert_eq!(
            activity_of(&baby, Activity::Play)
                .behavior_priority_pairs
                .len(),
            packages::get_play_package(SPEED_MODIFIER).len()
        );
        assert!(baby.iter().all(|data| data.activity_type != Activity::Work));
        assert!(
            adult
                .iter()
                .all(|data| data.activity_type != Activity::Play)
        );
        assert_eq!(
            activity_of(&adult, Activity::Work)
                .behavior_priority_pairs
                .len(),
            packages::get_work_package(profession, SPEED_MODIFIER).len()
        );

        // Every villager registers the always-available activities.
        for activity in [
            Activity::Core,
            Activity::Meet,
            Activity::Rest,
            Activity::Idle,
            Activity::Panic,
            Activity::PreRaid,
            Activity::Raid,
            Activity::Hide,
        ] {
            activity_of(&baby, activity);
            activity_of(&adult, activity);
        }

        // WORK/MEET are gated on their POI memories, the rest are unconditional.
        assert!(has_condition(
            activity_of(&adult, Activity::Work),
            &mem::JOB_SITE
        ));
        assert!(has_condition(
            activity_of(&adult, Activity::Meet),
            &mem::MEETING_POINT
        ));
        assert!(activity_of(&adult, Activity::Idle).conditions.is_empty());
    }

    #[test]
    fn remembered_threat_panics_and_bell_hides() {
        init_vanilla_registry();
        let world = fresh_test_world("villager_brain_transitions");
        let body: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));

        let profession: VillagerProfessionRef = &vanilla_villager_professions::NONE;
        let mut brain = test_brain(profession, false);
        brain.set_schedule(Schedule::villager());

        brain.set_memory(&mem::NEAREST_HOSTILE, Some(body.clone()));
        apply_activity_transitions(&mut brain, &world.world, &body);
        assert!(brain.is_active(Activity::Panic));

        // Clearing the threat lets the schedule take over again.
        brain.erase_memory(&mem::NEAREST_HOSTILE);
        apply_activity_transitions(&mut brain, &world.world, &body);
        assert!(!brain.is_active(Activity::Panic));

        brain.set_memory(&mem::HEARD_BELL_TIME, Some(42_i64));
        apply_activity_transitions(&mut brain, &world.world, &body);
        assert!(brain.is_active(Activity::Hide));
    }
}
