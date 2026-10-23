//! Serverbound packet for saving a structure block's settings.

use std::io::Cursor;

use glam::IVec3;
use steel_macros::{ReadFrom, ServerPacket};
use steel_registry::blocks::properties::StructureMode;
#[expect(unused_imports)]
use steel_registry::packets::play::S_SET_STRUCTURE_BLOCK;
use steel_utils::{
    BlockPos, Rotation,
    codec::{VarInt, VarLong},
    serial::prefixed_read::read_utf,
};

/// Maximum UTF-16 code units for the structure name (vanilla `readUtf()`).
pub const MAX_STRUCTURE_NAME_LENGTH: usize = 32_767;
/// Maximum UTF-16 code units for the structure metadata (vanilla `readUtf(128)`).
pub const MAX_STRUCTURE_DATA_LENGTH: usize = 128;
/// Absolute limit applied to each offset axis (vanilla `maxOffset = 48`).
pub const MAX_STRUCTURE_OFFSET: i8 = 48;
/// Upper limit applied to each size axis (vanilla `maxSize = 48`).
pub const MAX_STRUCTURE_SIZE: i8 = 48;

/// What the structure block should do when the settings are applied.
///
/// Mirrors vanilla's `StructureBlockEntity.UpdateType`, written with
/// `FriendlyByteBuf.writeEnum` as the enum ordinal (`VarInt`).
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq)]
#[read(as = VarInt)]
pub enum StructureBlockUpdateType {
    /// Only update the stored metadata.
    UpdateData = 0,
    /// Update the metadata and save the selected area.
    SaveArea = 1,
    /// Update the metadata and load the saved area.
    LoadArea = 2,
    /// Update the metadata and scan the selected area for data.
    ScanArea = 3,
}

/// Structure block mirroring mode.
///
/// Mirrors vanilla's `world.level.block.Mirror`, written with
/// `FriendlyByteBuf.writeEnum` as the enum ordinal (`VarInt`).
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq)]
#[read(as = VarInt)]
pub enum Mirror {
    /// No mirroring.
    None = 0,
    /// Mirrored across the Z axis (left/right).
    LeftRight = 1,
    /// Mirrored across the X axis (front/back).
    FrontBack = 2,
}

/// Sent when a player saves a structure block from its edit screen.
///
/// Matches vanilla's `ServerboundSetStructureBlockPacket`: the block position,
/// the update type and mode as `VarInt` enum ordinals, the structure name, the
/// offset as three bytes (clamped to `[-48, 48]`), the size as three bytes
/// (clamped to `[0, 48]`), the mirror and rotation as `VarInt` ids, the
/// metadata string (max 128 UTF-16 code units), the integrity float (clamped
/// to `[0.0, 1.0]`), the `VarLong` seed, then one flags byte packing
/// `ignoreEntities` (bit 0), `showAir` (bit 1), `showBoundingBox` (bit 2) and
/// `strict` (bit 3).
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_SET_STRUCTURE_BLOCK)]
pub struct SSetStructureBlock {
    /// Position of the edited structure block.
    pub pos: BlockPos,
    /// What the structure block should do with these settings.
    pub update_type: StructureBlockUpdateType,
    /// The structure block's mode.
    pub mode: StructureMode,
    /// Name of the structure to save or load.
    pub name: String,
    /// Offset from the structure block to the structure's corner.
    pub offset: BlockPos,
    /// Size of the selected structure area.
    pub size: IVec3,
    /// How the structure should be mirrored.
    pub mirror: Mirror,
    /// How the structure should be rotated.
    pub rotation: Rotation,
    /// Mode-specific metadata (`StructureMode::Data` payload).
    pub data: String,
    /// Whether entities inside the structure are ignored.
    pub ignore_entities: bool,
    /// Whether the placement is strict about block states.
    pub strict: bool,
    /// Whether air blocks are shown in the bounding box.
    pub show_air: bool,
    /// Whether the bounding box is rendered.
    pub show_bounding_box: bool,
    /// Integrity (chance for each block to be placed), clamped to `[0.0, 1.0]`.
    pub integrity: f32,
    /// Seed for the integrity random.
    pub seed: i64,
}

impl SSetStructureBlock {
    /// Flags bit for `ignore_entities` (vanilla `FLAG_IGNORE_ENTITIES`).
    pub const FLAG_IGNORE_ENTITIES: i8 = 1;
    /// Flags bit for `show_air` (vanilla `FLAG_SHOW_AIR`).
    pub const FLAG_SHOW_AIR: i8 = 2;
    /// Flags bit for `show_bounding_box` (vanilla `FLAG_SHOW_BOUNDING_BOX`).
    pub const FLAG_SHOW_BOUNDING_BOX: i8 = 4;
    /// Flags bit for `strict` (vanilla `FLAG_STRICT`).
    pub const FLAG_STRICT: i8 = 8;
}

impl steel_utils::serial::ReadFrom for SSetStructureBlock {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let pos = BlockPos::read(data)?;
        let update_type = StructureBlockUpdateType::read(data)?;
        // Vanilla `readEnum(StructureMode.class)`: ordinal order of the Java enum.
        let mode = match VarInt::read(data)?.0 {
            0 => StructureMode::Save,
            1 => StructureMode::Load,
            2 => StructureMode::Corner,
            3 => StructureMode::Data,
            _ => {
                return Err(std::io::Error::other("Invalid StructureMode enum ordinal"));
            }
        };
        let name = read_utf(data, MAX_STRUCTURE_NAME_LENGTH)?;

        let offset = BlockPos::new(
            i32::from(i8::read(data)?.clamp(-MAX_STRUCTURE_OFFSET, MAX_STRUCTURE_OFFSET)),
            i32::from(i8::read(data)?.clamp(-MAX_STRUCTURE_OFFSET, MAX_STRUCTURE_OFFSET)),
            i32::from(i8::read(data)?.clamp(-MAX_STRUCTURE_OFFSET, MAX_STRUCTURE_OFFSET)),
        );
        let size = IVec3::new(
            i32::from(i8::read(data)?.clamp(0, MAX_STRUCTURE_SIZE)),
            i32::from(i8::read(data)?.clamp(0, MAX_STRUCTURE_SIZE)),
            i32::from(i8::read(data)?.clamp(0, MAX_STRUCTURE_SIZE)),
        );
        let mirror = Mirror::read(data)?;
        // Vanilla `Rotation.STREAM_CODEC` uses `ByIdMap` with `OutOfBoundsStrategy.WRAP`.
        let rotation = match VarInt::read(data)?.0.rem_euclid(4) {
            0 => Rotation::None,
            1 => Rotation::Clockwise90,
            2 => Rotation::Clockwise180,
            _ => Rotation::CounterClockwise90,
        };
        let data_str = read_utf(data, MAX_STRUCTURE_DATA_LENGTH)?;
        let integrity = f32::read(data)?.clamp(0.0, 1.0);
        let seed = VarLong::read(data)?.0;
        let flags = i8::read(data)?;

        Ok(Self {
            pos,
            update_type,
            mode,
            name,
            offset,
            size,
            mirror,
            rotation,
            data: data_str,
            ignore_entities: flags & Self::FLAG_IGNORE_ENTITIES != 0,
            strict: flags & Self::FLAG_STRICT != 0,
            show_air: flags & Self::FLAG_SHOW_AIR != 0,
            show_bounding_box: flags & Self::FLAG_SHOW_BOUNDING_BOX != 0,
            integrity,
            seed,
        })
    }
}
