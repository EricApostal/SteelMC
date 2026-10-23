//! Clientbound custom chat completions packet - updates the client's chat completion entries.

use std::io::{Result, Write};

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_CUSTOM_CHAT_COMPLETIONS;
use steel_utils::{
    codec::VarInt,
    serial::{PrefixedWrite, WriteTo},
};

/// How the client should apply the supplied chat completion entries.
///
/// Vanilla ids come from `ByIdMap` (`ADD` 0, `REMOVE` 1, `SET` 2), written as a `VarInt`.
#[derive(WriteTo, Clone, Copy, Debug, PartialEq, Eq)]
#[write(as = VarInt)]
pub enum CustomChatCompletionsAction {
    /// Add the entries to the completion list.
    Add = 0,
    /// Remove the entries from the completion list.
    Remove = 1,
    /// Replace the completion list with the entries.
    Set = 2,
}

/// Sent to add, remove or replace the client's custom chat completion entries.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_CUSTOM_CHAT_COMPLETIONS)]
pub struct CCustomChatCompletions {
    /// What to do with `entries`.
    pub action: CustomChatCompletionsAction,
    /// The completion strings; written as a `VarInt` count of `VarInt`-length UTF-8 strings.
    pub entries: Vec<String>,
}

impl CCustomChatCompletions {
    /// Creates a new custom chat completions packet.
    #[must_use]
    pub const fn new(action: CustomChatCompletionsAction, entries: Vec<String>) -> Self {
        Self { action, entries }
    }
}

impl WriteTo for CCustomChatCompletions {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.action.write(writer)?;
        VarInt(self.entries.len() as i32).write(writer)?;
        for entry in &self.entries {
            entry.write_prefixed::<VarInt>(writer)?;
        }
        Ok(())
    }
}
