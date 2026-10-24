//! Vanilla `Brain` foundation: memory storage, sensors, activities and the
//! behavior tree, adapted to Steel's locking style.
//!
//! Vanilla `Brain<E>` owns everything for one entity and behaviors reach it via
//! `body.getBrain()`. Steel entities own their brain behind a lock, so a behavior
//! never re-enters it: [`Brain::tick`] splits its own state and hands behaviors a
//! [`BehaviorContext`] borrowing only the memory store. [`Provider`] builds the
//! brain for a mob and [`ActivityData`] registers its activity packages, exactly
//! like their vanilla namesakes.

pub mod activity;
pub mod behavior;
pub mod memory;
pub mod schedule;
pub mod sensor;
pub mod tracker;

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;

use activity::Activity;
use behavior::{BehaviorContext, BehaviorControl, BehaviorStatus};
use memory::{MemoryModuleType, MemoryModuleTypeRef, MemoryStatus, MemoryStore, MemoryValueType};
use schedule::Schedule;
pub use sensor::{SensorInstance, SensorType};

use crate::entity::SharedEntity;
use crate::world::World;

/// Vanilla `Brain.SCHEDULE_UPDATE_DELAY`.
const SCHEDULE_UPDATE_DELAY: i64 = 20;

/// Vanilla `Brain<E>`.
///
/// Stores one entity's memories, sensors, active activities and behavior tree.
/// Steel keeps it inside the owning entity (e.g. `SyncMutex<Brain>`), so all
/// mutation happens through `&mut self` on a single-threaded server tick.
pub struct Brain {
    memories: MemoryStore,
    sensors: Vec<SensorInstance>,
    behaviors_by_priority: BTreeMap<i32, FxHashMap<Activity, Vec<Box<dyn BehaviorControl>>>>,
    activity_requirements: FxHashMap<Activity, Vec<(MemoryModuleTypeRef, MemoryStatus)>>,
    activity_memories_to_erase_when_stopped: FxHashMap<Activity, Vec<MemoryModuleTypeRef>>,
    core_activities: Vec<Activity>,
    active_activities: Vec<Activity>,
    default_activity: Activity,
    schedule: Option<Schedule>,
    last_schedule_update: i64,
}

impl Default for Brain {
    fn default() -> Self {
        Self::new(&[], &[], Vec::new())
    }
}

