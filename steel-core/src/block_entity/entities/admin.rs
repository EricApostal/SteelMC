//! `StructureBlockEntity`, `JigsawBlockEntity`, `TestBlockEntity` and
//! `TestInstanceBlockEntity`.
//!
//! These are the admin-only block entities: they persist their editor configuration so a server
//! restart keeps the setup, and the block behaviors drive them from redstone and use-interaction.

use std::sync::Arc;
use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, FrontAndTop, StructureMode, TestBlockMode,
};
use steel_registry::{vanilla_block_entity_types, vanilla_blocks};
use steel_utils::types::UpdateFlags;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

// ---------------------------------------------------------------------------
// StructureBlockEntity
// ---------------------------------------------------------------------------

/// Vanilla `StructureBlockEntity` state.
#[derive(Clone, Debug, Default)]
pub struct StructureData {
    /// Structure name.
    pub name: String,
    /// Placing player's name.
    pub author: String,
    /// Free-form metadata string.
    pub metadata: String,
    /// Structure origin.
    pub pos: (i32, i32, i32),
    /// Structure size.
    pub size: (i32, i32, i32),
    /// `Rotation` ordinal.
    pub rotation: i32,
    /// `Mirror` ordinal.
    pub mirror: i32,
    /// `StructureMode` name.
    pub mode: String,
    /// Whether the block is redstone-powered.
    pub powered: bool,
    /// Whether the bounding box is rendered.
    pub show_bounding_box: bool,
    /// Whether air blocks are rendered.
    pub show_air: bool,
    /// Structure integrity (0-1).
    pub integrity: f32,
    /// Placement seed.
    pub seed: i64,
    /// Whether entities are ignored when loading.
    pub ignore_entities: bool,
    /// Whether this block is owned by the jigsaw system.
    pub used_by_jigsaw: bool,
}

/// Vanilla `StructureMode.LEGACY_CODEC` name for a block-state mode.
const fn structure_mode_name(mode: StructureMode) -> &'static str {
    match mode {
        StructureMode::Save => "SAVE",
        StructureMode::Load => "LOAD",
        StructureMode::Corner => "CORNER",
        StructureMode::Data => "DATA",
    }
}

/// Parses a vanilla legacy `StructureMode` name; unknown values load as `DATA`.
fn parse_structure_mode(name: &str) -> StructureMode {
    match name {
        "SAVE" => StructureMode::Save,
        "LOAD" => StructureMode::Load,
        "CORNER" => StructureMode::Corner,
        _ => StructureMode::Data,
    }
}

/// Vanilla `TestBlockMode.CODEC` name for a block-state mode.
const fn test_block_mode_name(mode: TestBlockMode) -> &'static str {
    match mode {
        TestBlockMode::Start => "start",
        TestBlockMode::Log => "log",
        TestBlockMode::Fail => "fail",
        TestBlockMode::Accept => "accept",
    }
}

/// `StructureBlockEntity`.
pub struct StructureBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<StructureData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `StructureBlockEntity`.
unsafe impl DowncastType for StructureBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/structure_block");
}

