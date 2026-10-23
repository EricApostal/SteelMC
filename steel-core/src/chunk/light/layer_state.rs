//! Vanilla `LayerLightSectionStorage` bookkeeping.
//!
//! Vanilla keeps light *data* and light *section state* in one per-layer storage
//! object (`LayerLightSectionStorage`) that owns a global section map. Steel keeps
//! the light data itself in the owning chunk ([`super::ChunkLightData`]) — the
//! divergence the chunk-pipeline audit already records — so this module ports only
//! the state half: section state bytes with their 26-neighbour counts, the
//! light-enabled columns, retained columns, queued section layers, the removal
//! queue, and the inconsistency flag that decides whether another light pass is
//! owed. Light *data* publishing stays with the workset's commit path.

use rustc_hash::{FxHashMap, FxHashSet};
use steel_utils::{ChunkPos, SectionPos};

use super::{DataLayer, LightLayer};

/// Vanilla `LayerLightSectionStorage.SectionState.EMPTY`.
const SECTION_STATE_EMPTY: u8 = 0;
/// Vanilla `SectionState.MIN_NEIGHBORS`.
const MIN_NEIGHBORS: i32 = 0;
/// Vanilla `SectionState.MAX_NEIGHBORS`.
const MAX_NEIGHBORS: i32 = 26;
/// Vanilla `SectionState.HAS_DATA_BIT`.
const HAS_DATA_BIT: u8 = 32;
/// Vanilla `SectionState.NEIGHBOR_COUNT_BITS`.
const NEIGHBOR_COUNT_BITS: u8 = 31;

/// Vanilla `LayerLightSectionStorage.SectionState` packed into one byte.
///
/// Bit 5 marks that light data is stored for the section; bits 0..=4 hold the
/// number of non-empty neighbors (0..=26).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SectionState(u8);

impl SectionState {
    /// Returns this state's raw byte.
    #[must_use]
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// Creates a state from its raw byte.
    #[must_use]
    pub const fn from_raw(raw: u8) -> Self {
        Self(raw)
    }

    /// Returns the vanilla `SectionState.hasData(state, hasData)` result.
    #[must_use]
    pub const fn with_has_data(self) -> Self {
        Self(self.0 | HAS_DATA_BIT)
    }

    /// Returns the vanilla `SectionState.hasData(state, false)` result.
    #[must_use]
    pub const fn without_has_data(self) -> Self {
        Self(self.0 & !HAS_DATA_BIT)
    }

    /// Returns the vanilla `SectionState.neighborCount(state, count)` result.
    ///
    /// # Panics
    /// Panics when `count` is outside vanilla's `0..=26` neighbour range, matching
    /// vanilla's `IllegalArgumentException`.
    #[must_use]
    pub const fn with_neighbor_count(self, count: i32) -> Self {
        assert!(
            count >= MIN_NEIGHBORS && count <= MAX_NEIGHBORS,
            "neighbor count was not within range [0; 26]"
        );
        Self((self.0 & !NEIGHBOR_COUNT_BITS) | (count as u8 & NEIGHBOR_COUNT_BITS))
    }

    /// Returns the vanilla `SectionState.hasData(state)` result.
    #[must_use]
    pub const fn has_data(self) -> bool {
        self.0 & HAS_DATA_BIT != 0
    }

    /// Returns the vanilla `SectionState.neighborCount(state)` result.
    #[must_use]
    pub const fn neighbor_count(self) -> i32 {
        (self.0 & NEIGHBOR_COUNT_BITS) as i32
    }

    /// Returns the vanilla `SectionState.type(state)` result.
    #[must_use]
    pub const fn section_type(self) -> SectionType {
        if self.0 == SECTION_STATE_EMPTY {
            SectionType::Empty
        } else if self.has_data() {
            SectionType::LightAndData
        } else {
            SectionType::LightOnly
        }
    }
}

/// Vanilla `LayerLightSectionStorage.SectionType`, used by light debug data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SectionType {
    /// No light section state is stored.
    Empty,
    /// Light is stored without block data.
    LightOnly,
    /// Light and block data are both stored.
    LightAndData,
}

