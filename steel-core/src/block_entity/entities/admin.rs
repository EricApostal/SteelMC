//! `StructureBlockEntity`, `JigsawBlockEntity`, `TestBlockEntity` and
//! `TestInstanceBlockEntity`.
//!
//! These are the admin-only block entities: they persist their editor configuration so a server
//! restart keeps the setup, and the block behaviors drive them from redstone and use-interaction.

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

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
    /// Whether this block is owned by the jigsaw system.
    pub used_by_jigsaw: bool,
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

    /// Vanilla `StructureBlockEntity.trigger` on a redstone rising edge.
    ///
    /// Only the redstone bookkeeping is a block-entity concern; the actual structure save/load
    /// requires the structure template manager.
    pub fn trigger(&self, _world: &std::sync::Arc<World>, _pos: BlockPos) {
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
        state.name = view
            .string("name")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.author = view
            .string("author")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.metadata = view
            .string("metadata")
            .map(|v| v.to_string())
            .unwrap_or_default();
        if let Some(pos) = view.compound("pos") {
            state.pos = (
                pos.int("X").unwrap_or(0),
                pos.int("Y").unwrap_or(0),
                pos.int("Z").unwrap_or(0),
            );
        }
        if let Some(size) = view.compound("size") {
            state.size = (
                size.int("X").unwrap_or(1),
                size.int("Y").unwrap_or(1),
                size.int("Z").unwrap_or(1),
            );
        }
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
            .map(|v| v.to_string())
            .unwrap_or_else(|| "LOAD".into());
        state.powered = view.byte("power").unwrap_or(1) != 0;
        state.show_bounding_box = view.byte("showboundingbox").unwrap_or(1) != 0;
        state.show_air = view.byte("showair").unwrap_or(0) != 0;
        state.integrity = view.float("integrity").unwrap_or(1.0);
        state.seed = view.long("seed").unwrap_or(0);
        state.used_by_jigsaw = view.byte("strict").unwrap_or(0) != 0;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("name", state.name.clone());
        nbt.insert("author", state.author.clone());
        nbt.insert("metadata", state.metadata.clone());

        let mut pos = NbtCompound::new();
        pos.insert("X", state.pos.0);
        pos.insert("Y", state.pos.1);
        pos.insert("Z", state.pos.2);
        nbt.insert("pos", pos);

        let mut size = NbtCompound::new();
        size.insert("X", state.size.0);
        size.insert("Y", state.size.1);
        size.insert("Z", state.size.2);
        nbt.insert("size", size);

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
        nbt.insert("power", i8::from(state.powered));
        nbt.insert("showboundingbox", i8::from(state.show_bounding_box));
        nbt.insert("showair", i8::from(state.show_air));
        nbt.insert("integrity", state.integrity);
        nbt.insert("seed", state.seed);
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
}

impl BlockEntity for JigsawBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.name = view
            .string("name")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.target = view
            .string("target")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.pool = view
            .string("pool")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.final_state = view
            .string("final_state")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.joint = view
            .string("joint")
            .map(|v| v.to_string())
            .unwrap_or_default();
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
}

// ---------------------------------------------------------------------------
// TestBlockEntity / TestInstanceBlockEntity
// ---------------------------------------------------------------------------

/// Vanilla `TestBlockEntity` state.
#[derive(Clone, Debug, Default)]
pub struct TestBlockData {
    /// Test function name.
    pub test: String,
    /// Optional message shown in the block.
    pub message: String,
    /// Test status name.
    pub status: String,
    /// Whether the block is powered/triggered.
    pub triggered: bool,
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
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::TEST_BLOCK, level, pos, state),
            state: SyncMutex::new(TestBlockData::default()),
        }
    }

    /// Returns a snapshot of the test configuration.
    #[must_use]
    pub fn data(&self) -> TestBlockData {
        self.state.lock().clone()
    }
}

impl BlockEntity for TestBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.test = view
            .string("test")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.message = view
            .string("message")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.status = view
            .string("status")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.triggered = view.byte("triggered").unwrap_or(0) != 0;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("test", state.test.clone());
        nbt.insert("message", state.message.clone());
        nbt.insert("status", state.status.clone());
        nbt.insert("triggered", i8::from(state.triggered));
    }
}

/// Vanilla `TestInstanceBlockEntity` state.
#[derive(Clone, Debug, Default)]
pub struct TestInstanceData {
    /// Test function name.
    pub test: String,
    /// Instance size.
    pub size: (i32, i32, i32),
    /// `Rotation` ordinal.
    pub rotation: i32,
    /// `TestInstanceBlockStatus` name.
    pub status: String,
    /// Error message, if the instance failed.
    pub error: Option<String>,
    /// Whether the instance is ignored by the runner.
    pub ignored: bool,
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
            state: SyncMutex::new(TestInstanceData {
                size: (1, 1, 1),
                status: "finished".into(),
                ..TestInstanceData::default()
            }),
        }
    }

    /// Vanilla `TestInstanceBlockEntity` signal: 15 while the test finished successfully.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        let state = self.state.lock();
        state.error.is_none() && state.status != "running"
    }

    /// Returns a snapshot of the instance configuration.
    #[must_use]
    pub fn data(&self) -> TestInstanceData {
        self.state.lock().clone()
    }
}

impl BlockEntity for TestInstanceBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.test = view
            .string("test")
            .map(|v| v.to_string())
            .unwrap_or_default();
        if let Some(size) = view.compound("size") {
            state.size = (
                size.int("X").unwrap_or(1),
                size.int("Y").unwrap_or(1),
                size.int("Z").unwrap_or(1),
            );
        }
        state.rotation = view
            .string("rotation")
            .map_or(0, |v| match v.to_str().as_ref() {
                "CLOCKWISE_90" => 1,
                "CLOCKWISE_180" => 2,
                "COUNTERCLOCKWISE_90" => 3,
                _ => 0,
            });
        state.status = view
            .string("status")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.error = view.string("error").map(|v| v.to_string());
        state.ignored = view.byte("ignored").unwrap_or(0) != 0;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("test", state.test.clone());

        let mut size = NbtCompound::new();
        size.insert("X", state.size.0);
        size.insert("Y", state.size.1);
        size.insert("Z", state.size.2);
        nbt.insert("size", size);

        nbt.insert(
            "rotation",
            match state.rotation {
                1 => "CLOCKWISE_90",
                2 => "CLOCKWISE_180",
                3 => "COUNTERCLOCKWISE_90",
                _ => "NONE",
            },
        );
        nbt.insert("status", state.status.clone());
        if let Some(error) = &state.error {
            nbt.insert("error", error.clone());
        }
        nbt.insert("ignored", i8::from(state.ignored));
    }

    fn tick(&self, _world: &std::sync::Arc<World>) {
        // TODO(GameTest): run the bound test instance through Steel's game-test harness when that
        // runtime exists; vanilla `TestInstanceBlockEntity.tick` drives the runner state machine.
    }
}