impl StructureBlockEntity {
    /// Creates a structure-block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let mode = structure_mode_name(state.get_value(&BlockStateProperties::STRUCTUREBLOCK_MODE));
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::STRUCTURE_BLOCK,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(StructureData {
                size: (1, 1, 1),
                integrity: 1.0,
                ignore_entities: true,
                show_bounding_box: true,
                mode: mode.into(),
                ..StructureData::default()
            }),
        }
    }

    /// Returns a snapshot of the structure configuration.
    #[must_use]
    pub fn data(&self) -> StructureData {
        self.state.lock().clone()
    }

    /// Vanilla `StructureBlockEntity.isPowered`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.state.lock().powered
    }

    /// Vanilla `StructureBlockEntity.setPowered`.
    pub fn set_powered(&self, powered: bool) {
        let mut state = self.state.lock();
        state.powered = powered;
        drop(state);
        BlockEntity::set_changed(self);
    }

    /// Vanilla `StructureBlockEntity.updateBlockState`: re-applies the persisted
    /// mode to the block state after loading.
    fn update_block_state(&self) {
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = world.get_block_state(pos);
        if state.get_block() != &vanilla_blocks::STRUCTURE_BLOCK {
            return;
        }
        let mode = parse_structure_mode(&self.state.lock().mode);
        world.set_block(
            pos,
            state.set_value(&BlockStateProperties::STRUCTUREBLOCK_MODE, mode),
            UpdateFlags::UPDATE_CLIENTS,
        );
    }

    /// Vanilla `StructureBlockEntity.setMode`, including the block-state update.
    pub fn set_mode(&self, mode: StructureMode) {
        self.state.lock().mode = structure_mode_name(mode).into();
        self.update_block_state();
    }

    /// Vanilla `StructureBlockEntity.setStructureName`.
    pub fn set_structure_name(&self, name: &str) {
        name.clone_into(&mut self.state.lock().name);
    }

    /// Vanilla `StructureBlockEntity.hasStructureName`.
    #[must_use]
    pub fn has_structure_name(&self) -> bool {
        !self.state.lock().name.is_empty()
    }

    /// Vanilla `StructureBlockEntity.setStructurePos`.
    pub fn set_pos(&self, pos: (i32, i32, i32)) {
        self.state.lock().pos = pos;
    }

    /// Vanilla `StructureBlockEntity.setStructureSize`.
    pub fn set_size(&self, size: (i32, i32, i32)) {
        self.state.lock().size = size;
    }

    /// Vanilla `StructureBlockEntity.setRotation`, taking a `Rotation` ordinal.
    pub fn set_rotation(&self, rotation: i32) {
        self.state.lock().rotation = rotation;
    }

    /// Vanilla `StructureBlockEntity.setMirror`, taking a `Mirror` ordinal.
    pub fn set_mirror(&self, mirror: i32) {
        self.state.lock().mirror = mirror;
    }

    /// Vanilla `StructureBlockEntity.setMetaData`.
    pub fn set_metadata(&self, metadata: &str) {
        metadata.clone_into(&mut self.state.lock().metadata);
    }

    /// Vanilla `StructureBlockEntity.setIgnoreEntities`.
    pub fn set_ignore_entities(&self, ignore_entities: bool) {
        self.state.lock().ignore_entities = ignore_entities;
    }

    /// Vanilla `StructureBlockEntity.setStrict`.
    pub fn set_strict(&self, strict: bool) {
        self.state.lock().used_by_jigsaw = strict;
    }

    /// Vanilla `StructureBlockEntity.setShowAir`.
    pub fn set_show_air(&self, show_air: bool) {
        self.state.lock().show_air = show_air;
    }

    /// Vanilla `StructureBlockEntity.setShowBoundingBox`.
    pub fn set_show_bounding_box(&self, show_bounding_box: bool) {
        self.state.lock().show_bounding_box = show_bounding_box;
    }

    /// Vanilla `StructureBlockEntity.setIntegrity`.
    pub fn set_integrity(&self, integrity: f32) {
        self.state.lock().integrity = integrity;
    }

    /// Vanilla `StructureBlockEntity.setSeed`.
    pub fn set_seed(&self, seed: i64) {
        self.state.lock().seed = seed;
    }

    /// Vanilla `StructureBlockEntity.trigger` on a redstone rising edge.
    ///
    /// Only the redstone bookkeeping is a block-entity concern; the actual structure save/load
    /// requires the structure template manager.
    pub const fn trigger(_world: &Arc<World>, _pos: BlockPos) {
        // TODO(Worldgen): invoke structure save/load from `StructureTemplateManager` once that
        // runtime exists for the worldgen lane (vanilla `StructureBlockEntity.load`/`saveStructure`).
    }
}