impl SectionType {
    /// Returns the vanilla debug display string.
    #[must_use]
    pub const fn display(self) -> &'static str {
        match self {
            Self::Empty => "2",
            Self::LightOnly => "1",
            Self::LightAndData => "0",
        }
    }
}

/// Vanilla `LayerLightSectionStorage` state for one light layer.
///
/// Keys are vanilla section nodes (`SectionPos.asLong`) and zero nodes
/// (`SectionPos.getZeroNode`, vanilla `ChunkPos.asLong`).
#[derive(Debug, Default)]
pub struct LayerLightState {
    section_states: FxHashMap<i64, SectionState>,
    columns_with_sources: FxHashSet<i64>,
    columns_to_retain_queued_data_for: FxHashSet<i64>,
    queued_sections: FxHashMap<i64, DataLayer>,
    to_remove: FxHashSet<i64>,
    /// Sections whose stored light changed since the last map swap.
    pub(crate) changed_sections: FxHashSet<i64>,
    /// Sections that must be reported through `onLightUpdate` on the next swap.
    pub(crate) sections_affected_by_light_updates: FxHashSet<i64>,
    has_inconsistencies: bool,
}

impl LayerLightState {
    /// Creates empty layer state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the vanilla `SectionPos.getZeroNode(chunkX, chunkZ)`.
    ///
    /// Vanilla defines it as `getZeroNode(SectionPos.asLong(x, 0, z))`, i.e. the
    /// section-node layout with the section Y bits cleared — not a chunk-position
    /// packed long.
    #[must_use]
    pub fn zero_node(chunk_pos: ChunkPos) -> i64 {
        Self::zero_node_of(Self::section_node(SectionPos::new(
            chunk_pos.0.x,
            0,
            chunk_pos.0.y,
        )))
    }

    /// Returns the vanilla `SectionPos.asLong` of a section position.
    #[must_use]
    pub fn section_node(section_pos: SectionPos) -> i64 {
        // `PackedSectionPos` uses vanilla's `SectionPos.asLong` bit layout.
        steel_utils::PackedSectionPos::from(section_pos).as_raw()
    }

    /// Returns the vanilla `SectionPos.getZeroNode(sectionNode)`.
    #[must_use]
    pub const fn zero_node_of(section_node: i64) -> i64 {
        // Vanilla drops the section Y bits: sections of one column share a zero node.
        section_node & !0xF_FFFF
    }

    /// Returns the vanilla `SectionPos.of(sectionNode)`.
    #[must_use]
    pub fn section_from_node(section_node: i64) -> SectionPos {
        steel_utils::PackedSectionPos::from_raw(section_node).to_section_pos()
    }

    /// Returns vanilla `SectionPos.offset(x, y, z)`.
    #[must_use]
    const fn offset_section(section: SectionPos, dx: i32, dy: i32, dz: i32) -> SectionPos {
        if dx == 0 && dy == 0 && dz == 0 {
            return section;
        }
        SectionPos::new(section.x() + dx, section.y() + dy, section.z() + dz)
    }

    /// Vanilla `LayerLightSectionStorage.setLightEnabled`.
    pub fn set_light_enabled(&mut self, zero_node: i64, enable: bool) {
        if enable {
            self.columns_with_sources.insert(zero_node);
        } else {
            self.columns_with_sources.remove(&zero_node);
        }
    }

    /// Vanilla `LayerLightSectionStorage.lightOnInSection`.
    #[must_use]
    pub fn light_on_in_section(&self, section_node: i64) -> bool {
        self.light_on_in_column(Self::zero_node_of(section_node))
    }

    /// Vanilla `LayerLightSectionStorage.lightOnInColumn`.
    #[must_use]
    pub fn light_on_in_column(&self, zero_node: i64) -> bool {
        self.columns_with_sources.contains(&zero_node)
    }

