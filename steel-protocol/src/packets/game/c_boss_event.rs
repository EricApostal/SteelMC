//! Clientbound boss event packet - boss bar add/update/remove operations.

use std::io::{Result, Write};

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_BOSS_EVENT;
use steel_utils::{codec::VarInt, serial::WriteTo};
use text_components::TextComponent;
use uuid::Uuid;

/// Color of a boss bar, in vanilla `BossEvent.BossBarColor` declaration order.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum BossBarColor {
    /// Pink bar (`minecraft:red` text formatting).
    Pink = 0,
    /// Blue bar.
    Blue = 1,
    /// Red bar.
    Red = 2,
    /// Green bar.
    Green = 3,
    /// Yellow bar.
    Yellow = 4,
    /// Purple bar.
    Purple = 5,
    /// White bar.
    White = 6,
}

/// Notch overlay of a boss bar, in vanilla `BossEvent.BossBarOverlay` declaration order.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum BossBarOverlay {
    /// Plain progress bar.
    Progress = 0,
    /// Bar split into 6 notches.
    Notched6 = 1,
    /// Bar split into 10 notches.
    Notched10 = 2,
    /// Bar split into 12 notches.
    Notched12 = 3,
    /// Bar split into 20 notches.
    Notched20 = 4,
}

/// Boss bar operation, mirroring vanilla's private `Operation` union.
///
/// Vanilla writes a `VarInt` discriminator (`OperationType` ordinal: `ADD` = 0,
/// `REMOVE` = 1, `UPDATE_PROGRESS` = 2, `UPDATE_NAME` = 3, `UPDATE_STYLE` = 4,
/// `UPDATE_PROPERTIES` = 5) followed by the per-operation payload.
#[derive(Clone, Debug)]
pub enum BossEventOperation {
    /// Adds a boss bar: name, progress, color, overlay, then a packed flags byte.
    Add {
        /// Title shown above the boss bar.
        name: TextComponent,
        /// Bar fill amount, 0.0..=1.0.
        progress: f32,
        /// Bar color.
        color: BossBarColor,
        /// Bar notch overlay.
        overlay: BossBarOverlay,
        /// Whether the sky darkens while the bar is visible.
        darken_screen: bool,
        /// Whether boss music plays while the bar is visible.
        play_music: bool,
        /// Whether world fog is created while the bar is visible.
        create_world_fog: bool,
    },
    /// Removes the boss bar with this packet's UUID. No payload.
    Remove,
    /// Updates the bar fill amount. Payload: `f32` big-endian.
    UpdateProgress {
        /// New bar fill amount, 0.0..=1.0.
        progress: f32,
    },
    /// Updates the bar title. Payload: trusted component NBT.
    UpdateName {
        /// New title shown above the boss bar.
        name: TextComponent,
    },
    /// Updates the bar color and overlay. Payload: two `VarInt` ordinals.
    UpdateStyle {
        /// New bar color.
        color: BossBarColor,
        /// New bar notch overlay.
        overlay: BossBarOverlay,
    },
    /// Updates the bar property flags. Payload: one packed flags byte.
    UpdateProperties {
        /// Whether the sky darkens while the bar is visible.
        darken_screen: bool,
        /// Whether boss music plays while the bar is visible.
        play_music: bool,
        /// Whether world fog is created while the bar is visible.
        create_world_fog: bool,
    },
}

/// Boss bar add/update/remove packet.
///
/// Wire order: boss UUID (two big-endian `i64`), `VarInt` operation
/// discriminator, then the per-operation payload of [`BossEventOperation`].
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_BOSS_EVENT)]
pub struct CBossEvent {
    /// UUID identifying the boss bar; vanilla field `id`.
    pub id: Uuid,
    /// Operation to apply to the boss bar.
    pub operation: BossEventOperation,
}

impl CBossEvent {
    /// Flag bit: darken the sky while the bar is visible.
    pub const FLAG_DARKEN: u8 = 1;
    /// Flag bit: play boss music while the bar is visible.
    pub const FLAG_MUSIC: u8 = 2;
    /// Flag bit: create world fog while the bar is visible.
    pub const FLAG_FOG: u8 = 4;

