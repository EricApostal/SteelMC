//! Serverbound packet for editing a writable book.

use std::io::{Cursor, Error, ErrorKind};

use steel_macros::ServerPacket;
#[expect(unused_imports)]
use steel_registry::packets::play::S_EDIT_BOOK;
use steel_utils::{
    codec::VarInt,
    serial::{ReadFrom, prefixed_read::read_utf},
};

/// Maximum UTF-16 code units per book page (vanilla `stringUtf8(1024)`).
pub const MAX_BOOK_PAGE_LENGTH: usize = 1024;
/// Maximum pages per book (vanilla `list(100)`).
pub const MAX_BOOK_PAGE_COUNT: usize = 100;
/// Maximum UTF-16 code units for the book title (vanilla `stringUtf8(32)`).
pub const MAX_BOOK_TITLE_LENGTH: usize = 32;

/// Sent when the player finishes editing a book and quill.
///
/// Matches vanilla's `ServerboundEditBookPacket`: the inventory slot as a
/// `VarInt`, a `VarInt`-prefixed list (max 100) of page strings (max 1024
/// UTF-16 code units each), then an optional title (max 32 UTF-16 code
/// units) encoded as a presence boolean followed by the string.
#[derive(ServerPacket, Clone, Debug)]
#[packet_id(Play = S_EDIT_BOOK)]
pub struct SEditBook {
    /// Inventory slot holding the edited book.
    pub slot: i32,
    /// The new page contents.
    pub pages: Vec<String>,
    /// The new title, if the book was signed.
    pub title: Option<String>,
}

impl ReadFrom for SEditBook {
    fn read(data: &mut Cursor<&[u8]>) -> std::io::Result<Self> {
        let slot = VarInt::read(data)?.0;
        let page_count = VarInt::read(data)?.0;
        if page_count < 0 || page_count as usize > MAX_BOOK_PAGE_COUNT {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "book page count out of bounds",
            ));
        }
        let mut pages = Vec::with_capacity(page_count as usize);
        for _ in 0..page_count {
            pages.push(read_utf(data, MAX_BOOK_PAGE_LENGTH)?);
        }
        let title = if bool::read(data)? {
            Some(read_utf(data, MAX_BOOK_TITLE_LENGTH)?)
        } else {
            None
        };
        Ok(Self { slot, pages, title })
    }
}