    /// Vanilla `LayerLightSectionStorage.retainData`.
    pub fn retain_data(&mut self, zero_node: i64, retain: bool) {
        if retain {
            self.columns_to_retain_queued_data_for.insert(zero_node);
        } else {
            self.columns_to_retain_queued_data_for.remove(&zero_node);
        }
    }

    /// Returns whether queued light data is retained for a column.
    #[must_use]
    pub fn retains_queued_data(&self, zero_node: i64) -> bool {
        self.columns_to_retain_queued_data_for.contains(&zero_node)
    }

    /// Vanilla `LayerLightSectionStorage.sectionStates` lookup for one section.
    #[must_use]
    pub fn section_state(&self, section_node: i64) -> SectionState {
        self.section_states
            .get(&section_node)
            .copied()
            .unwrap_or_default()
    }

    /// Vanilla `LayerLightSectionStorage.getDebugSectionType`.
    #[must_use]
    pub fn section_type(&self, section_node: i64) -> SectionType {
        self.section_state(section_node).section_type()
    }

    /// Vanilla `LightEngine.getDebugData` for one section node.
    #[must_use]
    pub fn debug_data(&self, section_node: i64) -> String {
        self.section_type(section_node).display().to_owned()
    }

    /// Vanilla `LayerLightSectionStorage.storingLightForSection`.
    #[must_use]
    pub fn storing_light_for_section(&self, section_node: i64) -> bool {
        self.section_state(section_node).has_data()
    }

    /// Vanilla `LayerLightSectionStorage.queueSectionData`.
    pub fn queue_section_data(&mut self, section_node: i64, data: Option<DataLayer>) {
        if let Some(data) = data {
            self.queued_sections.insert(section_node, data);
            self.has_inconsistencies = true;
        } else {
            self.queued_sections.remove(&section_node);
        }
    }

    /// Vanilla `LayerLightSectionStorage.queuedSections` lookup.
    #[must_use]
    pub fn queued_section(&self, section_node: i64) -> Option<&DataLayer> {
        self.queued_sections.get(&section_node)
    }

    /// Returns the number of sections currently queued for this layer.
    #[must_use]
    pub fn queued_section_count(&self) -> usize {
        self.queued_sections.len()
    }

    /// Vanilla `LightEngine.getDataLayerData`: queued data wins over stored data.
    #[must_use]
    pub fn data_layer_data(
        &self,
        section_node: i64,
        stored: impl FnOnce(i64) -> Option<DataLayer>,
    ) -> Option<DataLayer> {
        if let Some(queued) = self.queued_sections.get(&section_node) {
            return Some(queued.copy());
        }
        stored(section_node)
    }

    /// Vanilla `LayerLightSectionStorage.updateSectionStatus`.
    ///
    /// Vanilla stores neighbour counts for all 26 neighboring sections; Steel
    /// keeps the same global section-state map, so the neighbour increments cross
    /// chunk boundaries exactly like vanilla.
    pub fn update_section_status(&mut self, section_node: i64, section_empty: bool) {
        let state = self.section_state(section_node);
        let new_state = if section_empty {
            state.without_has_data()
        } else {
            state.with_has_data()
        };
        if state != new_state {
            self.put_section_state(section_node, new_state);
            let neighbor_increment = if section_empty { -1 } else { 1 };
            let section = Self::section_from_node(section_node);

            for offset_x in -1..=1 {
                for offset_y in -1..=1 {
                    for offset_z in -1..=1 {
                        if offset_x == 0 && offset_y == 0 && offset_z == 0 {
                            continue;
                        }

                        let neighbor_node = Self::section_node(Self::offset_section(
                            section, offset_x, offset_y, offset_z,
                        ));
                        let neighbor_state = self.section_state(neighbor_node);
                        self.put_section_state(
                            neighbor_node,
                            neighbor_state.with_neighbor_count(
                                neighbor_state.neighbor_count() + neighbor_increment,
                            ),
                        );
                    }
                }
            }
        }
    }

