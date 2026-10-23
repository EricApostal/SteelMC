//! Clientbound custom report details packet.

use std::io::{Error, Result, Write};

use rustc_hash::FxHashMap;
use steel_macros::ClientPacket;
use steel_registry::packets::config::C_CUSTOM_REPORT_DETAILS;
use steel_registry::packets::play::C_CUSTOM_REPORT_DETAILS as PLAY_C_CUSTOM_REPORT_DETAILS;
use steel_utils::codec::VarInt;
use steel_utils::serial::{PrefixedWrite, WriteTo};

/// Maximum key length accepted by the client (vanilla `MAX_DETAIL_KEY_LENGTH`).
const MAX_DETAIL_KEY_LENGTH: usize = 128;
/// Maximum value length accepted by the client (vanilla `MAX_DETAIL_VALUE_LENGTH`).
const MAX_DETAIL_VALUE_LENGTH: usize = 4096;
/// Maximum entry count accepted by the client (vanilla `MAX_DETAIL_COUNT`).
const MAX_DETAIL_COUNT: usize = 32;

/// Extra key/value pairs shown next to the "report" button on the client.
///
/// Mirrors vanilla's `ClientboundCustomReportDetailsPacket`, whose details are
/// encoded by `ByteBufCodecs.map(HashMap::new, stringUtf8(128),
/// stringUtf8(4096), 32)`: a VarInt entry count, then each key and value as a
/// VarInt-prefixed UTF-8 string (map iteration order is unspecified).
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(
    Config = C_CUSTOM_REPORT_DETAILS,
    Play = PLAY_C_CUSTOM_REPORT_DETAILS
)]
pub struct CCustomReportDetails {
    /// Report detail entries, keyed by the detail name.
    pub details: FxHashMap<String, String>,
}

impl WriteTo for CCustomReportDetails {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        if self.details.len() > MAX_DETAIL_COUNT {
            return Err(Error::other(format!(
                "{} details exceeded max size of: {}",
                self.details.len(),
                MAX_DETAIL_COUNT
            )));
        }

        VarInt(self.details.len() as i32).write(writer)?;
        for (key, value) in &self.details {
            key.write_prefixed_bound::<VarInt>(writer, MAX_DETAIL_KEY_LENGTH)?;
            value.write_prefixed_bound::<VarInt>(writer, MAX_DETAIL_VALUE_LENGTH)?;
        }
        Ok(())
    }
}

impl CCustomReportDetails {
    /// Creates a new custom report details packet.
    #[must_use]
    pub const fn new(details: FxHashMap<String, String>) -> Self {
        Self { details }
    }
}
