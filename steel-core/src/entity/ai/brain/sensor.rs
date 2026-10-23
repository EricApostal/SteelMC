//! Vanilla `Sensor<S>` / `SensorType` framework.
//!
//! Concrete villager and iron-golem sensors arrive in Wave 2; this module carries
//! the gating semantics from vanilla `Sensor.tick` (scan-rate countdown, first-scan
//! jitter, `requires()` auto-registering memories on brain construction) so sensors
//! stay behavioral drivers rather than plain query helpers.

use super::memory::{MemoryModuleTypeRef, MemoryStore};
use crate::entity::SharedEntity;
use crate::world::World;

/// Vanilla `Sensor.DEFAULT_SCAN_RATE`.
pub const DEFAULT_SCAN_RATE: i64 = 20;

/// Vanilla `Sensor<E>`: senses the world and writes memories, never starts behaviors.
pub trait BrainSensor: Send + Sync {
    /// Vanilla `requires()`: memories this sensor needs registered and may write.
    /// [`super::Brain`] registers them during construction.
    fn requires(&self) -> &'static [MemoryModuleTypeRef];

    /// Vanilla `doTick`: refreshes this sensor's memories for the current scan.
    fn sense(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore);

    /// Vanilla `Sensor(scanRate)`: the number of ticks between scans.
    fn scan_rate(&self) -> i64 {
        DEFAULT_SCAN_RATE
    }
}

/// Vanilla `SensorType<U>`: a lazy sensor factory, addressed by identity.
pub struct SensorType {
    create: fn() -> Box<dyn BrainSensor>,
    debug_name: &'static str,
}

impl SensorType {
    #[must_use]
    pub const fn new(create: fn() -> Box<dyn BrainSensor>, debug_name: &'static str) -> Self {
        Self { create, debug_name }
    }

    #[must_use]
    pub fn create(&self) -> Box<dyn BrainSensor> {
        (self.create)()
    }

    #[must_use]
    pub const fn debug_name(&self) -> &'static str {
        self.debug_name
    }
}

/// A live sensor with its vanilla `timeToTick` countdown.
pub struct SensorInstance {
    sensor: Box<dyn BrainSensor>,
    sensor_type: &'static SensorType,
    scan_rate: i64,
    time_to_tick: i64,
}

impl SensorInstance {
    #[must_use]
    pub fn new(sensor_type: &'static SensorType) -> Self {
        let sensor = sensor_type.create();
        let scan_rate = sensor.scan_rate().max(1);
        Self {
            sensor,
            sensor_type,
            scan_rate,
            time_to_tick: scan_rate,
        }
    }

    #[must_use]
    pub const fn sensor_type(&self) -> &'static SensorType {
        self.sensor_type
    }

    #[must_use]
    pub fn requires(&self) -> &'static [MemoryModuleTypeRef] {
        self.sensor.requires()
    }

    /// Vanilla `Sensor.randomlyDelayStart`: delays the first scan by a jitter in
    /// `[0, scan_rate)`. Vanilla seeds this from the entity's `RandomSource`; the
    /// value is never observable, so a random source is enough.
    pub fn randomly_delay_start(&mut self) {
        self.time_to_tick = rand::random_range(0..self.scan_rate);
    }

    /// Vanilla `Sensor.tick`: count down to the next scan. Returns whether `sense`
    /// ran this tick.
    fn tick(&mut self, level: &World, entity: &SharedEntity, memories: &mut MemoryStore) -> bool {
        self.time_to_tick -= 1;
        if self.time_to_tick > 0 {
            return false;
        }

        self.time_to_tick = self.scan_rate;
        self.sensor.sense(level, entity, memories);
        true
    }
}

pub(super) fn tick_sensors(
    sensors: &mut [SensorInstance],
    level: &World,
    entity: &SharedEntity,
    memories: &mut MemoryStore,
) {
    for sensor in sensors {
        sensor.tick(level, entity, memories);
    }
}