impl Brain {
    /// Vanilla `Brain(memoryTypes, sensorTypes, activities, memories, random)`.
    ///
    /// `memories`/`random` are dropped: Steel has no packed-memory codec yet and
    /// seeds sensor jitter locally.
    #[must_use]
    pub fn new(
        memory_types: &[MemoryModuleTypeRef],
        sensor_types: &[&'static SensorType],
        activities: Vec<ActivityData>,
    ) -> Self {
        let mut brain = Self {
            memories: MemoryStore::new(),
            sensors: Vec::new(),
            behaviors_by_priority: BTreeMap::new(),
            activity_requirements: FxHashMap::default(),
            activity_memories_to_erase_when_stopped: FxHashMap::default(),
            core_activities: vec![Activity::Core],
            active_activities: vec![Activity::Core],
            default_activity: Activity::Idle,
            schedule: None,
            last_schedule_update: -9999,
        };

        for ty in memory_types {
            brain.register_memory(*ty);
        }

        for sensor_type in sensor_types {
            let mut sensor = SensorInstance::new(sensor_type);
            sensor.randomly_delay_start();
            for ty in sensor.requires() {
                brain.register_memory(*ty);
            }
            brain.sensors.push(sensor);
        }

        for activity in activities {
            brain.add_activity(activity);
        }

        brain.use_default_activity();
        brain
    }

    /// Vanilla `Brain.registerMemory`.
    pub fn register_memory(&mut self, ty: MemoryModuleTypeRef) {
        self.memories.register(ty);
    }

    #[must_use]
    pub const fn memories(&self) -> &MemoryStore {
        &self.memories
    }

    pub const fn memories_mut(&mut self) -> &mut MemoryStore {
        &mut self.memories
    }

    #[must_use]
    pub fn has_memory_value<T>(&self, ty: &MemoryModuleType<T>) -> bool {
        self.memories
            .check_key(ty.key(), MemoryStatus::ValuePresent)
    }

    /// Vanilla `Brain.clearMemories`.
    pub fn clear_memories(&mut self) {
        self.memories.clear();
    }

    pub fn erase_memory<T>(&mut self, ty: &MemoryModuleType<T>) {
        self.memories.erase(ty);
    }

    /// Vanilla `Brain.setMemory(type, Optional<U>)`: `None` erases.
    pub fn set_memory<T: MemoryValueType>(&mut self, ty: &MemoryModuleType<T>, value: Option<T>) {
        match value {
            Some(value) => self.memories.set(ty, value),
            None => self.memories.erase(ty),
        }
    }

    pub fn set_memory_with_expiry<T: MemoryValueType>(
        &mut self,
        ty: &MemoryModuleType<T>,
        value: T,
        time_to_live: i64,
    ) {
        self.memories.set_with_expiry(ty, value, time_to_live);
    }

    #[must_use]
    pub fn get_memory<T: MemoryValueType>(&self, ty: &MemoryModuleType<T>) -> Option<T> {
        self.memories.get(ty)
    }

    #[must_use]
    pub fn time_until_expiry<T>(&self, ty: &MemoryModuleType<T>) -> i64 {
        self.memories.time_to_live(ty)
    }

    #[must_use]
    pub fn is_memory_value<T: MemoryValueType + PartialEq>(
        &self,
        ty: &MemoryModuleType<T>,
        value: &T,
    ) -> bool {
        self.memories.is_value(ty, value)
    }

    #[must_use]
    pub fn check_memory(&self, ty: MemoryModuleTypeRef, status: MemoryStatus) -> bool {
        self.memories.check(ty, status)
    }

    /// Vanilla `Brain.forgetOutdatedMemories`: expires memory slots.
    pub fn forget_outdated_memories(&mut self) {
        self.memories.forget_outdated();
    }

    pub const fn set_schedule(&mut self, schedule: Schedule) {
        self.schedule = Some(schedule);
    }

    pub fn set_core_activities(&mut self, activities: Vec<Activity>) {
        self.core_activities = activities;
    }

    #[must_use]
    pub fn get_active_activities(&self) -> &[Activity] {
        &self.active_activities
    }

    /// Vanilla `Brain.useDefaultActivity`.
    pub fn use_default_activity(&mut self) {
        self.set_active_activity(self.default_activity);
    }

    #[must_use]
    pub fn get_active_non_core_activity(&self) -> Option<Activity> {
        self.active_activities
            .iter()
            .copied()
            .find(|activity| !self.core_activities.contains(activity))
    }

    /// Vanilla `Brain.setActiveActivityIfPossible`.
    pub fn set_active_activity_if_possible(&mut self, activity: Activity) {
        if self.activity_requirements_are_met(activity) {
            self.set_active_activity(activity);
        } else {
            self.use_default_activity();
        }
    }

    fn set_active_activity(&mut self, activity: Activity) {
        if self.is_active(activity) {
            return;
        }

        self.erase_memories_for_other_activities_than(activity);
        self.active_activities.clear();
        self.active_activities
            .extend_from_slice(&self.core_activities);
        if !self.active_activities.contains(&activity) {
            self.active_activities.push(activity);
        }
    }

    fn erase_memories_for_other_activities_than(&mut self, activity: Activity) {
        let old_activities = self.active_activities.clone();
        for old_activity in old_activities {
            if old_activity == activity {
                continue;
            }
            let Some(memory_types) = self
                .activity_memories_to_erase_when_stopped
                .get(&old_activity)
            else {
                continue;
            };
            let memory_types = memory_types.clone();
            for ty in memory_types {
                self.memories.erase_dyn(ty);
            }
        }
    }

    /// Vanilla `Brain.setActiveActivityToFirstValid`.
    pub fn set_active_activity_to_first_valid(&mut self, activities: &[Activity]) {
        for activity in activities {
            if self.activity_requirements_are_met(*activity) {
                self.set_active_activity(*activity);
                break;
            }
        }
    }

    pub const fn set_default_activity(&mut self, activity: Activity) {
        self.default_activity = activity;
    }

    /// Vanilla `Brain.updateActivityFromSchedule`: refreshes the scheduled
    /// activity every [`SCHEDULE_UPDATE_DELAY`] ticks.
    pub fn update_activity_from_schedule(&mut self, world: &World) {
        let game_time = world.game_time();
        if game_time - self.last_schedule_update <= SCHEDULE_UPDATE_DELAY {
            return;
        }

        self.last_schedule_update = game_time;
        let total_ticks = self
            .schedule
            .and_then(|schedule| world.clock_total_ticks(schedule.timeline().clock))
            .unwrap_or(game_time);
        self.set_activity_from_schedule(total_ticks);
    }

    /// Applies the schedule sampled at `total_ticks`.
    ///
    /// Vanilla reads this through `EnvironmentAttributeSystem.getValue`; Steel
    /// samples the villager timeline's activity track directly.
    pub fn set_activity_from_schedule(&mut self, total_ticks: i64) {
        let scheduled = self
            .schedule
            .map_or(Activity::Idle, |schedule| schedule.activity_at(total_ticks));
        if !self.active_activities.contains(&scheduled) {
            self.set_active_activity_if_possible(scheduled);
        }
    }

    /// Vanilla `Brain.addActivity`.
    pub fn add_activity(&mut self, data: ActivityData) {
        let activity = data.activity_type;
        self.activity_requirements.insert(activity, data.conditions);
        if !data.memories_to_erase_when_stopped.is_empty() {
            self.activity_memories_to_erase_when_stopped
                .insert(activity, data.memories_to_erase_when_stopped);
        }

        for (priority, behavior) in data.behavior_priority_pairs {
            for ty in behavior.required_memories() {
                self.register_memory(ty);
            }
            self.behaviors_by_priority
                .entry(priority)
                .or_default()
                .entry(activity)
                .or_default()
                .push(behavior);
        }
    }

    /// Vanilla `Brain.removeAllBehaviors`.
    pub fn remove_all_behaviors(&mut self) {
        self.behaviors_by_priority.clear();
    }

    #[must_use]
    pub fn is_active(&self, activity: Activity) -> bool {
        self.active_activities.contains(&activity)
    }

    #[must_use]
    pub fn get_running_behaviors(&self) -> Vec<&dyn BehaviorControl> {
        let mut running = Vec::new();
        for behaviors_for_activity in self.behaviors_by_priority.values() {
            for behaviors in behaviors_for_activity.values() {
                for behavior in behaviors {
                    if behavior.status() == BehaviorStatus::Running {
                        running.push(behavior.as_ref());
                    }
                }
            }
        }
        running
    }

    /// Vanilla `Brain.tick`.
    pub fn tick(&mut self, level: &World, body: &SharedEntity) {
        self.forget_outdated_memories();
        self.tick_sensors(level, body);
        self.start_each_non_running_behavior(level, body);
        self.tick_each_running_behavior(level, body);
    }

    /// Vanilla `Brain.stopAll`.
    pub fn stop_all(&mut self, level: &World, body: &SharedEntity) {
        let timestamp = level.game_time();
        let Self {
            memories,
            behaviors_by_priority,
            ..
        } = self;

        for behaviors_for_activity in behaviors_by_priority.values_mut() {
            for behaviors in behaviors_for_activity.values_mut() {
                for behavior in behaviors.iter_mut() {
                    if behavior.status() != BehaviorStatus::Running {
                        continue;
                    }
                    let mut ctx = BehaviorContext::new(&mut *memories, level, body, timestamp);
                    behavior.do_stop(&mut ctx);
                }
            }
        }
    }

    fn tick_sensors(&mut self, level: &World, body: &SharedEntity) {
        let Self {
            sensors, memories, ..
        } = self;
        sensor::tick_sensors(sensors, level, body, memories);
    }

    fn start_each_non_running_behavior(&mut self, level: &World, body: &SharedEntity) {
        let timestamp = level.game_time();
        let Self {
            memories,
            active_activities,
            behaviors_by_priority,
            ..
        } = self;

        for behaviors_for_activity in behaviors_by_priority.values_mut() {
            for (activity, behaviors) in behaviors_for_activity.iter_mut() {
                if !active_activities.contains(activity) {
                    continue;
                }
                for behavior in behaviors.iter_mut() {
                    if behavior.status() != BehaviorStatus::Stopped {
                        continue;
                    }
                    let mut ctx = BehaviorContext::new(&mut *memories, level, body, timestamp);
                    behavior.try_start(&mut ctx);
                }
            }
        }
    }

    fn tick_each_running_behavior(&mut self, level: &World, body: &SharedEntity) {
        let timestamp = level.game_time();
        let Self {
            memories,
            behaviors_by_priority,
            ..
        } = self;

        for behaviors_for_activity in behaviors_by_priority.values_mut() {
            for behaviors in behaviors_for_activity.values_mut() {
                for behavior in behaviors.iter_mut() {
                    if behavior.status() != BehaviorStatus::Running {
                        continue;
                    }
                    let mut ctx = BehaviorContext::new(&mut *memories, level, body, timestamp);
                    behavior.tick_or_stop(&mut ctx);
                }
            }
        }
    }

    #[must_use]
    fn activity_requirements_are_met(&self, activity: Activity) -> bool {
        let Some(requirements) = self.activity_requirements.get(&activity) else {
            return false;
        };

        requirements
            .iter()
            .all(|(ty, status)| self.memories.check(*ty, *status))
    }

    /// Vanilla `Brain.isBrainDead`.
    #[must_use]
    pub fn is_brain_dead(&self) -> bool {
        self.memories.is_empty() && self.sensors.is_empty() && self.behaviors_by_priority.is_empty()
    }
}

/// Vanilla `ActivityData<E>`.
pub struct ActivityData {
    pub activity_type: Activity,
    pub behavior_priority_pairs: Vec<(i32, Box<dyn BehaviorControl>)>,
    pub conditions: Vec<(MemoryModuleTypeRef, MemoryStatus)>,
    pub memories_to_erase_when_stopped: Vec<MemoryModuleTypeRef>,
}

impl ActivityData {
    /// Vanilla `ActivityData.create(activity, behaviorPriorityPairs)`.
    #[must_use]
    pub fn create(
        activity: Activity,
        behavior_priority_pairs: Vec<(i32, Box<dyn BehaviorControl>)>,
    ) -> Self {
        Self {
            activity_type: activity,
            behavior_priority_pairs,
            conditions: Vec::new(),
            memories_to_erase_when_stopped: Vec::new(),
        }
    }