impl BlockEntity for StructureBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.name = view.string("name").map_or_default(ToString::to_string);
        state.author = view.string("author").map_or_default(ToString::to_string);
        state.metadata = view.string("metadata").map_or_default(ToString::to_string);
        // Vanilla `StructureBlockEntity.loadAdditional`: flat `posX/Y/Z` and
        // `sizeX/Y/Z` keys, clamped to the editor's -48..48 / 0..48 ranges.
        state.pos = (
            view.int("posX").unwrap_or(0).clamp(-48, 48),
            view.int("posY").unwrap_or(0).clamp(-48, 48),
            view.int("posZ").unwrap_or(0).clamp(-48, 48),
        );
        state.size = (
            view.int("sizeX").unwrap_or(1).clamp(0, 48),
            view.int("sizeY").unwrap_or(1).clamp(0, 48),
            view.int("sizeZ").unwrap_or(1).clamp(0, 48),
        );
        state.rotation = view
            .string("rotation")
            .map_or(0, |v| match v.to_str().as_ref() {
                "CLOCKWISE_90" => 1,
                "CLOCKWISE_180" => 2,
                "COUNTERCLOCKWISE_90" => 3,
                _ => 0,
            });
        state.mirror = view
            .string("mirror")
            .map_or(0, |v| match v.to_str().as_ref() {
                "LEFT_RIGHT" => 1,
                "FRONT_BACK" => 2,
                _ => 0,
            });
        state.mode = view
            .string("mode")
            .map_or_else(|| "DATA".into(), ToString::to_string);
        state.ignore_entities = view.byte("ignoreEntities").unwrap_or(1) != 0;
        state.powered = view.byte("powered").unwrap_or(0) != 0;
        state.show_bounding_box = view.byte("showboundingbox").unwrap_or(1) != 0;
        state.show_air = view.byte("showair").unwrap_or(0) != 0;
        state.integrity = view.float("integrity").unwrap_or(1.0);
        state.seed = view.long("seed").unwrap_or(0);
        state.used_by_jigsaw = view.byte("strict").unwrap_or(0) != 0;
        drop(state);
        self.update_block_state();
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("name", state.name.clone());
        nbt.insert("author", state.author.clone());
        nbt.insert("metadata", state.metadata.clone());
        nbt.insert("posX", state.pos.0);
        nbt.insert("posY", state.pos.1);
        nbt.insert("posZ", state.pos.2);
        nbt.insert("sizeX", state.size.0);
        nbt.insert("sizeY", state.size.1);
        nbt.insert("sizeZ", state.size.2);
        nbt.insert(
            "rotation",
            match state.rotation {
                1 => "CLOCKWISE_90",
                2 => "CLOCKWISE_180",
                3 => "COUNTERCLOCKWISE_90",
                _ => "NONE",
            },
        );
        nbt.insert(
            "mirror",
            match state.mirror {
                1 => "LEFT_RIGHT",
                2 => "FRONT_BACK",
                _ => "NONE",
            },
        );
        nbt.insert("mode", state.mode.clone());
        nbt.insert("ignoreEntities", i8::from(state.ignore_entities));
        nbt.insert("strict", i8::from(state.used_by_jigsaw));
        nbt.insert("powered", i8::from(state.powered));
        nbt.insert("showair", i8::from(state.show_air));
        nbt.insert("showboundingbox", i8::from(state.show_bounding_box));
        nbt.insert("integrity", state.integrity);
        nbt.insert("seed", state.seed);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `StructureBlockEntity.getUpdateTag` returns `saveCustomOnly`.
        Some(self.save_custom_only())
    }
}

// ---------------------------------------------------------------------------
// JigsawBlockEntity
// ---------------------------------------------------------------------------

/// Vanilla `JigsawBlockEntity` state.
#[derive(Clone, Debug, Default)]
pub struct JigsawData {
    /// `name` joint id.
    pub name: String,
    /// Target joint id.
    pub target: String,
    /// Template pool key.
    pub pool: String,
    /// `final_state` block.
    pub final_state: String,
    /// `joint` type name.
    pub joint: String,
    /// Placement priority.
    pub placement_priority: i32,
    /// Selection priority.
    pub selection_priority: i32,
}

/// `JigsawBlockEntity`.
pub struct JigsawBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<JigsawData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `JigsawBlockEntity`.
unsafe impl DowncastType for JigsawBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/jigsaw");
}

