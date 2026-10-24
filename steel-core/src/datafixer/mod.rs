//! Vanilla `DataFixer` surface: the version gate Steel needs, and nothing more.
//!
//! Vanilla keeps every stored record's `DataVersion` and upgrades older records through
//! `net.minecraft.util.datafix.DataFixers` — a `DataFixerUpper` graph of version-keyed
//! `Schema`s over `TypeReferences`, driven by the `DataFixTypes` list of fixable storage
//! kinds. Steel does **not** port that graph: a fixer for arbitrary Minecraft versions is a
//! different project, and a partial one is worse than none because it silently leaves
//! whatever it does not model in a half-upgraded state.
//!
//! What Steel does implement is the *gate* — the half of vanilla's behaviour that makes
//! loading safe:
//!
//! * the record's `DataVersion` must be readable (a missing tag means `0`, exactly
//!   `NbtUtils.getDataVersion`), and
//! * it must be **equal** to this build's [`DATA_VERSION`].
//!
//! An equal version loads normally. Anything else is refused with a message saying which
//! version was found and what to do about it: a *newer* record cannot be downgraded without
//! corrupting it, and an *older* record cannot be upgraded because there is no fixer graph.
//! Loading the older record anyway — the previous behaviour, which merely logged a warning —
//! is the unsafe option, because the codecs would then read fields the fixer was supposed to
//! rewrite and land on plausible-looking but wrong block states, items, and biomes.
//!
//! Every vanilla-format reader therefore routes its record through [`ensure_current`]:
//! chunks and the entity/POI side regions (`chunk_saver::anvil`), `level.dat`
//! (`level_data::anvil`), and `data/*.dat` saved data (`chunk_saver::anvil::saved_data`).
//!
//! Steel's own formats carry their own version stamps — the `.srg` region header
//! (`chunk_saver::format::FORMAT_VERSION`), `level.toml`, and the `STLP` player payload — and
//! are gated by [`ensure_steel_current`] for the same reason.
//!
//! A future port of the fixer graph would grow a `Schema` table next to [`DataFixType`] and
//! turn [`ensure_current`]'s "older" arm into a dispatch; nothing else here would change.

use std::io;

use simdnbt::borrow::NbtCompound as BorrowedNbtCompound;
use simdnbt::owned::NbtCompound;
use steel_utils::version::{DATA_VERSION, MINECRAFT_VERSION};

/// The `DataVersion` a stored record declares, following vanilla `NbtUtils.getDataVersion`.
///
/// A missing tag is version `0`: vanilla writes `DataVersion` on every record it saves, so
/// its absence means the record predates the tag and is therefore older than anything this
/// build can read.
#[must_use]
pub fn version_of(nbt: &NbtCompound) -> i32 {
    nbt.int("DataVersion").unwrap_or(0)
}

/// [`version_of`] for a borrowed `simdnbt` compound, as read straight off the wire.
#[must_use]
pub fn version_of_borrowed(nbt: &BorrowedNbtCompound<'_, '_>) -> i32 {
    nbt.int("DataVersion").unwrap_or(0)
}

/// How a stored data version relates to this build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataVersionRelation {
    /// Written by this build; the codecs can read it as-is.
    Current,
    /// Written by an older version, which vanilla would upgrade through `DataFixers`.
    Older,
    /// Written by a newer version, which vanilla would refuse to downgrade.
    Newer,
}

/// Classifies a vanilla `DataVersion` against this build's [`DATA_VERSION`].
#[must_use]
pub const fn classify(found: i32) -> DataVersionRelation {
    if found == DATA_VERSION {
        DataVersionRelation::Current
    } else if found < DATA_VERSION {
        DataVersionRelation::Older
    } else {
        DataVersionRelation::Newer
    }
}

/// The kinds of stored data vanilla lists in `DataFixTypes`.
///
/// Steel reads a subset of these; the enum exists so error messages name the domain and so a
/// future fixer table has a place to hang off. The mapping to vanilla's enum is the obvious
/// one (`DataFixTypes.LEVEL` → [`DataFixType::Level`], and so on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFixType {
    /// `level.dat` — vanilla `DataFixTypes.LEVEL`.
    Level,
    /// `playerdata/<uuid>.dat` — vanilla `DataFixTypes.PLAYER`.
    Player,
    /// A chunk record — vanilla `DataFixTypes.CHUNK`.
    Chunk,
    /// An entity record — vanilla `DataFixTypes.ENTITY`.
    Entity,
    /// An item stack — vanilla `DataFixTypes.ITEM_STACK`.
    ItemStack,
    /// A structure template — vanilla `DataFixTypes.STRUCTURE`.
    Structure,
    /// A POI record — vanilla `DataFixTypes.POI`.
    Poi,
    /// Any other SavedData payload.
    SavedData,
}

impl DataFixType {
    /// Human-readable name used in gate errors.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Level => "level data",
            Self::Player => "player data",
            Self::Chunk => "chunk",
            Self::Entity => "entity",
            Self::ItemStack => "item stack",
            Self::Structure => "structure",
            Self::Poi => "POI",
            Self::SavedData => "saved data",
        }
    }
}

