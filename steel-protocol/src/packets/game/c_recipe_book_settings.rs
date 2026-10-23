//! Clientbound recipe book settings packet.

use steel_macros::{ClientPacket, ReadFrom, WriteTo};
use steel_registry::packets::play::C_RECIPE_BOOK_SETTINGS;

/// Vanilla `net.minecraft.world.inventory.RecipeBookType`.
///
/// The client sends the type ordinal as a `VarInt` in
/// [`SRecipeBookChangeSettings`](super::s_recipe_book_change_settings::SRecipeBookChangeSettings);
/// the server keys [`RecipeBookSettings`] by this type.
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq)]
#[read(as = VarInt)]
pub enum RecipeBookType {
    /// The crafting table book (`CRAFTING`).
    Crafting = 0,
    /// The furnace book (`FURNACE`).
    Furnace = 1,
    /// The blast furnace book (`BLAST_FURNACE`).
    BlastFurnace = 2,
    /// The smoker book (`SMOKER`).
    Smoker = 3,
}

/// Vanilla `RecipeBookSettings.TypeSettings`: the open and filtering flags of one book.
#[derive(WriteTo, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecipeBookTypeSettings {
    /// Whether the book's GUI is open.
    pub open: bool,
    /// Whether the book only shows craftable recipes.
    pub filtering: bool,
}

/// Vanilla `RecipeBookSettings`: one [`RecipeBookTypeSettings`] per recipe book type.
///
/// Wire order is crafting, furnace, blast furnace, smoker, each two boolean bytes.
#[derive(WriteTo, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecipeBookSettings {
    /// Crafting table book settings.
    pub crafting: RecipeBookTypeSettings,
    /// Furnace book settings.
    pub furnace: RecipeBookTypeSettings,
    /// Blast furnace book settings.
    pub blast_furnace: RecipeBookTypeSettings,
    /// Smoker book settings.
    pub smoker: RecipeBookTypeSettings,
}

impl RecipeBookSettings {
    /// Settings of every book closed and unfiltered (vanilla `TypeSettings.DEFAULT`).
    pub const DEFAULT: Self = Self {
        crafting: RecipeBookTypeSettings {
            open: false,
            filtering: false,
        },
        furnace: RecipeBookTypeSettings {
            open: false,
            filtering: false,
        },
        blast_furnace: RecipeBookTypeSettings {
            open: false,
            filtering: false,
        },
        smoker: RecipeBookTypeSettings {
            open: false,
            filtering: false,
        },
    };

    /// Returns the settings of `book_type` (vanilla `RecipeBookSettings.getSettings`).
    #[must_use]
    pub const fn settings(&self, book_type: RecipeBookType) -> &RecipeBookTypeSettings {
        match book_type {
            RecipeBookType::Crafting => &self.crafting,
            RecipeBookType::Furnace => &self.furnace,
            RecipeBookType::BlastFurnace => &self.blast_furnace,
            RecipeBookType::Smoker => &self.smoker,
        }
    }

    /// Returns a copy with the settings of `book_type` replaced.
    #[must_use]
    pub const fn with_settings(
        mut self,
        book_type: RecipeBookType,
        settings: RecipeBookTypeSettings,
    ) -> Self {
        match book_type {
            RecipeBookType::Crafting => self.crafting = settings,
            RecipeBookType::Furnace => self.furnace = settings,
            RecipeBookType::BlastFurnace => self.blast_furnace = settings,
            RecipeBookType::Smoker => self.smoker = settings,
        }
        self
    }
}

/// Synchronises the player's recipe book settings.
///
/// Matches vanilla's `ClientboundRecipeBookSettingsPacket`.
#[derive(ClientPacket, WriteTo, Clone, Copy, Debug)]
#[packet_id(Play = C_RECIPE_BOOK_SETTINGS)]
pub struct CRecipeBookSettings {
    /// The settings of all four books.
    pub book_settings: RecipeBookSettings,
}