    /// Vanilla `LayerLightSectionStorage.putSectionState`.
    fn put_section_state(&mut self, section_node: i64, state: SectionState) {
        if state.raw() != SECTION_STATE_EMPTY {
            if self.section_states.insert(section_node, state).is_none() {
                self.initialize_section(section_node);
            }
        } else if self.section_states.remove(&section_node).is_some() {
            self.remove_section(section_node);
        }
    }

    /// Vanilla `LayerLightSectionStorage.initializeSection`.
    ///
    /// Vanilla also installs the queued or fresh `DataLayer` into the updating
    /// section map; Steel's light data lives in the owning chunk, so the data
    /// install happens in the workset commit and only the state bookkeeping is
    /// recorded here.
    fn initialize_section(&mut self, section_node: i64) {
        if !self.to_remove.remove(&section_node) {
            self.changed_sections.insert(section_node);
            self.mark_section_and_neighbors_as_affected(section_node);
            self.has_inconsistencies = true;
        }
    }

    /// Vanilla `LayerLightSectionStorage.removeSection`.
    fn remove_section(&mut self, section_node: i64) {
        self.to_remove.insert(section_node);
        self.has_inconsistencies = true;
    }

    /// Vanilla `LayerLightSectionStorage.markSectionAndNeighborsAsAffected`.
    fn mark_section_and_neighbors_as_affected(&mut self, section_node: i64) {
        let section = Self::section_from_node(section_node);

        for offset_z in -1..=1 {
            for offset_x in -1..=1 {
                for offset_y in -1..=1 {
                    self.sections_affected_by_light_updates
                        .insert(Self::section_node(Self::offset_section(
                            section, offset_x, offset_y, offset_z,
                        )));
                }
            }
        }
    }

    /// Vanilla `LayerLightSectionStorage.hasInconsistencies`.
    #[must_use]
    pub const fn has_inconsistencies(&self) -> bool {
        self.has_inconsistencies
    }

    /// Vanilla `LayerLightSectionStorage.markNewInconsistencies`.
    ///
    /// Returns the sections whose queued data became visible so the caller can
    /// publish them into chunk-owned light storage. Steel's chunk light data is
    /// already the single source of truth for a chunk's stored layer, so every
    /// queued section that vanilla would move into `updatingSectionData` is
    /// reported back instead of being installed here.
    pub fn mark_new_inconsistencies(&mut self) -> Vec<(i64, DataLayer)> {
        let mut published = Vec::new();
        if !self.has_inconsistencies {
            return published;
        }

        self.has_inconsistencies = false;

        for node in self.to_remove.drain() {
            let queued = self.queued_sections.remove(&node);
            if self
                .columns_to_retain_queued_data_for
                .contains(&Self::zero_node_of(node))
            {
                if let Some(queued) = queued {
                    self.queued_sections.insert(node, queued);
                }
            }
            self.changed_sections.insert(node);
        }

        let storing = self
            .queued_sections
            .keys()
            .copied()
            .filter(|node| self.storing_light_for_section(*node))
            // Retained columns keep their queued data until the column is released;
            // publishing it now would consume the data `retain_data` preserves.
            .filter(|node| {
                !self
                    .columns_to_retain_queued_data_for
                    .contains(&Self::zero_node_of(*node))
            })
            .collect::<Vec<_>>();

        for node in storing {
            if let Some(data) = self.queued_sections.remove(&node) {
                self.changed_sections.insert(node);
                published.push((node, data));
            }
        }

        published
    }

    /// Vanilla `LayerLightSectionStorage.swapSectionMap`.
    ///
    /// Returns the sections that must be reported through `onLightUpdate`.
    pub fn swap_section_map(&mut self) -> Vec<i64> {
        self.changed_sections.clear();
        self.sections_affected_by_light_updates.drain().collect()
    }

    /// Returns true when this layer is fully idle (no pending state or data).
    #[must_use]
    pub fn is_idle(&self) -> bool {
        !self.has_inconsistencies
            && self.queued_sections.is_empty()
            && self.to_remove.is_empty()
            && self.changed_sections.is_empty()
            && self.sections_affected_by_light_updates.is_empty()
            && self.section_states.is_empty()
            && self.columns_with_sources.is_empty()
            && self.columns_to_retain_queued_data_for.is_empty()
    }
}

