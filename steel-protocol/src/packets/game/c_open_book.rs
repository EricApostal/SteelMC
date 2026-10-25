//! Clientbound open book packet - tells the client to open the book in a hand.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_OPEN_BOOK;
use steel_utils::types::InteractionHand;

/// Opens the written book held in the given hand.
///
/// Vanilla's `ClientboundOpenBookPacket` carries an `InteractionHand`, whose
/// stream codec is a `VarInt` id mapper.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_OPEN_BOOK)]
pub struct COpenBook {
    /// The hand holding the book to open.
    pub hand: InteractionHand,
}

impl COpenBook {
    /// Creates a new open book packet.
    #[must_use]
    pub const fn new(hand: InteractionHand) -> Self {
        Self { hand }
    }
}
