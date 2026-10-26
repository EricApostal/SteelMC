//! Map id allocation.
//!
//! Mirrors vanilla `MapIndex`: a monotonically increasing id stored in saved
//! data so every map item gets a unique `MapId`.

use serde::{Deserialize, Serialize};

/// Vanilla `MapIndex`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapIndex {
    #[serde(default = "default_last_map_id")]
    last_map_id: i32,
}

const fn default_last_map_id() -> i32 {
    -1
}

impl MapIndex {
    /// Creates an index that hands out `0` first, like vanilla's `-1` initial value.
    #[must_use]
    pub const fn new() -> Self {
        Self { last_map_id: -1 }
    }

    /// Vanilla `MapIndex.getNextMapId`.
    pub const fn next_map_id(&mut self) -> i32 {
        self.last_map_id += 1;
        self.last_map_id
    }

    /// Returns the most recently handed-out id.
    #[must_use]
    pub const fn last_map_id(self) -> i32 {
        self.last_map_id
    }

    /// Raises the index so it never hands out an already used id.
    pub fn observe(&mut self, map_id: i32) {
        self.last_map_id = self.last_map_id.max(map_id);
    }
}

impl Default for MapIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::MapIndex;

    #[test]
    fn ids_start_at_zero_and_increase() {
        let mut index = MapIndex::new();
        assert_eq!(index.last_map_id(), -1);
        assert_eq!(index.next_map_id(), 0);
        assert_eq!(index.next_map_id(), 1);
    }

    #[test]
    fn observing_existing_ids_prevents_reuse() {
        let mut index = MapIndex::new();
        index.observe(7);
        assert_eq!(index.next_map_id(), 8);
    }
}