/// Vanilla `LevelLightEngine` state: the block layer plus the optional sky layer.
///
/// Steel does not have separate `BlockLightEngine`/`SkyLightEngine` types, so this
/// holds one [`LayerLightState`] per light layer and dispatches like
/// `LevelLightEngine` does. Vanilla's `hasBlockLight` is true for every builtin
/// dimension, so the block layer is always present here.
#[derive(Debug)]
pub struct LevelLightState {
    has_skylight: bool,
    block: LayerLightState,
    sky: LayerLightState,
}

impl LevelLightState {
    /// Creates the level light engine state for a dimension.
    #[must_use]
    pub fn new(has_skylight: bool) -> Self {
        Self {
            has_skylight,
            block: LayerLightState::new(),
            sky: LayerLightState::new(),
        }
    }

    /// Returns whether this level has a sky light layer.
    #[must_use]
    pub const fn has_skylight(&self) -> bool {
        self.has_skylight
    }

    /// Returns one layer's state, or `None` for a sky layer on a skyless dimension.
    #[must_use]
    pub fn layer(&self, layer: LightLayer) -> Option<&LayerLightState> {
        match layer {
            LightLayer::Block => Some(&self.block),
            LightLayer::Sky if self.has_skylight => Some(&self.sky),
            LightLayer::Sky => None,
        }
    }

    /// Returns one layer's state mutably, or `None` for a sky layer on a skyless dimension.
    pub fn layer_mut(&mut self, layer: LightLayer) -> Option<&mut LayerLightState> {
        match layer {
            LightLayer::Block => Some(&mut self.block),
            LightLayer::Sky if self.has_skylight => Some(&mut self.sky),
            LightLayer::Sky => None,
        }
    }

    /// Vanilla `LevelLightEngine.setLightEnabled`.
    pub fn set_light_enabled(&mut self, chunk_pos: ChunkPos, enable: bool) {
        let zero_node = LayerLightState::zero_node(chunk_pos);
        self.block.set_light_enabled(zero_node, enable);
        if self.has_skylight {
            self.sky.set_light_enabled(zero_node, enable);
        }
    }

    /// Vanilla `LevelLightEngine.retainData`.
    pub fn retain_data(&mut self, chunk_pos: ChunkPos, retain: bool) {
        let zero_node = LayerLightState::zero_node(chunk_pos);
        self.block.retain_data(zero_node, retain);
        if self.has_skylight {
            self.sky.retain_data(zero_node, retain);
        }
    }

    /// Vanilla `LevelLightEngine.lightOnInColumn`.
    #[must_use]
    pub fn light_on_in_column(&self, chunk_pos: ChunkPos) -> bool {
        let zero_node = LayerLightState::zero_node(chunk_pos);
        self.block.light_on_in_column(zero_node)
            && (!self.has_skylight || self.sky.light_on_in_column(zero_node))
    }

    /// Vanilla `LevelLightEngine.queueSectionData`.
    pub fn queue_section_data(
        &mut self,
        layer: LightLayer,
        section_pos: SectionPos,
        data: Option<DataLayer>,
    ) {
        if let Some(state) = self.layer_mut(layer) {
            state.queue_section_data(LayerLightState::section_node(section_pos), data);
        }
    }

    /// Vanilla `LevelLightEngine.updateSectionStatus`.
    pub fn update_section_status(&mut self, section_pos: SectionPos, section_empty: bool) {
        let section_node = LayerLightState::section_node(section_pos);
        self.block
            .update_section_status(section_node, section_empty);
        if self.has_skylight {
            self.sky.update_section_status(section_node, section_empty);
        }
    }

    /// Vanilla `LevelLightEngine.hasLightWork`'s storage half.
    #[must_use]
    pub fn has_inconsistencies(&self) -> bool {
        self.block.has_inconsistencies() || (self.has_skylight && self.sky.has_inconsistencies())
    }