impl JigsawBlockEntity {
    /// Creates a jigsaw-block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::JIGSAW, level, pos, state),
            state: SyncMutex::new(JigsawData {
                name: "minecraft:empty".into(),
                target: "minecraft:empty".into(),
                pool: "minecraft:empty".into(),
                final_state: "minecraft:air".into(),
                joint: "rollable".into(),
                ..JigsawData::default()
            }),
        }
    }

    /// Returns a snapshot of the jigsaw configuration.
    #[must_use]
    pub fn data(&self) -> JigsawData {
        self.state.lock().clone()
    }

    /// Vanilla `JigsawBlockEntity.setName`.
    pub fn set_name(&self, name: &str) {
        name.clone_into(&mut self.state.lock().name);
    }

    /// Vanilla `JigsawBlockEntity.setTarget`.
    pub fn set_target(&self, target: &str) {
        target.clone_into(&mut self.state.lock().target);
    }

    /// Vanilla `JigsawBlockEntity.setPool`.
    pub fn set_pool(&self, pool: &str) {
        pool.clone_into(&mut self.state.lock().pool);
    }

    /// Vanilla `JigsawBlockEntity.setFinalState`.
    pub fn set_final_state(&self, final_state: &str) {
        final_state.clone_into(&mut self.state.lock().final_state);
    }

    /// Vanilla `JigsawBlockEntity.setJoint`.
    pub fn set_joint(&self, joint: &str) {
        joint.clone_into(&mut self.state.lock().joint);
    }

    /// Vanilla `JigsawBlockEntity.setPlacementPriority`.
    pub fn set_placement_priority(&self, priority: i32) {
        self.state.lock().placement_priority = priority;
    }

    /// Vanilla `JigsawBlockEntity.setSelectionPriority`.
    pub fn set_selection_priority(&self, priority: i32) {
        self.state.lock().selection_priority = priority;
    }
}

impl BlockEntity for JigsawBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        // Vanilla defaults: `minecraft:empty` for ids/pool, `minecraft:air` for the
        // final state, and the joint derived from the block orientation.
        state.name = view
            .string("name")
            .map_or_else(|| "minecraft:empty".into(), ToString::to_string);
        state.target = view
            .string("target")
            .map_or_else(|| "minecraft:empty".into(), ToString::to_string);
        state.pool = view
            .string("pool")
            .map_or_else(|| "minecraft:empty".into(), ToString::to_string);
        state.final_state = view
            .string("final_state")
            .map_or_else(|| "minecraft:air".into(), ToString::to_string);
        state.joint = view.string("joint").map_or_else(
            || {
                let orientation = self
                    .get_block_state()
                    .get_value(&BlockStateProperties::ORIENTATION);
                if matches!(
                    orientation,
                    FrontAndTop::WestUp
                        | FrontAndTop::EastUp
                        | FrontAndTop::NorthUp
                        | FrontAndTop::SouthUp
                ) {
                    "aligned".into()
                } else {
                    "rollable".into()
                }
            },
            ToString::to_string,
        );
        state.placement_priority = view.int("placement_priority").unwrap_or(0);
        state.selection_priority = view.int("selection_priority").unwrap_or(0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("name", state.name.clone());
        nbt.insert("target", state.target.clone());
        nbt.insert("pool", state.pool.clone());
        nbt.insert("final_state", state.final_state.clone());
        nbt.insert("joint", state.joint.clone());
        nbt.insert("placement_priority", state.placement_priority);
        nbt.insert("selection_priority", state.selection_priority);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `JigsawBlockEntity.getUpdateTag` returns `saveCustomOnly`.
        Some(self.save_custom_only())
    }
}

// ---------------------------------------------------------------------------
// TestBlockEntity / TestInstanceBlockEntity
// ---------------------------------------------------------------------------

/// Vanilla `TestBlockEntity` state.
#[derive(Clone, Debug)]
pub struct TestBlockData {
    /// Test mode mirrored from the `mode` block-state property.
    pub mode: TestBlockMode,
    /// Message logged by the block.
    pub message: String,
    /// Whether the block is powered by the test runner.
    pub powered: bool,
    /// Runtime-only latch set by `trigger`; never persisted in vanilla.
    pub triggered: bool,
}

