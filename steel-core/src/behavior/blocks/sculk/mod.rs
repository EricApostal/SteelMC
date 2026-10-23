//! Vanilla sculk block behaviors.
//!
//! Ports `SculkSensorBlock`, `CalibratedSculkSensorBlock`, `SculkShriekerBlock`,
//! `SculkCatalystBlock` and `SculkBlock`. Vibration arrival is driven by the block entities'
//! [`VibrationSystemListener`](crate::world::game_event::vibration::VibrationSystemListener)
//! callbacks; the blocks below own state transitions, redstone output and XP drops.
pub mod calibrated_sculk_sensor_block;
pub mod sculk_block;
pub mod sculk_catalyst_block;
pub mod sculk_sensor_block;
pub mod sculk_shrieker_block;

pub use calibrated_sculk_sensor_block::CalibratedSculkSensorBlock;
pub use sculk_block::SculkBlock;
pub use sculk_catalyst_block::{SCULK_CATALYST_PULSE_TICKS, SculkCatalystBlock};
pub use sculk_sensor_block::{
    SCULK_SENSOR_ACTIVE_TICKS, SCULK_SENSOR_COOLDOWN_TICKS, SculkSensorBlock,
};
pub use sculk_shrieker_block::{SCULK_SHRIEKING_TICKS, SculkShriekerBlock};