    /// Returns the vanilla `LayerLightSectionStorage.SectionType` of one section.
    ///
    /// Vanilla `LevelLightEngine.getDebugSectionType` returns `EMPTY` for a layer
    /// this dimension does not have.
    #[must_use]
    pub fn debug_section_type(&self, layer: LightLayer, section_pos: SectionPos) -> SectionType {
        match self.layer(layer) {
            Some(state) => state.section_type(LayerLightState::section_node(section_pos)),
            None => SectionType::Empty,
        }
    }

    /// Vanilla `LevelLightEngine.getDebugData`, including its `"n/a"` fallback.
    #[must_use]
    pub fn debug_data(&self, layer: LightLayer, section_pos: SectionPos) -> String {
        match self.layer(layer) {
            Some(state) => state.debug_data(LayerLightState::section_node(section_pos)),
            None => "n/a".to_owned(),
        }
    }

    /// Vanilla `LightEngine.getDataLayerData` for one section of one layer.
    #[must_use]
    pub fn data_layer_data(
        &self,
        layer: LightLayer,
        section_pos: SectionPos,
        stored: impl FnOnce(i64) -> Option<DataLayer>,
    ) -> Option<DataLayer> {
        self.layer(layer)?
            .data_layer_data(LayerLightState::section_node(section_pos), stored)
    }

    /// Runs vanilla `markNewInconsistencies` for both layers.
    ///
    /// Returns the queued sections that became visible, tagged with their layer.
    pub fn mark_new_inconsistencies(&mut self) -> Vec<(LightLayer, i64, DataLayer)> {
        let mut published = self
            .block
            .mark_new_inconsistencies()
            .into_iter()
            .map(|(node, data)| (LightLayer::Block, node, data))
            .collect::<Vec<_>>();
        if self.has_skylight {
            published.extend(
                self.sky
                    .mark_new_inconsistencies()
                    .into_iter()
                    .map(|(node, data)| (LightLayer::Sky, node, data)),
            );
        }
        published
    }

    /// Runs vanilla `swapSectionMap`'s `onLightUpdate` half for both layers.
    pub fn swap_section_maps(&mut self) -> Vec<(LightLayer, i64)> {
        let mut affected = self
            .block
            .swap_section_map()
            .into_iter()
            .map(|node| (LightLayer::Block, node))
            .collect::<Vec<_>>();
        if self.has_skylight {
            affected.extend(
                self.sky
                    .swap_section_map()
                    .into_iter()
                    .map(|node| (LightLayer::Sky, node)),
            );
        }
        affected
    }

