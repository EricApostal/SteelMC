//! Clientbound place ghost recipe packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_PLACE_GHOST_RECIPE;

use super::recipe_display::RecipeDisplay;

/// Shows a ghost (preview) of a recipe inside an open container.
///
/// Matches vanilla's `ClientboundPlaceGhostRecipePacket`: the container id as a
/// `VarInt` followed by the recipe display.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_PLACE_GHOST_RECIPE)]
pub struct CPlaceGhostRecipe {
    /// Id of the open container the ghost items are placed into.
    #[write(as = VarInt)]
    pub container_id: i32,
    /// The recipe display to preview.
    pub recipe_display: RecipeDisplay,
}
