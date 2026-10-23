//! Clientbound recipe book add packet.

use steel_macros::{ClientPacket, WriteTo};
use steel_registry::packets::play::C_RECIPE_BOOK_ADD;

use super::recipe_display::RecipeDisplayEntry;

/// One entry of [`CRecipeBookAdd`].
///
/// Mirrors vanilla's `ClientboundRecipeBookAddPacket.Entry`: the display entry
/// followed by a single flag byte.
#[derive(WriteTo, Clone, Debug)]
pub struct RecipeBookAddEntry {
    /// The recipe display being added to the book.
    pub contents: RecipeDisplayEntry,
    /// Flag byte, see [`Self::FLAG_NOTIFICATION`] and [`Self::FLAG_HIGHLIGHT`].
    pub flags: i8,
}

impl RecipeBookAddEntry {
    /// The book should show a toast/notification for this recipe.
    pub const FLAG_NOTIFICATION: i8 = 1;
    /// The recipe should be highlighted in the book.
    pub const FLAG_HIGHLIGHT: i8 = 2;

    /// Creates an entry from the two vanilla flag bits.
    #[must_use]
    pub const fn new(contents: RecipeDisplayEntry, notification: bool, highlight: bool) -> Self {
        let flags = (if notification {
            Self::FLAG_NOTIFICATION
        } else {
            0
        }) | (if highlight { Self::FLAG_HIGHLIGHT } else { 0 });
        Self { contents, flags }
    }

    /// Returns whether the notification flag is set.
    #[must_use]
    pub const fn notification(&self) -> bool {
        self.flags & Self::FLAG_NOTIFICATION != 0
    }

    /// Returns whether the highlight flag is set.
    #[must_use]
    pub const fn highlight(&self) -> bool {
        self.flags & Self::FLAG_HIGHLIGHT != 0
    }
}

/// Adds recipe displays to the client's recipe book.
///
/// Matches vanilla's `ClientboundRecipeBookAddPacket`: a `VarInt`-prefixed list of
/// entries followed by the `replace` flag.
#[derive(ClientPacket, WriteTo, Clone, Debug)]
#[packet_id(Play = C_RECIPE_BOOK_ADD)]
pub struct CRecipeBookAdd {
    /// The displays to add.
    #[write(as = Prefixed(VarInt))]
    pub entries: Vec<RecipeBookAddEntry>,
    /// Whether the listed entries replace the whole book instead of appending.
    pub replace: bool,
}
