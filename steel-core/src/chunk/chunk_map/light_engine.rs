//! Vanilla `LevelLightEngine` / `LightEventListener` API on top of [`ChunkMap`].
//!
//! Steel keeps light *data* in the owning chunk and propagates through scoped
//! worksets, so the engine-side bookkeeping lives in
//! [`crate::chunk::light::LevelLightState`]. This module exposes the vanilla
//! `LightEventListener` surface (`setLightEnabled`, `retainData`,
//! `queueSectionData`, `updateSectionStatus`, `getDataLayerData`,
//! `getDebugData`, `hasInconsistencies`) against that state, and runs vanilla's
//! `markNewInconsistencies` / `swapSectionMap` pair after each propagation round.

use steel_utils::SectionPos;

use super::{ChunkMap, ChunkPos, ChunkStatus};
use crate::chunk::light::{
    DataLayer, LayerLightState, LightLayer, LightSection, LightSectionData, LightSectionRange,
    SectionType,
};

impl ChunkMap {
    /// Vanilla `LightEventListener.setLightEnabled`.
    ///
    /// Vanilla calls this from `ThreadedLevelLightEngine.initializeLight` (with
    /// the persisted light-correct flag) and from `updateChunkStatus` (with
    /// `false`). While a column is disabled, sky sections are treated as fully
    /// lit for the engine's section-state bookkeeping.
    pub fn set_light_enabled(&self, chunk_pos: ChunkPos, enable: bool) {
        self.light_engine
            .lock()
            .set_light_enabled(chunk_pos, enable);
    }

    /// Vanilla `LightEventListener.retainData`.
    ///
    /// Keeps queued section light for a column across `markNewInconsistencies`
    /// removal passes; used while persisted light is being handed to the engine
    /// (`SerializableChunkData` calls `retainData(pos, true)` before queuing
    /// each section and `retainData(pos, false)` after lighting).
    pub fn retain_light_data(&self, chunk_pos: ChunkPos, retain: bool) {
        self.light_engine.lock().retain_data(chunk_pos, retain);
    }

    /// Vanilla `LightEventListener.queueSectionData`.
    ///
    /// Queued data is published into chunk-owned light storage by
    /// [`Self::apply_light_engine_inconsistencies`], mirroring
    /// `LayerLightSectionStorage.markNewInconsistencies`.
    pub fn queue_light_section_data(
        &self,
        layer: LightLayer,
        section_pos: SectionPos,
        data: Option<DataLayer>,
    ) {
        self.light_engine
            .lock()
            .queue_section_data(layer, section_pos, data);
    }

    /// Vanilla `LightEventListener.updateSectionStatus`.
    pub fn update_light_section_status(&self, section_pos: SectionPos, section_empty: bool) {
        self.light_engine
            .lock()
            .update_section_status(section_pos, section_empty);
    }

    /// Vanilla `LightEventListener.hasLightWork`'s storage half.
    ///
    /// Steel tracks queue work separately (`has_pending_light_updates`); this
    /// reports only `LayerLightSectionStorage.hasInconsistencies`.
    #[must_use]
    pub fn has_light_inconsistencies(&self) -> bool {
        self.light_engine.lock().has_inconsistencies()
    }

    /// Vanilla `LevelLightEngine.lightOnInColumn`.
    #[must_use]
    pub fn light_on_in_column(&self, chunk_pos: ChunkPos) -> bool {
        self.light_engine.lock().light_on_in_column(chunk_pos)
    }

    /// Vanilla `LevelLightEngine.getDebugData`.
    #[must_use]
    pub fn light_debug_data(&self, layer: LightLayer, section_pos: SectionPos) -> String {
        self.light_engine.lock().debug_data(layer, section_pos)
    }

    /// Vanilla `LevelLightEngine.getDebugSectionType`.
    #[must_use]
    pub fn light_debug_section_type(
        &self,
        layer: LightLayer,
        section_pos: SectionPos,
    ) -> SectionType {
        self.light_engine
            .lock()
            .debug_section_type(layer, section_pos)
    }

