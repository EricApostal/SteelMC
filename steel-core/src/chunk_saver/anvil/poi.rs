//! Vanilla POI storage ⇄ `PersistentPoi`.
//!
//! `poi/r.<x>.<z>.mca` holds one record per chunk column:
//!
//! ```text
//! { Sections: { "<sectionY>": { Valid: 1b, Records: [
//!       { pos: [x, y, z], type: "minecraft:home", free_tickets: 0 } ] } },
//!   DataVersion: <int> }
//! ```
//!
//! Steel only persists `(x, y, z, free_tickets)` and re-derives the POI type from the block
//! state on load, so the writer re-derives the vanilla `type` from the chunk's block states
//! and the reader validates the recorded type against the derived one (the derived type wins,
//! with a warning on mismatch).

use std::io;
use std::str::FromStr;

use simdnbt::borrow::NbtCompound as BorrowedNbtCompound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::Identifier;

use super::nbt::compound_list;

/// One POI entry in vanilla layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VanillaPoi {
    /// Absolute block position.
    pub pos: [i32; 3],
    /// Recorded POI type.
    pub poi_type: Option<Identifier>,
    /// Remaining free tickets.
    pub free_tickets: u32,
}

/// Serializes POI entries into one vanilla chunk record.
///
/// Entries are grouped by section Y exactly like vanilla `SectionStorage`.
#[must_use]
pub(crate) fn poi_to_nbt(entries: &[VanillaPoi]) -> NbtCompound {
    let mut sections: Vec<(i32, NbtCompound)> = Vec::new();
    for entry in entries {
        let section_y = entry.pos[1].div_euclid(16);
        let record = section_record(entry);
        if let Some((_, section)) = sections
            .iter_mut()
            .find(|(existing, _)| *existing == section_y)
        {
            if let Some(NbtTag::List(NbtList::Compound(records))) = section.get_mut("Records") {
                records.push(record);
            }
            continue;
        }

        let mut section = NbtCompound::new();
        section.insert("Valid", NbtTag::Byte(1));
        section.insert("Records", NbtList::Compound(vec![record]));
        sections.push((section_y, section));
    }

    let mut root = NbtCompound::new();
    let mut sections_compound = NbtCompound::new();
    for (section_y, section) in sections {
        sections_compound.insert(section_y.to_string(), NbtTag::Compound(section));
    }
    root.insert("Sections", NbtTag::Compound(sections_compound));
    root.insert("DataVersion", steel_utils::version::DATA_VERSION);
    root
}

/// Parses one vanilla POI chunk record.
///
/// # Errors
/// Returns `InvalidData` when a record's `pos` is malformed.
pub(crate) fn poi_from_nbt(tag: &BorrowedNbtCompound<'_, '_>) -> io::Result<Vec<VanillaPoi>> {
    let mut entries = Vec::new();
    let Some(sections) = tag.compound("Sections") else {
        return Ok(entries);
    };

    for (_, section) in sections.iter() {
        let Some(section) = section.compound() else {
            continue;
        };
        let records = section.list("Records").map_or_else(Vec::new, compound_list);
        for record in records {
            let Some(pos) = record.list("pos").and_then(|list| list.ints()) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "vanilla POI record has no integer `pos`",
                ));
            };
            if pos.len() < 3 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "vanilla POI record `pos` has fewer than three coordinates",
                ));
            }
            let poi_type = record
                .string("type")
                .and_then(|value| Identifier::from_str(value.to_str().as_ref()).ok());
            let free_tickets = record.int("free_tickets").unwrap_or(0).max(0) as u32;
            entries.push(VanillaPoi {
                pos: [pos[0], pos[1], pos[2]],
                poi_type,
                free_tickets,
            });
        }
    }

    Ok(entries)
}

fn section_record(entry: &VanillaPoi) -> NbtCompound {
    let mut record = NbtCompound::new();
    record.insert("pos", NbtTag::IntArray(entry.pos.to_vec()));
    if let Some(poi_type) = &entry.poi_type {
        record.insert("type", poi_type.to_string());
    }
    record.insert("free_tickets", entry.free_tickets as i32);
    record
}
