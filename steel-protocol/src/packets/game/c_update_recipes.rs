//! Clientbound update recipes packet.

use std::io::{Error, Result, Write};

use steel_macros::ClientPacket;
use steel_registry::packets::play::C_UPDATE_RECIPES;
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

use super::recipe_display::{ItemHolderSet, SlotDisplay};

/// One entry of vanilla's `itemSets` map: a `recipe_property_set` key and the
/// items that satisfy it.
///
/// Vanilla encodes the map as `VarInt` count followed by key/value pairs, where
/// the key is a plain `Identifier` and the value is `Item.STREAM_CODEC`'s
/// `VarInt`-prefixed list of item registry ids. A `Vec` of entries is used here
/// instead of a `HashMap` so the wire order stays deterministic.
///
/// `Item` does not implement a holder `WriteTo`, so the ids are resolved with
/// [`RegistryEntry`](steel_registry::RegistryEntry) directly.
#[derive(Clone, Debug)]
pub struct RecipePropertySetEntry {
    /// Resource location of the property set (e.g. `minecraft:furnace_input`).
    pub key: Identifier,
    /// Items accepted by the property set.
    pub items: Vec<steel_registry::items::ItemRef>,
}

impl WriteTo for RecipePropertySetEntry {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        use steel_registry::RegistryEntry as _;
        self.key.write(writer)?;
        let count = i32::try_from(self.items.len())
            .map_err(|_| Error::other("recipe property set has too many items"))?;
        VarInt(count).write(writer)?;
        for item in &self.items {
            VarInt(item.id() as i32).write(writer)?;
        }
        Ok(())
    }
}

/// One stonecutter entry of vanilla's `SelectableRecipe.SingleInputSet`.
///
/// `SingleInputSet.noRecipeCodec()` drops the selectable recipe's `Optional<RecipeHolder>`,
/// so only the input ingredient (an item `holderSet`) and the option display are sent.
#[derive(Clone, Debug)]
pub struct SelectableStonecutterRecipe {
    /// Ingredient accepted by the stonecutter (vanilla `Ingredient.CONTENTS_STREAM_CODEC`).
    pub input: ItemHolderSet,
    /// Display of the stonecutting option.
    pub option_display: SlotDisplay,
}

impl WriteTo for SelectableStonecutterRecipe {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.input.write(writer)?;
        self.option_display.write(writer)
    }
}

/// Synchronizes the recipe property sets and stonecutter options.
///
/// Matches vanilla's `ClientboundUpdateRecipesPacket`: the `itemSets` map first,
/// then the stonecutter `SingleInputSet`.
#[derive(ClientPacket, Clone, Debug)]
#[packet_id(Play = C_UPDATE_RECIPES)]
pub struct CUpdateRecipes {
    /// Item sets keyed by their `recipe_property_set` resource location.
    pub item_sets: Vec<RecipePropertySetEntry>,
    /// Stonecutter selectable recipes.
    pub stonecutter_recipes: Vec<SelectableStonecutterRecipe>,
}

impl WriteTo for CUpdateRecipes {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.item_sets.write(writer)?;
        self.stonecutter_recipes.write(writer)
    }
}
