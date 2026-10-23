//! Serverbound recipe book seen recipe packet.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_RECIPE_BOOK_SEEN_RECIPE;

use super::recipe_display::RecipeDisplayId;

/// Sent when the player dismisses the notification of a newly unlocked recipe.
///
/// Matches vanilla's `ServerboundRecipeBookSeenRecipePacket`: a single `VarInt`
/// recipe display id.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_RECIPE_BOOK_SEEN_RECIPE)]
pub struct SRecipeBookSeenRecipe {
    /// The display whose notification was dismissed.
    pub recipe: RecipeDisplayId,
}
