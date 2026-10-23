//! Serverbound packet for running actions on a test instance block.

use std::io::Cursor;

use glam::IVec3;
use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::play::S_TEST_INSTANCE_BLOCK_ACTION;
use steel_utils::{BlockPos, Identifier, Rotation, codec::VarInt};
use text_components::TextComponent;

/// Action to perform on a test instance block.
///
/// Mirrors vanilla's `ServerboundTestInstanceBlockActionPacket.Action`, whose
/// `STREAM_CODEC` is `ByteBufCodecs.idMapper(BY_ID, ...)` with explicit
/// ids and `OutOfBoundsStrategy.ZERO` (unknown ids fall back to `Init`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestInstanceBlockAction {
    /// Place the structure for the selected test.
    Init = 0,
    /// Query the stored test data.
    Query = 1,
    /// Store new test data in the block.
    Set = 2,
    /// Reset the test area.
    Reset = 3,
    /// Save the structure to disk.
    Save = 4,
    /// Export the test structure.
    Export = 5,
    /// Run the test.
    Run = 6,
}

impl steel_utils::serial::ReadFrom for TestInstanceBlockAction {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        // Vanilla `ByIdMap.continuous(..., OutOfBoundsStrategy.ZERO)`.
        Ok(match VarInt::read(data)?.0 {
            1 => Self::Query,
            2 => Self::Set,
            3 => Self::Reset,
            4 => Self::Save,
            5 => Self::Export,
            6 => Self::Run,
            _ => Self::Init,
        })
    }
}

/// Stored test configuration of a test instance block.
///
/// Mirrors vanilla's `TestInstanceBlockEntity.Data`: the optional test key
/// (`ResourceKey<GameTestInstance>` on the wire as a nullable namespaced
/// id), the structure size (three `VarInt`s via `Vec3i.STREAM_CODEC`), the
/// rotation (`VarInt` id), the `ignoreEntities` boolean, the status
/// (`VarInt` id), and the optional error message (bool-prefixed NBT
/// component).
#[derive(Clone, Debug)]
pub struct TestInstanceBlockData {
    /// Namespaced id of the game test to run, if one is selected.
    pub test: Option<Identifier>,
    /// Size of the test area.
    pub size: IVec3,
    /// Rotation of the test area.
    pub rotation: Rotation,
    /// Whether entities in the test area are ignored.
    pub ignore_entities: bool,
    /// Current test status (see [`TestInstanceBlockStatus`]).
    pub status: TestInstanceBlockStatus,
    /// Error message shown when the test finished with an error.
    pub error_message: Option<TextComponent>,
}

/// Running state of a test instance block.
///
/// Mirrors vanilla's `TestInstanceBlockEntity.Status` ids (`CLEARED = 0`,
/// `RUNNING = 1`, `FINISHED = 2`), written with `ByteBufCodecs.idMapper`
/// (`OutOfBoundsStrategy.ZERO`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestInstanceBlockStatus {
    /// No run in progress and no failure recorded.
    Cleared,
    /// A test run is in progress.
    Running,
    /// The test run has finished.
    Finished,
}

impl steel_utils::serial::ReadFrom for TestInstanceBlockData {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        // Vanilla `ByteBufCodecs.optional(ResourceKey.streamCodec(TEST_INSTANCE))`:
        // presence boolean, then the namespaced id when present.
        let test = if bool::read(data)? {
            Some(Identifier::read(data)?)
        } else {
            None
        };
        let size = IVec3 {
            x: VarInt::read(data)?.0,
            y: VarInt::read(data)?.0,
            z: VarInt::read(data)?.0,
        };
        // Vanilla `Rotation.STREAM_CODEC` uses `ByIdMap` with `OutOfBoundsStrategy.WRAP`.
        let rotation = match VarInt::read(data)?.0.rem_euclid(4) {
            0 => Rotation::None,
            1 => Rotation::Clockwise90,
            2 => Rotation::Clockwise180,
            _ => Rotation::CounterClockwise90,
        };
        let ignore_entities = bool::read(data)?;
        // Vanilla `OutOfBoundsStrategy.ZERO`: unknown ids fall back to `CLEARED`.
        let status = match VarInt::read(data)?.0 {
            1 => TestInstanceBlockStatus::Running,
            2 => TestInstanceBlockStatus::Finished,
            _ => TestInstanceBlockStatus::Cleared,
        };
        let error_message = Option::<TextComponent>::read(data)?;

        Ok(Self {
            test,
            size,
            rotation,
            ignore_entities,
            status,
            error_message,
        })
    }
}

/// Sent when a player presses an action button in a test instance block's screen.
///
/// Matches vanilla's `ServerboundTestInstanceBlockActionPacket`: the block
/// position, the action as a `VarInt` id, then the embedded
/// [`TestInstanceBlockData`].
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_TEST_INSTANCE_BLOCK_ACTION)]
pub struct STestInstanceBlockAction {
    /// Position of the test instance block.
    pub pos: BlockPos,
    /// The action to perform.
    pub action: TestInstanceBlockAction,
    /// The test data carried with the request.
    pub data: TestInstanceBlockData,
}

impl steel_utils::serial::ReadFrom for STestInstanceBlockAction {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let pos = BlockPos::read(data)?;
        let action = TestInstanceBlockAction::read(data)?;
        let inner = TestInstanceBlockData::read(data)?;
        Ok(Self {
            pos,
            action,
            data: inner,
        })
    }
}
