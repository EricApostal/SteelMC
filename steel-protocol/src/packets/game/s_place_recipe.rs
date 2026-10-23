//! Serverbound place recipe packet.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_PLACE_RECIPE;

use super::recipe_display::RecipeDisplayId;

/// Sent when the player clicks a recipe display in the recipe book.
///
/// Matches vanilla's `ServerboundPlaceRecipePacket`: the container id, the display
/// id and the use-max-items flag. All three fields have `ReadFrom` impls, so the
/// derive handles them.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_PLACE_RECIPE)]
pub struct SPlaceRecipe {
    /// Id of the open container the recipe should be placed into.
    #[read(as = VarInt)]
    pub container_id: i32,
    /// The recipe display that was clicked.
    pub recipe: RecipeDisplayId,
    /// Whether the recipe should be crafted as many times as possible.
    pub use_max_items: bool,
}