    /// Returns true when both layers are idle.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.block.is_idle() && (!self.has_skylight || self.sky.is_idle())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_state_matches_vanilla_packing() {
        let state = SectionState::default()
            .with_neighbor_count(3)
            .with_has_data();
        assert!(state.has_data());
        assert_eq!(state.neighbor_count(), 3);
        assert_eq!(state.section_type(), SectionType::LightAndData);
        assert_eq!(
            state.without_has_data().section_type(),
            SectionType::LightOnly
        );
        assert_eq!(SectionState::default().section_type(), SectionType::Empty);
        assert_eq!(SectionType::Empty.display(), "2");
        assert_eq!(SectionType::LightOnly.display(), "1");
        assert_eq!(SectionType::LightAndData.display(), "0");
    }

    #[test]
    fn empty_state_after_data_removal_is_empty_again() {
        let state = SectionState::default().with_has_data();
        assert_eq!(state.without_has_data().section_type(), SectionType::Empty);
        assert_eq!(
            SectionState::from_raw(state.raw() & !HAS_DATA_BIT & !NEIGHBOR_COUNT_BITS)
                .section_type(),
            SectionType::Empty
        );
    }

    #[test]
    fn zero_node_matches_chunk_pos_packing() {
        let pos = ChunkPos::new(-3, 7);
        assert_eq!(
            LayerLightState::zero_node(pos),
            LayerLightState::section_node(SectionPos::new(-3, 0, 7))
        );
        assert_ne!(
            LayerLightState::zero_node(pos),
            steel_utils::PackedChunkPos::from(pos).as_raw()
        );
    }

    #[test]
    fn section_node_matches_vanilla_as_long_layout() {
        let (x, y, z) = (1_i64, 2_i64, 3_i64);
        let expected = ((x & 0x3F_FFFF) << 42) | ((z & 0x3F_FFFF) << 20) | (y & 0xF_FFFF);
        assert_eq!(
            LayerLightState::section_node(SectionPos::new(1, 2, 3)),
            expected
        );
    }

    #[test]
    fn zero_node_of_section_node_drops_section_y() {
        let section = SectionPos::new(4, -5, 9);
        let node = LayerLightState::section_node(section);
        assert_eq!(
            LayerLightState::zero_node_of(node),
            LayerLightState::zero_node(ChunkPos::new(4, 9))
        );
        assert_eq!(LayerLightState::section_from_node(node), section);
    }

    #[test]
    fn update_section_status_tracks_neighbors_across_columns() {
        let mut state = LayerLightState::new();
        let center = SectionPos::new(0, 4, 0);
        let center_node = LayerLightState::section_node(center);
        state.update_section_status(center_node, false);
        assert!(state.storing_light_for_section(center_node));
        assert_eq!(state.section_state(center_node).neighbor_count(), 0);

        let neighbor = SectionPos::new(-1, 4, 0);
        let neighbor_node = LayerLightState::section_node(neighbor);
        state.update_section_status(neighbor_node, false);
        assert_eq!(state.section_state(center_node).neighbor_count(), 1);

        state.update_section_status(neighbor_node, true);
        assert_eq!(state.section_state(center_node).neighbor_count(), 0);
    }

    #[test]
    fn mark_new_inconsistencies_publishes_queued_sections_that_store_light() {
        let mut state = LayerLightState::new();
        let section_node = LayerLightState::section_node(SectionPos::new(0, 1, 0));
        state.update_section_status(section_node, false);
        state.mark_new_inconsistencies();
        state.queue_section_data(section_node, Some(DataLayer::filled(7)));

        assert!(state.has_inconsistencies());
        let published = state.mark_new_inconsistencies();
        assert_eq!(published.len(), 1);
        assert_eq!(published[0].0, section_node);
        assert_eq!(published[0].1.homogeneous_value(), Some(7));
        assert!(!state.has_inconsistencies());
    }

    #[test]
    fn retained_queued_data_survives_removal() {
        let mut state = LayerLightState::new();
        let pos = ChunkPos::new(2, 2);
        let zero_node = LayerLightState::zero_node(pos);
        let section_node = LayerLightState::section_node(SectionPos::new(2, 1, 2));
        state.retain_data(zero_node, true);
        state.queue_section_data(section_node, Some(DataLayer::filled(4)));
        state
            .section_states
            .insert(section_node, SectionState::default().with_has_data());
        state.remove_section(section_node);

        state.mark_new_inconsistencies();
        assert!(state.queued_section(section_node).is_some());
        state.queue_section_data(section_node, None);
        assert!(state.queued_section(section_node).is_none());
    }

    #[test]
    fn data_layer_data_prefers_queued_data() {
        let mut state = LayerLightState::new();
        let section_node = LayerLightState::section_node(SectionPos::new(0, 0, 0));
        state.queue_section_data(section_node, Some(DataLayer::filled(3)));
        let data = state
            .data_layer_data(section_node, |_| Some(DataLayer::filled(12)))
            .expect("queued data should be returned");
        assert_eq!(data.homogeneous_value(), Some(3));

        state.queue_section_data(section_node, None);
        let data = state
            .data_layer_data(section_node, |_| Some(DataLayer::filled(12)))
            .expect("stored data should be returned");
        assert_eq!(data.homogeneous_value(), Some(12));
    }
}