    /// Vanilla `ActivityData.create(activity, priorityOfFirstBehavior, behaviorList)`.
    #[must_use]
    pub fn create_priority(
        activity: Activity,
        priority_of_first_behavior: i32,
        behavior_list: Vec<Box<dyn BehaviorControl>>,
    ) -> Self {
        Self::create(
            activity,
            Self::create_priority_pairs(priority_of_first_behavior, behavior_list),
        )
    }

    /// Vanilla `ActivityData.create(activity, priorityOfFirstBehavior, behaviorList,
    /// memoryThatMustHaveValueAndWillBeErasedAfter)`.
    #[must_use]
    pub fn create_with_erase(
        activity: Activity,
        priority_of_first_behavior: i32,
        behavior_list: Vec<Box<dyn BehaviorControl>>,
        memory_that_must_have_value_and_will_be_erased_after: MemoryModuleTypeRef,
    ) -> Self {
        Self {
            activity_type: activity,
            behavior_priority_pairs: Self::create_priority_pairs(
                priority_of_first_behavior,
                behavior_list,
            ),
            conditions: vec![(
                memory_that_must_have_value_and_will_be_erased_after,
                MemoryStatus::ValuePresent,
            )],
            memories_to_erase_when_stopped: vec![
                memory_that_must_have_value_and_will_be_erased_after,
            ],
        }
    }