impl Default for TestBlockData {
    fn default() -> Self {
        Self {
            mode: TestBlockMode::Fail,
            message: String::new(),
            powered: false,
            triggered: false,
        }
    }
}

/// `TestBlockEntity`.
pub struct TestBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<TestBlockData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TestBlockEntity`.
unsafe impl DowncastType for TestBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/test_block");
}

impl TestBlockEntity {
    /// Creates a test-block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let mode = state.get_value(&BlockStateProperties::TEST_BLOCK_MODE);
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::TEST_BLOCK, level, pos, state),
            state: SyncMutex::new(TestBlockData {
                mode,
                ..TestBlockData::default()
            }),
        }
    }

    /// Returns a snapshot of the test configuration.
    #[must_use]
    pub fn data(&self) -> TestBlockData {
        self.state.lock().clone()
    }

    /// Vanilla `TestBlockEntity.setMode`, including the block-state update.
    pub fn set_mode(&self, mode: TestBlockMode) {
        self.state.lock().mode = mode;
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = world.get_block_state(pos);
        if state.get_block() != &vanilla_blocks::TEST_BLOCK {
            return;
        }
        world.set_block(
            pos,
            state.set_value(&BlockStateProperties::TEST_BLOCK_MODE, mode),
            UpdateFlags::UPDATE_CLIENTS,
        );
    }

    /// Vanilla `TestBlockEntity.setMessage`.
    pub fn set_message(&self, message: &str) {
        message.clone_into(&mut self.state.lock().message);
    }
}

impl BlockEntity for TestBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        // Vanilla `TestBlockEntity.loadAdditional`: `mode` (default `FAIL`),
        // `message` (default empty) and `powered` (default false).
        state.mode =
            view.string("mode")
                .map_or(TestBlockMode::Fail, |v| match v.to_str().as_ref() {
                    "start" => TestBlockMode::Start,
                    "log" => TestBlockMode::Log,
                    "accept" => TestBlockMode::Accept,
                    _ => TestBlockMode::Fail,
                });
        state.message = view
            .string("message")
            .map_or_else(String::new, ToString::to_string);
        state.powered = view.byte("powered").unwrap_or(0) != 0;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("mode", test_block_mode_name(state.mode));
        nbt.insert("message", state.message.clone());
        nbt.insert("powered", i8::from(state.powered));
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `TestBlockEntity.getUpdateTag` returns `saveCustomOnly`.
        Some(self.save_custom_only())
    }
}

/// Vanilla `TestInstanceBlockEntity` state.
#[derive(Clone, Debug)]
pub struct TestInstanceData {
    /// Test function key; empty when no test is bound.
    pub test: String,
    /// Instance size.
    pub size: (i32, i32, i32),
    /// `Rotation` ordinal.
    pub rotation: i32,
    /// `TestInstanceBlockStatus` name.
    pub status: String,
    /// Error message, if the instance failed.
    pub error: Option<String>,
    /// Whether entities are ignored while running the test.
    pub ignore_entities: bool,
    /// Error markers reported by the last run, as `(position, text)`.
    pub error_markers: Vec<(BlockPos, String)>,
}

impl Default for TestInstanceData {
    fn default() -> Self {
        // Vanilla constructor: empty test, zero size, `NONE` rotation,
        // `ignore_entities = false`, `CLEARED` status, no error.
        Self {
            test: String::new(),
            size: (0, 0, 0),
            rotation: 0,
            status: "cleared".into(),
            error: None,
            ignore_entities: false,
            error_markers: Vec::new(),
        }
    }
}

/// `TestInstanceBlockEntity`.
pub struct TestInstanceBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<TestInstanceData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TestInstanceBlockEntity`.
unsafe impl DowncastType for TestInstanceBlockEntity {
    const TYPE_KEY: DowncastTypeKey =
        DowncastTypeKey::new("steel:block_entity/test_instance_block");
}

