//! Vanilla villager schedule.
//!
//! In 26.3 the villager schedule is no longer a `Schedule`/`ScheduleBuilder`
//! object: it is the `minecraft:villager_schedule` timeline whose
//! `minecraft:gameplay/villager_activity` (and `…/baby_villager_activity`) track
//! holds `Activity` keyframes. Steel reuses `steel_registry::vanilla_timelines`
//! directly instead of inventing a second schedule representation.

use steel_registry::timeline::{KeyframeValue, Timeline};
use steel_registry::vanilla_timelines::VILLAGER_SCHEDULE;

use super::activity::Activity;

/// Vanilla `EnvironmentAttributes.VILLAGER_ACTIVITY` track name.
pub const VILLAGER_ACTIVITY_TRACK: &str = "minecraft:gameplay/villager_activity";
/// Vanilla `EnvironmentAttributes.BABY_VILLAGER_ACTIVITY` track name.
pub const BABY_VILLAGER_ACTIVITY_TRACK: &str = "minecraft:gameplay/baby_villager_activity";

/// A timeline-backed activity schedule.
#[derive(Debug, Clone, Copy)]
pub struct Schedule {
    timeline: &'static Timeline,
    track: &'static str,
}

impl Schedule {
    #[must_use]
    pub const fn new(timeline: &'static Timeline, track: &'static str) -> Self {
        Self { timeline, track }
    }

    /// Vanilla overworld villager schedule.
    #[must_use]
    pub const fn villager() -> Self {
        Self::new(&VILLAGER_SCHEDULE, VILLAGER_ACTIVITY_TRACK)
    }

    /// Vanilla overworld baby villager schedule.
    #[must_use]
    pub const fn baby_villager() -> Self {
        Self::new(&VILLAGER_SCHEDULE, BABY_VILLAGER_ACTIVITY_TRACK)
    }

    #[must_use]
    pub const fn timeline(&self) -> &'static Timeline {
        self.timeline
    }

    #[must_use]
    pub const fn track(&self) -> &'static str {
        self.track
    }

    /// Samples the scheduled activity for a total clock tick count.
    ///
    /// Vanilla's activity attribute type is not interpolated, so the value is the
    /// last keyframe at or before the sampled tick, wrapping to the final keyframe.
    #[must_use]
    pub fn activity_at(&self, total_ticks: i64) -> Activity {
        let Some(track) = self
            .timeline
            .tracks
            .iter()
            .find(|track| track.name == self.track)
        else {
            return Activity::Idle;
        };

        let ticks = match self.timeline.period_ticks {
            Some(period) => total_ticks.rem_euclid(i64::from(period)),
            None => total_ticks,
        };

        let mut activity = track
            .keyframes
            .last()
            .and_then(|keyframe| activity_of(&keyframe.value));

        for keyframe in track.keyframes {
            if keyframe.ticks > ticks {
                break;
            }
            if let Some(scheduled) = activity_of(&keyframe.value) {
                activity = Some(scheduled);
            }
        }

        activity.unwrap_or(Activity::Idle)
    }
}

fn activity_of(value: &KeyframeValue) -> Option<Activity> {
    match value {
        KeyframeValue::String(name) => Activity::from_name(name),
        _ => None,
    }
}