    /// Vanilla `ActivityData.create(activity, behaviorPriorityPairs, conditions,
    /// memoriesToEraseWhenStopped)`.
    #[must_use]
    pub fn create_full(
        activity: Activity,
        behavior_priority_pairs: Vec<(i32, Box<dyn BehaviorControl>)>,
        conditions: Vec<(MemoryModuleTypeRef, MemoryStatus)>,
        memories_to_erase_when_stopped: Vec<MemoryModuleTypeRef>,
    ) -> Self {
        Self {
            activity_type: activity,
            behavior_priority_pairs,
            conditions,
            memories_to_erase_when_stopped,
        }
    }

    /// Vanilla `ActivityData.createPriorityPairs`.
    #[must_use]
    pub fn create_priority_pairs(
        priority_of_first_behavior: i32,
        behavior_list: Vec<Box<dyn BehaviorControl>>,
    ) -> Vec<(i32, Box<dyn BehaviorControl>)> {
        let mut next_priority = priority_of_first_behavior;
        behavior_list
            .into_iter()
            .map(|behavior| {
                let priority = next_priority;
                next_priority += 1;
                (priority, behavior)
            })
            .collect()
    }
}

/// Vanilla `Brain.ActivitySupplier<E>`.
pub type ActivitySupplier = Box<dyn Fn(&SharedEntity) -> Vec<ActivityData> + Send + Sync>;

/// Vanilla `Brain.Provider<E>`.
pub struct Provider {
    memory_types: Vec<MemoryModuleTypeRef>,
    sensor_types: Vec<&'static SensorType>,
    activities: ActivitySupplier,
}

impl Provider {
    #[must_use]
    pub fn new(sensor_types: Vec<&'static SensorType>, activities: ActivitySupplier) -> Self {
        Self {
            memory_types: Vec::new(),
            sensor_types,
            activities,
        }
    }

