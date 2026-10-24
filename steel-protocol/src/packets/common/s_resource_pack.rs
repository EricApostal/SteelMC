//! Serverbound resource pack packet - the client's answer to a resource-pack push.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::config::S_RESOURCE_PACK;
#[expect(unused_imports)]
use steel_registry::packets::play::S_RESOURCE_PACK as PLAY_S_RESOURCE_PACK;
use uuid::Uuid;

/// The action a client reports for a resource pack.
///
/// Discriminants mirror vanilla's `ServerboundResourcePackPacket.Action`
/// declaration order, which is the enum ordinal written as a `VarInt`.
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq)]
#[read(as = VarInt)]
pub enum ResourcePackAction {
    /// The pack was loaded successfully.
    SuccessfullyLoaded = 0,
    /// The user declined the pack.
    Declined = 1,
    /// Downloading the pack failed.
    FailedDownload = 2,
    /// The user accepted the pack and downloading started.
    Accepted = 3,
    /// The pack finished downloading.
    Downloaded = 4,
    /// The pack's URL was invalid.
    InvalidUrl = 5,
    /// Reloading the pack failed.
    FailedReload = 6,
    /// The pack was discarded.
    Discarded = 7,
}

impl ResourcePackAction {
    /// Whether this action ends the resource-pack exchange.
    ///
    /// Mirrors vanilla's `Action.isTerminal()`.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Accepted | Self::Downloaded)
    }
}

/// Sent by the client to report the outcome of a resource-pack push.
///
/// Equivalent to vanilla's `ServerboundResourcePackPacket`: the pack UUID
/// followed by the action ordinal as a `VarInt`.
#[derive(ReadFrom, ServerPacket, Clone, Debug)]
#[packet_id(Config = S_RESOURCE_PACK, Play = PLAY_S_RESOURCE_PACK)]
pub struct SResourcePack {
    /// The id of the resource pack this answer refers to.
    pub id: Uuid,
    /// The action the client performed.
    pub action: ResourcePackAction,
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use steel_utils::serial::ReadFrom as _;

    use super::*;

    #[test]
    fn resource_pack_reads_uuid_then_action_ordinal() {
        let id = Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff);
        let mut bytes = id.as_bytes().to_vec();
        bytes.push(ResourcePackAction::Declined as u8);

        let packet = SResourcePack::read(&mut Cursor::new(&bytes[..])).expect("should decode");

        assert_eq!(packet.id, id);
        assert_eq!(packet.action, ResourcePackAction::Declined);
    }

    #[test]
    fn only_accepted_and_downloaded_are_non_terminal() {
        assert!(!ResourcePackAction::Accepted.is_terminal());
        assert!(!ResourcePackAction::Downloaded.is_terminal());
        assert!(ResourcePackAction::SuccessfullyLoaded.is_terminal());
        assert!(ResourcePackAction::Declined.is_terminal());
        assert!(ResourcePackAction::FailedDownload.is_terminal());
        assert!(ResourcePackAction::InvalidUrl.is_terminal());
        assert!(ResourcePackAction::FailedReload.is_terminal());
        assert!(ResourcePackAction::Discarded.is_terminal());
    }
}