impl TestInstanceBlockEntity {
    /// Creates a test-instance entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::TEST_INSTANCE_BLOCK,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(TestInstanceData::default()),
        }
    }

    /// Vanilla `TestInstanceBlockEntity` completion state: the runner finished
    /// and did not report an error.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        let state = self.state.lock();
        state.status == "finished" && state.error.is_none()
    }

    /// Returns a snapshot of the instance configuration.
    #[must_use]
    pub fn data(&self) -> TestInstanceData {
        self.state.lock().clone()
    }

    /// Vanilla `TestInstanceBlockEntity.set`: replaces the whole data record and
    /// marks the block entity changed.
    pub fn set_data(
        &self,
        test: Option<&Identifier>,
        size: (i32, i32, i32),
        rotation: i32,
        ignore_entities: bool,
    ) {
        {
            let mut state = self.state.lock();
            state.test = test.map_or_else(String::new, ToString::to_string);
            state.size = size;
            state.rotation = rotation;
            state.ignore_entities = ignore_entities;
        }
        self.set_changed();
    }
}

impl BlockEntity for TestInstanceBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        // Vanilla `TestInstanceBlockEntity` stores everything under `data`
        // (`test`, `size`, `rotation`, `ignore_entities`, `status`,
        // `error_message`) plus a top-level `errors` list.
        if let Some(data) = view.compound("data") {
            state.test = data
                .string("test")
                .map_or_else(String::new, ToString::to_string);
            if let Some(size) = data.list("size").and_then(|list| list.ints())
                && size.len() == 3
            {
                state.size = (size[0], size[1], size[2]);
            }
            state.rotation = data
                .string("rotation")
                .map_or(0, |v| match v.to_str().as_ref() {
                    "clockwise_90" => 1,
                    "180" => 2,
                    "counterclockwise_90" => 3,
                    _ => 0,
                });
            state.ignore_entities = data.byte("ignore_entities").unwrap_or(0) != 0;
            state.status = data
                .string("status")
                .map_or_else(|| "cleared".into(), ToString::to_string);
            state.error = data.string("error_message").map(ToString::to_string);
        }
        state.error_markers.clear();
        if let Some(errors) = view.list("errors").and_then(|list| list.compounds()) {
            for marker in errors {
                let Some(pos) = marker.list("pos").and_then(|list| list.ints()) else {
                    continue;
                };
                if pos.len() != 3 {
                    continue;
                }
                state.error_markers.push((
                    BlockPos::new(pos[0], pos[1], pos[2]),
                    marker
                        .string("text")
                        .map_or_else(String::new, ToString::to_string),
                ));
            }
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        let mut data = NbtCompound::new();
        if !state.test.is_empty() {
            data.insert("test", state.test.clone());
        }
        data.insert(
            "size",
            NbtList::Int(vec![state.size.0, state.size.1, state.size.2]),
        );
        data.insert(
            "rotation",
            match state.rotation {
                1 => "clockwise_90",
                2 => "180",
                3 => "counterclockwise_90",
                _ => "none",
            },
        );
        data.insert("ignore_entities", i8::from(state.ignore_entities));
        data.insert("status", state.status.clone());
        if let Some(error) = &state.error {
            data.insert("error_message", error.clone());
        }
        nbt.insert("data", data);

        if !state.error_markers.is_empty() {
            let markers = state
                .error_markers
                .iter()
                .map(|(pos, text)| {
                    let mut marker = NbtCompound::new();
                    marker.insert("pos", NbtList::Int(vec![pos.x(), pos.y(), pos.z()]));
                    marker.insert("text", text.clone());
                    marker
                })
                .collect();
            nbt.insert("errors", NbtList::Compound(markers));
        }
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `TestInstanceBlockEntity.getUpdateTag` returns `saveCustomOnly`.
        Some(self.save_custom_only())
    }

    fn set_changed(&self) {
        // Vanilla `TestInstanceBlockEntity.setChanged` also re-sends the block entity.
        self.base.set_changed();
        let Some(world) = self.get_level() else {
            return;
        };
        world.send_block_updated(self.get_block_pos());
    }

    fn tick(&self, _world: &Arc<World>) {
        // TODO(GameTest): run the bound test instance through Steel's game-test harness when that
        // runtime exists; vanilla `TestInstanceBlockEntity.tick` drives the runner state machine.
    }
}