    #[must_use]
    pub fn with_memory_types(
        memory_types: Vec<MemoryModuleTypeRef>,
        sensor_types: Vec<&'static SensorType>,
        activities: ActivitySupplier,
    ) -> Self {
        Self {
            memory_types,
            sensor_types,
            activities,
        }
    }

    /// Vanilla `Brain.Provider.makeBrain`.
    #[must_use]
    pub fn make_brain(&self, body: &SharedEntity) -> Brain {
        Brain::new(
            &self.memory_types,
            &self.sensor_types,
            (self.activities)(body),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};
    use steel_registry::vanilla_damage_types;
    use steel_utils::locks::SyncMutex;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use steel_utils::BlockPos;

    use super::*;
    use crate::entity::damage::DamageSource;
    use crate::entity::entities::PigEntity;
    use crate::test_support::fresh_test_world;

    use super::behavior::gate::{GateBehavior, OrderPolicy, RunOne, RunningPolicy};
    use super::behavior::{Behavior, BehaviorState};
    use super::tracker::WalkTarget;
    use memory::module_type;
    use sensor::BrainSensor;

    fn erased<T: 'static>(ty: &'static MemoryModuleType<T>) -> MemoryModuleTypeRef {
        ty
    }

    #[test]
    fn memory_set_get_erase_with_expiry() {
        let mut brain = Brain::new(
            &[erased(&module_type::HOME), erased(&module_type::LAST_SLEPT)],
            &[],
            Vec::new(),
        );

        assert!(!brain.has_memory_value(&module_type::HOME));
        let dimension = steel_utils::Identifier::vanilla_static("overworld");
        let home = steel_utils::GlobalPos::new(dimension, BlockPos::new(1, 2, 3));
        brain.set_memory(&module_type::HOME, Some(home.clone()));
        assert!(brain.has_memory_value(&module_type::HOME));
        assert_eq!(brain.get_memory(&module_type::HOME), Some(home.clone()));

        brain.set_memory(&module_type::HOME, None);
        assert!(!brain.has_memory_value(&module_type::HOME));
        assert_eq!(brain.get_memory(&module_type::HOME), None);

        // Vanilla keys that are not registered keep the value absent.
        brain.set_memory(&module_type::JOB_SITE, Some(home));
        assert!(!brain.has_memory_value(&module_type::JOB_SITE));

        brain.set_memory_with_expiry(&module_type::LAST_SLEPT, 42_i64, 3);
        assert_eq!(brain.get_memory(&module_type::LAST_SLEPT), Some(42));
        assert_eq!(brain.time_until_expiry(&module_type::LAST_SLEPT), 3);

        for _ in 0..2 {
            brain.forget_outdated_memories();
        }
        assert_eq!(brain.time_until_expiry(&module_type::LAST_SLEPT), 1);
        assert!(brain.has_memory_value(&module_type::LAST_SLEPT));

        // Vanilla `MemorySlot.tick` clears only on the tick *after* the countdown
        // reaches zero (`hasExpired()` is `timeToLive <= 0`, checked before the
        // decrement), so a 3-tick expiry keeps the value through the third
        // `forgetOutdatedMemories` call.
        brain.forget_outdated_memories();
        assert_eq!(brain.time_until_expiry(&module_type::LAST_SLEPT), 0);
        assert!(brain.has_memory_value(&module_type::LAST_SLEPT));

        brain.forget_outdated_memories();
        assert!(!brain.has_memory_value(&module_type::LAST_SLEPT));

        // Unexpiring memories survive forgetting.
        brain.set_memory(
            &module_type::HOME,
            Some(steel_utils::GlobalPos::new(
                steel_utils::Identifier::vanilla_static("overworld"),
                BlockPos::new(1, 2, 3),
            )),
        );
        for _ in 0..100 {
            brain.forget_outdated_memories();
        }
        assert!(brain.has_memory_value(&module_type::HOME));
    }

