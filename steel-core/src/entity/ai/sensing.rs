//! Per-mob sensing caches and the vanilla `net.minecraft.world.entity.ai.sensing`
//! sensor classes.
//!
//! One submodule per vanilla sensor class, each owning its `SensorType` static:
//! [`villager`] holds the shared sensors (nearest living/players/items, bed, hurt-by,
//! villager hostiles/babies, secondary POIs, golem) and the top-level modules hold
//! the brain-mob specific ones, in vanilla `SensorType` registration order. The
//! brain builder of each mob owns the order the sensors run in.

pub(crate) mod adult;
pub(crate) mod adult_any_type;
pub(crate) mod axolotl_attackables;
pub(crate) mod breeze_attack_entity;
pub(crate) mod dummy;
pub(crate) mod frog_attackables;
pub(crate) mod hoglin_specific;
pub(crate) mod is_in_water;
pub(crate) mod mob_sensor;
pub(crate) mod nearest_visible_living;
pub(crate) mod piglin_brute_specific;
pub(crate) mod piglin_specific;
pub(crate) mod tempting;
pub(crate) mod villager;
pub(crate) mod warden_entity;

use rustc_hash::FxHashSet;

/// Vanilla `Sensing` line-of-sight cache.
#[derive(Debug, Default)]
pub(crate) struct Sensing {
    seen: FxHashSet<i32>,
    unseen: FxHashSet<i32>,
}

impl Sensing {
    /// Creates an empty sensing cache.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Clears per-tick sensing results.
    pub(crate) fn tick(&mut self) {
        self.seen.clear();
        self.unseen.clear();
    }

    /// Returns cached line-of-sight for `target_id`, computing it on first use.
    pub(crate) fn has_line_of_sight(
        &mut self,
        target_id: i32,
        test: impl FnOnce() -> bool,
    ) -> bool {
        if self.seen.contains(&target_id) {
            return true;
        }
        if self.unseen.contains(&target_id) {
            return false;
        }

        let has_line_of_sight = test();
        if has_line_of_sight {
            self.seen.insert(target_id);
        } else {
            self.unseen.insert(target_id);
        }
        has_line_of_sight
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn sensing_caches_seen_targets_until_tick() {
        let calls = AtomicUsize::new(0);
        let mut sensing = Sensing::new();

        assert!(sensing.has_line_of_sight(1, || {
            calls.fetch_add(1, Ordering::Relaxed);
            true
        }));
        assert!(sensing.has_line_of_sight(1, || {
            calls.fetch_add(1, Ordering::Relaxed);
            false
        }));
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        sensing.tick();
        assert!(!sensing.has_line_of_sight(1, || {
            calls.fetch_add(1, Ordering::Relaxed);
            false
        }));
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn sensing_caches_unseen_targets_until_tick() {
        let calls = AtomicUsize::new(0);
        let mut sensing = Sensing::new();

        assert!(!sensing.has_line_of_sight(1, || {
            calls.fetch_add(1, Ordering::Relaxed);
            false
        }));
        assert!(!sensing.has_line_of_sight(1, || {
            calls.fetch_add(1, Ordering::Relaxed);
            true
        }));

        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
