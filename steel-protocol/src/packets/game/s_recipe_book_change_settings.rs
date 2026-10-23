//! Serverbound recipe book change settings packet.

use steel_macros::{ReadFrom, ServerPacket};
#[expect(unused_imports)]
use steel_registry::packets::play::S_RECIPE_BOOK_CHANGE_SETTINGS;

use super::c_recipe_book_settings::RecipeBookType;

/// Sent when the player opens/closes or (un)filters one of the recipe books.
///
/// Matches vanilla's `ServerboundRecipeBookChangeSettingsPacket`, whose custom
/// codec writes the book type as a `VarInt` ordinal followed by the two flags.
#[derive(ServerPacket, ReadFrom, Clone, Debug)]
#[packet_id(Play = S_RECIPE_BOOK_CHANGE_SETTINGS)]
pub struct SRecipeBookChangeSettings {
    /// Which of the four books changed.
    pub book_type: RecipeBookType,
    /// Whether the book's GUI is open.
    pub is_open: bool,
    /// Whether the book only shows craftable recipes.
    pub is_filtering: bool,
}