    /// Creates a packet from a raw operation.
    #[must_use]
    pub const fn new(id: Uuid, operation: BossEventOperation) -> Self {
        Self { id, operation }
    }

    /// Creates an add-boss-bar packet.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors vanilla ClientboundBossEventPacket.AddOperation, whose record carries these fields verbatim"
    )]
    pub const fn add(
        id: Uuid,
        name: TextComponent,
        progress: f32,
        color: BossBarColor,
        overlay: BossBarOverlay,
        darken_screen: bool,
        play_music: bool,
        create_world_fog: bool,
    ) -> Self {
        Self::new(
            id,
            BossEventOperation::Add {
                name,
                progress,
                color,
                overlay,
                darken_screen,
                play_music,
                create_world_fog,
            },
        )
    }

    /// Creates a remove-boss-bar packet.
    #[must_use]
    pub const fn remove(id: Uuid) -> Self {
        Self::new(id, BossEventOperation::Remove)
    }

    /// Creates an update-progress packet.
    #[must_use]
    pub const fn update_progress(id: Uuid, progress: f32) -> Self {
        Self::new(id, BossEventOperation::UpdateProgress { progress })
    }

    /// Creates an update-name packet.
    #[must_use]
    pub const fn update_name(id: Uuid, name: TextComponent) -> Self {
        Self::new(id, BossEventOperation::UpdateName { name })
    }

    /// Creates an update-style packet.
    #[must_use]
    pub const fn update_style(id: Uuid, color: BossBarColor, overlay: BossBarOverlay) -> Self {
        Self::new(id, BossEventOperation::UpdateStyle { color, overlay })
    }

    /// Creates an update-properties packet.
    #[must_use]
    pub const fn update_properties(
        id: Uuid,
        darken_screen: bool,
        play_music: bool,
        create_world_fog: bool,
    ) -> Self {
        Self::new(
            id,
            BossEventOperation::UpdateProperties {
                darken_screen,
                play_music,
                create_world_fog,
            },
        )
    }
}

/// Packs the three boss bar property flags into one byte, mirroring vanilla's
/// private `encodeProperties`: bit 0 darken, bit 1 music, bit 2 fog.
const fn encode_properties(darken_screen: bool, play_music: bool, create_world_fog: bool) -> u8 {
    let mut flags = 0u8;
    if darken_screen {
        flags |= CBossEvent::FLAG_DARKEN;
    }
    if play_music {
        flags |= CBossEvent::FLAG_MUSIC;
    }
    if create_world_fog {
        flags |= CBossEvent::FLAG_FOG;
    }
    flags
}

impl WriteTo for CBossEvent {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.id.write(writer)?;
        match &self.operation {
            BossEventOperation::Add {
                name,
                progress,
                color,
                overlay,
                darken_screen,
                play_music,
                create_world_fog,
            } => {
                VarInt(0).write(writer)?;
                name.write(writer)?;
                progress.write(writer)?;
                color.write(writer)?;
                overlay.write(writer)?;
                encode_properties(*darken_screen, *play_music, *create_world_fog).write(writer)?;
            }
            BossEventOperation::Remove => {
                VarInt(1).write(writer)?;
            }
            BossEventOperation::UpdateProgress { progress } => {
                VarInt(2).write(writer)?;
                progress.write(writer)?;
            }
            BossEventOperation::UpdateName { name } => {
                VarInt(3).write(writer)?;
                name.write(writer)?;
            }
            BossEventOperation::UpdateStyle { color, overlay } => {
                VarInt(4).write(writer)?;
                color.write(writer)?;
                overlay.write(writer)?;
            }
            BossEventOperation::UpdateProperties {
                darken_screen,
                play_music,
                create_world_fog,
            } => {
                VarInt(5).write(writer)?;
                encode_properties(*darken_screen, *play_music, *create_world_fog).write(writer)?;
            }
        }
        Ok(())
    }
}
