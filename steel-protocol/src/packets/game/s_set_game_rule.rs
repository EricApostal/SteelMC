//! Serverbound packet for changing game rules.

use std::io::Cursor;

use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::play::S_SET_GAME_RULE;
use steel_utils::{
    Identifier,
    codec::VarInt,
    serial::{PrefixedRead, ReadFrom, prefixed_read::read_utf},
};

/// Maximum UTF-16 code units for a game rule value string.
pub const MAX_GAME_RULE_VALUE_LENGTH: usize = 32_767;
/// Maximum number of entries in one packet (vanilla `ByteBufCodecs.list()`).
pub const MAX_GAME_RULE_ENTRIES: usize = 256;

/// A single game rule change: the rule's registry key plus its new value.
///
/// Matches vanilla's `ServerboundSetGameRulePacket.Entry`: a game-rule
/// registry key (a namespaced id on the wire) followed by a UTF-8 string
/// value (max 32767 UTF-16 code units).
#[derive(Clone, Debug)]
pub struct SSetGameRuleEntry {
    /// Registry key of the game rule (e.g. `minecraft:advance_time`).
    pub game_rule: Identifier,
    /// The new value, serialized as a string.
    pub value: String,
}

impl ReadFrom for SSetGameRuleEntry {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let game_rule = Identifier::read(data)?;
        let value = read_utf(data, MAX_GAME_RULE_VALUE_LENGTH)?;
        Ok(Self { game_rule, value })
    }
}

/// Sent when the player edits game rules from the settings screen.
///
/// Matches vanilla's `ServerboundSetGameRulePacket`: a `VarInt`-prefixed
/// list of [`SSetGameRuleEntry`]. The server must validate permissions
/// before applying the changes.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_SET_GAME_RULE)]
pub struct SSetGameRule {
    /// The game rule changes to apply.
    pub entries: Vec<SSetGameRuleEntry>,
}

impl ReadFrom for SSetGameRule {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let entries =
            Vec::<SSetGameRuleEntry>::read_prefixed_bound::<VarInt>(data, MAX_GAME_RULE_ENTRIES)?;
        Ok(Self { entries })
    }
}
