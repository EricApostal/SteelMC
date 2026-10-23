//! Clientbound recipe book remove packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_RECIPE_BOOK_REMOVE;

use super::recipe_display::RecipeDisplayId;

/// Removes recipe displays from the client's recipe book.
///
/// Matches vanilla's `ClientboundRecipeBookRemovePacket`: a `VarInt`-prefixed list
/// of display ids, each encoded as a `VarInt`.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_RECIPE_BOOK_REMOVE)]
pub struct CRecipeBookRemove {
    /// Ids of the displays to remove.
    #[write(as = Prefixed(VarInt))]
    pub recipes: Vec<RecipeDisplayId>,
}