/// Refuses any vanilla-format record this build cannot prove it can read.
///
/// [`DataVersionRelation::Current`] is accepted; both other relations are `InvalidData`
/// errors. `what` names the specific record (for example the chunk position) so a single bad
/// record is identifiable in a world of thousands.
///
/// # Errors
/// Returns `InvalidData` when `found` is not this build's [`DATA_VERSION`].
pub fn ensure_current(found: i32, kind: DataFixType, what: &str) -> io::Result<()> {
    let kind = kind.as_str();
    match classify(found) {
        DataVersionRelation::Current => Ok(()),
        DataVersionRelation::Newer => Err(invalid(format!(
            "the {kind} of {what} declares data version {found}, which is newer than this build's \
             {DATA_VERSION} (Minecraft {MINECRAFT_VERSION}). Steel will not load it: a newer \
             record can only be read by rewriting it backwards, which corrupts it. Run this \
             build's version of the server, or restore a copy of the world saved by it."
        ))),
        DataVersionRelation::Older => Err(invalid(format!(
            "the {kind} of {what} declares data version {found}, which is older than this build's \
             {DATA_VERSION} (Minecraft {MINECRAFT_VERSION}). Steel does not implement Mojang's \
             DataFixers, so it cannot upgrade the record, and loading it unfixed would silently \
             produce wrong data. Open the world with a vanilla {MINECRAFT_VERSION} server first, \
             let it save, and then load the upgraded world here."
        ))),
    }
}

/// Refuses any Steel-format payload written by a different build.
///
/// Steel's own formats (`.srg` region header, `level.toml`, `STLP` player payload) carry their
/// own integer stamps rather than a Minecraft `DataVersion`. Older payloads are refused for
/// the same reason as older vanilla records — there is no migration for them unless one is
/// written explicitly — but the message points at Steel's formats rather than at DataFixers.
///
/// # Errors
/// Returns `InvalidData` when `found` differs from `current`.
pub fn ensure_steel_current(found: u64, current: u64, what: &str) -> io::Result<()> {
    if found == current {
        return Ok(());
    }
    let direction = if found < current { "older" } else { "newer" };
    Err(invalid(format!(
        "{what} uses Steel format version {found}, but this build writes version {current}, so \
         the payload is {direction} than this build. Steel has no migration for that format \
         change; restore the matching backup of the world (or the file's `.bak` copy) and load \
         it with the build that wrote it."
    )))
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_version(version: Option<i32>) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        if let Some(version) = version {
            nbt.insert("DataVersion", version);
        }
        nbt
    }

    #[test]
    fn missing_data_version_reads_as_zero_like_vanilla() {
        assert_eq!(version_of(&with_version(None)), 0);
    }

    #[test]
    fn classify_splits_current_older_and_newer() {
        assert_eq!(classify(DATA_VERSION), DataVersionRelation::Current);
        assert_eq!(classify(DATA_VERSION - 1), DataVersionRelation::Older);
        assert_eq!(classify(0), DataVersionRelation::Older);
        assert_eq!(classify(DATA_VERSION + 1), DataVersionRelation::Newer);
    }

    #[test]
    fn current_version_passes_the_gate() {
        ensure_current(DATA_VERSION, DataFixType::Chunk, "chunk (3, -7)")
            .expect("the build's own data version must load");
    }

    #[test]
    fn newer_version_is_refused_and_names_the_found_version() {
        let error = ensure_current(DATA_VERSION + 1, DataFixType::Chunk, "chunk (3, -7)")
            .expect_err("a newer record must be refused");
        let message = error.to_string();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(message.contains(&(DATA_VERSION + 1).to_string()), "{message}");
        assert!(message.contains("newer"), "{message}");
        assert!(message.contains("chunk (3, -7)"), "{message}");
    }

    #[test]
    fn older_version_is_refused_and_points_at_vanilla() {
        let error = ensure_current(DATA_VERSION - 1, DataFixType::Level, "level.dat")
            .expect_err("an older record must be refused");
        let message = error.to_string();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(message.contains("older"), "{message}");
        assert!(message.contains("DataFixers"), "{message}");
        assert!(message.contains(MINECRAFT_VERSION), "{message}");
    }

    #[test]
    fn unstamped_record_is_refused_as_older() {
        let found = version_of(&with_version(None));
        let error = ensure_current(found, DataFixType::Poi, "POI record")
            .expect_err("an unstamped record predates the DataVersion tag");
        assert!(error.to_string().contains("older"), "{error}");
    }

    #[test]
    fn steel_format_gate_accepts_only_the_current_version() {
        ensure_steel_current(24, 24, "region file r.0.0.srg").expect("matching version loads");

        let older = ensure_steel_current(23, 24, "region file r.0.0.srg")
            .expect_err("an older Steel payload must be refused");
        assert!(older.to_string().contains("older"), "{older}");

        let newer = ensure_steel_current(25, 24, "region file r.0.0.srg")
            .expect_err("a newer Steel payload must be refused");
        assert!(newer.to_string().contains("newer"), "{newer}");
    }
}