    /// Vanilla `LightEventListener.getDataLayerData`: queued data wins over the
    /// chunk's committed light data.
    #[must_use]
    pub fn light_data_layer(
        &self,
        layer: LightLayer,
        section_pos: SectionPos,
    ) -> Option<DataLayer> {
        let stored = || self.stored_light_data_layer(layer, section_pos);
        self.light_engine
            .lock()
            .data_layer_data(layer, section_pos, |_| stored())
    }

    fn stored_light_data_layer(
        &self,
        layer: LightLayer,
        section_pos: SectionPos,
    ) -> Option<DataLayer> {
        let chunk_pos = ChunkPos::new(section_pos.x(), section_pos.z());
        let holder = self.light_update_holder(chunk_pos)?;
        let chunk = holder.try_chunk(ChunkStatus::Light)?;
        let light = chunk.light();
        let storage = match layer {
            LightLayer::Sky => &light.sky,
            LightLayer::Block => &light.block,
        };

        storage
            .section(section_pos.y())
            .and_then(LightSection::visible_data)
            .map(LightSectionData::to_data_layer)
    }

    /// Runs vanilla `ThreadedLevelLightEngine.updateChunkStatus`'s light half.
    ///
    /// Vanilla disables light for the column, clears every queued layer, marks
    /// every real section empty, and releases retained data. Steel calls this
    /// when a column is finalized on unload so the engine's section-state map
    /// cannot grow without bound.
    pub(super) fn release_light_column(&self, chunk_pos: ChunkPos, range: LightSectionRange) {
        let mut engine = self.light_engine.lock();
        engine.retain_data(chunk_pos, false);
        engine.set_light_enabled(chunk_pos, false);
        for section_y in range.min_section_y()..range.max_section_y_exclusive() {
            let section_pos = SectionPos::new(chunk_pos.0.x, section_y, chunk_pos.0.y);
            engine.queue_section_data(LightLayer::Block, section_pos, None);
            engine.queue_section_data(LightLayer::Sky, section_pos, None);
        }
        for section_y in range.min_chunk_section_y()..range.max_chunk_section_y_exclusive() {
            engine.update_section_status(
                SectionPos::new(chunk_pos.0.x, section_y, chunk_pos.0.y),
                true,
            );
        }
    }

    /// Runs vanilla `LightEngine.runLightUpdates`'s storage epilogue.
    ///
    /// `markNewInconsistencies` publishes queued section light into chunk-owned
    /// storage; `swapSectionMap` reports every section touched by the round
    /// through `onLightUpdate` (Steel: `light_changed`, which drives packets and
    /// the saved-light dirty flag).
    pub(super) fn apply_light_engine_inconsistencies(&self) {
        // Vanilla runs both steps unconditionally at the end of `runLightUpdates`;
        // `markNewInconsistencies` is itself a no-op unless the flag is set, and
        // `swapSectionMap` must still flush any pending `onLightUpdate` reports.
        let published = self.light_engine.lock().mark_new_inconsistencies();
        for (layer, section_node, data) in published {
            self.publish_light_data_layer(
                layer,
                LayerLightState::section_from_node(section_node),
                &data,
            );
        }

        let affected = self.light_engine.lock().swap_section_maps();
        for (layer, section_node) in affected {
            self.light_changed(layer, LayerLightState::section_from_node(section_node));
        }
    }

    fn publish_light_data_layer(
        &self,
        layer: LightLayer,
        section_pos: SectionPos,
        data: &DataLayer,
    ) {
        let chunk_pos = ChunkPos::new(section_pos.x(), section_pos.z());
        let Some(holder) = self.light_update_holder(chunk_pos) else {
            return;
        };
        let Some(chunk) = holder.try_chunk(ChunkStatus::Light) else {
            return;
        };

        let mut light = chunk.light_mut();
        let storage = match layer {
            LightLayer::Sky => &mut light.sky,
            LightLayer::Block => &mut light.block,
        };
        if let Some(section) = storage.section_mut(section_pos.y()) {
            *section = LightSection::visible(LightSectionData::from_data_layer(data));
        }
    }
}
