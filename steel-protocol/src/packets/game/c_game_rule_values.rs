//! Clientbound game rule values packet.

use std::io::{Result, Write};

use rustc_hash::FxHashMap;
use steel_macros::ClientPacket;
use steel_registry::packets::play::C_GAME_RULE_VALUES;
use steel_utils::{
    Identifier,
    codec::VarInt,
    serial::{PrefixedWrite, WriteTo},
};

/// Sends the current value of every available game rule to the client.
///
/// Mirrors vanilla's `ClientboundGameRuleValuesPacket`: the map's keys are the
/// game rule registry names (`ResourceKey.streamCodec` -> identifier string) and
/// the values are the rules' string representations.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_GAME_RULE_VALUES)]
pub struct CGameRuleValues {
    /// Current value of every game rule, keyed by the rule's identifier.
    pub values: FxHashMap<Identifier, String>,
}

impl WriteTo for CGameRuleValues {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.values.len() as i32).write(writer)?;
        for (key, value) in &self.values {
            key.write(writer)?;
            value.write_prefixed::<VarInt>(writer)?;
        }
        Ok(())
    }
}

impl CGameRuleValues {
    /// Creates a new game rule values packet.
    #[must_use]
    pub const fn new(values: FxHashMap<Identifier, String>) -> Self {
        Self { values }
    }
}
