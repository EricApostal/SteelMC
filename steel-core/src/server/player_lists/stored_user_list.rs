//! Persistence core shared by Steel's vanilla player lists.
//!
//! Vanilla keeps ops, bans, and the allowlist in `StoredUserList` subclasses that
//! load and save a pretty-printed JSON array of entries. Steel keeps the same file
//! names and entry shape, but stores them under the server save root (the directory
//! that already holds Steel's server-level player files) instead of the process
//! working directory, and writes them atomically.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    time::SystemTime,
};

use chrono::{DateTime, Utc};
use serde::{Serialize, de::DeserializeOwned};
use steel_utils::locks::{AsyncMutex, SyncMutex};
use tokio::fs;
use uuid::Uuid;

/// Vanilla `NameAndId`: the UUID/name pair every stored player entry carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct NameAndId {
    /// Player UUID.
    pub uuid: Uuid,
    /// Player name as recorded when the entry was created.
    pub name: String,
}

impl NameAndId {
    /// Creates a stored identity.
    #[must_use]
    pub const fn new(uuid: Uuid, name: String) -> Self {
        Self { uuid, name }
    }
}

/// One entry stored in a vanilla `StoredUserList`.
pub(super) trait StoredUserEntry: Clone + PartialEq + Serialize + DeserializeOwned {
    /// Vanilla `getKeyForUser`: the map key this entry is stored under.
    fn key(&self) -> String;

    /// Vanilla `StoredUserEntry.hasExpired`.
    fn has_expired(&self) -> bool {
        false
    }
}

/// Vanilla `StoredUserList` behaviour over one JSON file.
pub(super) struct StoredUserList<V: StoredUserEntry> {
    file: Option<PathBuf>,
    save_lock: AsyncMutex<()>,
    entries: SyncMutex<BTreeMap<String, V>>,
}

impl<V: StoredUserEntry> StoredUserList<V> {
    /// Creates a list that never touches disk.
    pub(super) fn in_memory() -> Self {
        Self {
            file: None,
            save_lock: AsyncMutex::new(()),
            entries: SyncMutex::new(BTreeMap::new()),
        }
    }

    /// Loads one list, skipping unparsable entries as vanilla does.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read.
    pub(super) async fn load(file: Option<PathBuf>) -> io::Result<Self> {
        let Some(file) = file else {
            return Ok(Self::in_memory());
        };
        let list = Self {
            file: Some(file),
            save_lock: AsyncMutex::new(()),
            entries: SyncMutex::new(BTreeMap::new()),
        };
        list.reload().await?;
        Ok(list)
    }

    /// Re-reads the backing file, clearing entries when it is absent.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read.
    pub(super) async fn reload(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let mut entries = BTreeMap::new();
        if fs::try_exists(file).await? {
            let bytes = fs::read(file).await?;
            if !bytes.is_empty() {
                let raw: Vec<serde_json::Value> =
                    serde_json::from_slice(&bytes).map_err(|error| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("{}: {error}", file.display()),
                        )
                    })?;
                for value in raw {
                    match serde_json::from_value::<V>(value) {
                        Ok(entry) => {
                            entries.insert(entry.key(), entry);
                        }
                        Err(error) => {
                            log::warn!("Could not parse an entry of {}: {error}", file.display());
                        }
                    }
                }
            }
        }
        *self.entries.lock() = entries;
        Ok(())
    }

    /// Returns the number of stored entries.
    pub(super) fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Returns whether the list is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Returns whether `key` is stored, dropping expired entries first.
    pub(super) fn contains(&self, key: &str) -> bool {
        self.remove_expired();
        self.entries.lock().contains_key(key)
    }

    /// Returns one entry, dropping expired entries first.
    pub(super) fn get(&self, key: &str) -> Option<V> {
        self.remove_expired();
        self.entries.lock().get(key).cloned()
    }

    /// Returns every stored entry in key order.
    pub(super) fn all(&self) -> Vec<V> {
        self.remove_expired();
        self.entries.lock().values().cloned().collect()
    }

    /// Returns every entry key in sorted order.
    pub(super) fn keys(&self) -> Vec<String> {
        self.entries.lock().keys().cloned().collect()
    }

    /// Inserts one entry, returning whether the list changed.
    ///
    /// A failed save only logs a warning, matching vanilla `StoredUserList.add`.
    pub(super) async fn insert(&self, entry: V) -> bool {
        let changed = {
            let mut entries = self.entries.lock();
            let key = entry.key();
            if entries.get(&key) == Some(&entry) {
                false
            } else {
                entries.insert(key, entry);
                true
            }
        };
        if changed && let Err(error) = self.save().await {
            log::warn!("Could not save the list after adding a user: {error}");
        }
        changed
    }

    /// Removes one entry by key, returning whether the list changed.
    pub(super) async fn remove(&self, key: &str) -> bool {
        let changed = self.entries.lock().remove(key).is_some();
        if changed && let Err(error) = self.save().await {
            log::warn!("Could not save the list after removing a user: {error}");
        }
        changed
    }

    /// Removes every entry.
    pub(super) async fn clear(&self) {
        if self.entries.lock().is_empty() {
            return;
        }
        self.entries.lock().clear();
        if let Err(error) = self.save().await {
            log::warn!("Could not save the list after removing a user: {error}");
        }
    }

    /// Drops entries whose ban has expired.
    fn remove_expired(&self) {
        let mut entries = self.entries.lock();
        entries.retain(|_, entry| !entry.has_expired());
    }

    async fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let _guard = self.save_lock.lock().await;
        let entries: Vec<V> = self.entries.lock().values().cloned().collect();
        let bytes = serde_json::to_vec_pretty(&entries).map_err(io::Error::other)?;
        write_atomic(file, &bytes).await
    }
}