    #[test]
    fn empty_collection_memory_is_stored_absent() {
        let mut brain = Brain::new(&[erased(&module_type::NEAREST_PLAYERS)], &[], Vec::new());

        brain.set_memory(&module_type::NEAREST_PLAYERS, Some(Vec::new()));
        assert!(!brain.has_memory_value(&module_type::NEAREST_PLAYERS));
    }

    #[test]
    fn activity_switches_by_schedule() {
        let mut brain = Brain::default();
        brain.set_schedule(Schedule::villager());

        for activity in [
            Activity::Idle,
            Activity::Work,
            Activity::Meet,
            Activity::Rest,
        ] {
            brain.add_activity(ActivityData::create(activity, Vec::new()));
        }

        // Vanilla villager_schedule keyframes: 10 idle, 2000 work, 9000 meet,
        // 11000 idle, 12000 rest, wrapping to rest before 10.
        assert_eq!(Schedule::villager().activity_at(0), Activity::Rest);
        assert_eq!(Schedule::villager().activity_at(10), Activity::Idle);
        assert_eq!(Schedule::villager().activity_at(2000), Activity::Work);
        assert_eq!(Schedule::villager().activity_at(8999), Activity::Work);
        assert_eq!(Schedule::villager().activity_at(9000), Activity::Meet);
        assert_eq!(Schedule::villager().activity_at(12000), Activity::Rest);

        brain.set_activity_from_schedule(2000);
        assert!(brain.is_active(Activity::Work));
        assert!(brain.is_active(Activity::Core));
        assert_eq!(brain.get_active_non_core_activity(), Some(Activity::Work));

        brain.set_activity_from_schedule(9000);
        assert!(brain.is_active(Activity::Meet));
        assert!(!brain.is_active(Activity::Work));

        // An activity without registered requirements falls back to the default.
        brain.set_active_activity_if_possible(Activity::Panic);
        assert_eq!(brain.get_active_non_core_activity(), Some(Activity::Idle));
    }

    #[test]
    fn activity_requirements_gate_switching() {
        // Vanilla `Brain.addActivity` registers only the memories the behaviors
        // require; activity conditions must already be registered by the provider or
        // a behavior, so the brain is built with `HURT_BY` registered like a real
        // provider's memory list does.
        let mut brain = Brain::new(&[erased(&module_type::HURT_BY)], &[], Vec::new());
        brain.add_activity(ActivityData::create_full(
            Activity::Panic,
            Vec::new(),
            vec![(erased(&module_type::HURT_BY), MemoryStatus::ValuePresent)],
            vec![],
        ));

        brain.set_active_activity_if_possible(Activity::Panic);
        assert_eq!(brain.get_active_non_core_activity(), Some(Activity::Idle));

        let source = DamageSource::environment(&vanilla_damage_types::GENERIC);
        brain.set_memory(&module_type::HURT_BY, Some(source));
        brain.set_active_activity_if_possible(Activity::Panic);
        assert_eq!(brain.get_active_non_core_activity(), Some(Activity::Panic));
    }

    struct RequiresSensor;

    impl BrainSensor for RequiresSensor {
        fn requires(&self) -> &'static [MemoryModuleTypeRef] {
            static REQUIRES: &[MemoryModuleTypeRef] = &[&module_type::NEAREST_HOSTILE];
            REQUIRES
        }

        fn sense(&mut self, _level: &World, _entity: &SharedEntity, _memories: &mut MemoryStore) {}
    }

    static REQUIRES_SENSOR_TYPE: SensorType =
        SensorType::new(|| Box::new(RequiresSensor), "requires_sensor");

    #[test]
    fn sensor_requires_registers_memories() {
        let brain = Brain::new(&[], &[&REQUIRES_SENSOR_TYPE], Vec::new());

        assert!(brain.check_memory(&module_type::NEAREST_HOSTILE, MemoryStatus::Registered));
        assert!(!brain.has_memory_value(&module_type::NEAREST_HOSTILE));
        assert!(brain.check_memory(&module_type::NEAREST_HOSTILE, MemoryStatus::ValueAbsent));
    }

    struct RecordingBehavior {
        state: BehaviorState,
        id: u32,
        log: Arc<SyncMutex<Vec<u32>>>,
    }

    impl RecordingBehavior {
        fn new(id: u32, log: &Arc<SyncMutex<Vec<u32>>>) -> Self {
            Self {
                state: BehaviorState::with_entry_condition(Vec::new()),
                id,
                log: Arc::clone(log),
            }
        }
    }