/// Vanilla `BanListEntry.DATE_FORMAT` (`yyyy-MM-dd HH:mm:ss Z`).
pub(super) const BAN_DATE_FORMAT: &str = "%Y-%m-%d %H:%M:%S %z";

/// The vanilla `expires` sentinel for a permanent ban.
pub(super) const EXPIRES_NEVER: &str = "forever";

/// Formats a timestamp the way vanilla ban entries are written.
pub(super) fn format_ban_date(time: SystemTime) -> String {
    DateTime::<Utc>::from(time)
        .format(BAN_DATE_FORMAT)
        .to_string()
}

/// Formats an optional timestamp, writing vanilla's `forever` for permanent bans.
pub(super) fn serialize_ban_date<S: serde::Serializer>(
    value: &Option<SystemTime>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.map_or_else(|| EXPIRES_NEVER.to_owned(), format_ban_date))
}

/// Parses a vanilla `created` timestamp, defaulting to now on failure.
pub(super) fn deserialize_created<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SystemTime, D::Error> {
    let raw = <Option<String> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(raw
        .as_deref()
        .and_then(parse_ban_date)
        .unwrap_or_else(SystemTime::now))
}

/// Parses a vanilla `expires` timestamp, treating `forever` and failures as permanent.
pub(super) fn deserialize_expires<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SystemTime>, D::Error> {
    let raw = <Option<String> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(raw.as_deref().and_then(parse_ban_date))
}

/// Parses a vanilla ban timestamp, falling back to RFC 3339 as well.
pub(super) fn parse_ban_date(value: &str) -> Option<SystemTime> {
    if let Ok(parsed) = DateTime::parse_from_str(value, BAN_DATE_FORMAT) {
        return Some(parsed.into());
    }
    DateTime::parse_from_rfc3339(value).ok().map(Into::into)
}

/// Returns the filename stem suffix vanilla uses for the temporary write.
fn temporary_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    file.with_file_name(name)
}

/// Writes bytes to `file` through a temporary sibling and a rename.
async fn write_atomic(file: &Path, bytes: &[u8]) -> io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let temporary = temporary_path(file);
    if let Some(parent) = file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).await?;
    }
    let mut handle = fs::File::create(&temporary).await?;
    handle.write_all(bytes).await?;
    handle.sync_all().await?;
    drop(handle);
    fs::rename(&temporary, file).await
}

/// Formats a required `created` timestamp the way vanilla writes it.
pub(super) fn serialize_created<S: serde::Serializer>(
    value: &SystemTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format_ban_date(*value))
}

/// Vanilla `PlayerList.BAN_DATE_FORMAT` (`yyyy-MM-dd 'at' HH:mm:ss z`).
const BAN_DISPLAY_FORMAT: &str = "%Y-%m-%d at %H:%M:%S %Z";

/// Formats a ban expiration for player-visible kick messages.
pub fn format_ban_expiry(time: SystemTime) -> String {
    DateTime::<Utc>::from(time)
        .format(BAN_DISPLAY_FORMAT)
        .to_string()
}