    impl Behavior for RecordingBehavior {
        fn state(&self) -> &BehaviorState {
            &self.state
        }

        fn state_mut(&mut self) -> &mut BehaviorState {
            &mut self.state
        }

        fn start(&mut self, _ctx: &mut BehaviorContext<'_>) {
            self.log.lock().push(self.id);
        }

        fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
            true
        }

        fn tick(&mut self, _ctx: &mut BehaviorContext<'_>) {
            self.log.lock().push(self.id);
        }
    }

    #[test]
    fn gate_behavior_ordered_run_one_starts_first_only() {
        init_vanilla_registry();
        let world = fresh_test_world("brain_gate_ordered");
        let body: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));

        let log = Arc::new(SyncMutex::new(Vec::new()));
        let mut gate = GateBehavior::new(
            Vec::new(),
            Vec::new(),
            OrderPolicy::Ordered,
            RunningPolicy::RunOne,
            vec![
                (Box::new(RecordingBehavior::new(1, &log)), 1),
                (Box::new(RecordingBehavior::new(2, &log)), 1),
            ],
        );

        let mut memories = MemoryStore::new();
        let mut ctx = BehaviorContext::new(&mut memories, &world.world, &body, 0);
        assert!(gate.try_start(&mut ctx));

        assert_eq!(&*log.lock(), &[1]);
        assert_eq!(gate.status(), BehaviorStatus::Running);
    }

    #[test]
    fn gate_behavior_try_all_runs_in_priority_order() {
        init_vanilla_registry();
        let world = fresh_test_world("brain_gate_try_all");
        let body: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));

        let log = Arc::new(SyncMutex::new(Vec::new()));
        let mut gate = GateBehavior::new(
            Vec::new(),
            Vec::new(),
            OrderPolicy::Ordered,
            RunningPolicy::TryAll,
            vec![
                (Box::new(RecordingBehavior::new(1, &log)), 1),
                (Box::new(RecordingBehavior::new(2, &log)), 1),
            ],
        );

        let mut memories = MemoryStore::new();
        let mut ctx = BehaviorContext::new(&mut memories, &world.world, &body, 0);
        assert!(gate.try_start(&mut ctx));

        assert_eq!(&*log.lock(), &[1, 2]);
    }

    #[test]
    fn run_one_starts_exactly_one_behavior() {
        init_vanilla_registry();
        let world = fresh_test_world("brain_run_one");
        let body: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));

        let log = Arc::new(SyncMutex::new(Vec::new()));
        let mut gate = RunOne::new(vec![
            (Box::new(RecordingBehavior::new(1, &log)), 1),
            (Box::new(RecordingBehavior::new(2, &log)), 1),
            (Box::new(RecordingBehavior::new(3, &log)), 1),
        ]);

        let mut memories = MemoryStore::new();
        let mut ctx = BehaviorContext::new(&mut memories, &world.world, &body, 0);
        assert!(gate.try_start(&mut ctx));

        assert_eq!(log.lock().len(), 1);
    }

    #[test]
    fn gate_behavior_erases_exit_memories_when_stopped() {
        init_vanilla_registry();
        let world = fresh_test_world("brain_gate_erase");
        let body: SharedEntity = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));

        let log = Arc::new(SyncMutex::new(Vec::new()));
        let mut gate = GateBehavior::new(
            Vec::new(),
            vec![erased(&module_type::WALK_TARGET)],
            OrderPolicy::Ordered,
            RunningPolicy::RunOne,
            vec![(Box::new(RecordingBehavior::new(1, &log)), 1)],
        );

        let mut memories = MemoryStore::new();
        memories.register(&module_type::WALK_TARGET);

        let mut ctx = BehaviorContext::new(&mut memories, &world.world, &body, 0);
        assert!(gate.try_start(&mut ctx));
        assert!(!ctx.has_memory_value(&module_type::WALK_TARGET));

        ctx.set_memory(
            &module_type::WALK_TARGET,
            WalkTarget::from_vec(DVec3::new(1.0, 2.0, 3.0), 1.0, 1),
        );
        assert!(ctx.has_memory_value(&module_type::WALK_TARGET));

        gate.do_stop(&mut ctx);
        assert!(!ctx.has_memory_value(&module_type::WALK_TARGET));
        assert_eq!(gate.status(), BehaviorStatus::Stopped);
    }
}
