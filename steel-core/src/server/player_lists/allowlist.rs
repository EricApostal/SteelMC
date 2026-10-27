//! Vanilla `UserWhiteList` (`whitelist.json`).

use std::{
    io,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::stored_user_list::{NameAndId, StoredUserEntry, StoredUserList};

/// One `whitelist.json` entry, mirroring vanilla `UserWhiteListEntry`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowListEntry {
    /// Stored identity, flattened into the top-level `uuid`/`name` properties.
    #[serde(flatten)]
    user: NameAndId,
}

impl AllowListEntry {
    /// Creates one allowlist entry.
    #[must_use]
    pub const fn new(uuid: Uuid, name: String) -> Self {
        Self {
            user: NameAndId::new(uuid, name),
        }
    }

    /// Returns the allowlisted player's UUID.
    #[must_use]
    pub const fn uuid(&self) -> Uuid {
        self.user.uuid
    }

    /// Returns the allowlisted player's recorded name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.user.name
    }

    /// Returns the identity used for list lookups.
    #[must_use]
    pub const fn name_and_id(&self) -> &NameAndId {
        &self.user
    }
}

impl StoredUserEntry for AllowListEntry {
    fn key(&self) -> String {
        self.user.uuid.to_string()
    }
}

/// Vanilla `UserWhiteList` plus the `white-list` property that gates it.
///
/// The property itself lives in Steel's config file, so `set_enabled` only changes
/// runtime enforcement; the configured value is applied when the server loads.
pub struct AllowList {
    list: StoredUserList<AllowListEntry>,
    enabled: AtomicBool,
}

impl AllowList {
    /// Creates an allowlist that never touches disk.
    /// Test/Flint-adapter only: lists that never touch disk.
    #[cfg(any(test, feature = "test-framework"))]
    pub(super) fn in_memory(enabled: bool) -> Self {
        Self {
            list: StoredUserList::in_memory(),
            enabled: AtomicBool::new(enabled),
        }
    }

    /// Loads `whitelist.json`, returning an empty list when absent.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub(super) async fn load(file: Option<PathBuf>, enabled: bool) -> io::Result<Self> {
        Ok(Self {
            list: StoredUserList::load(file).await?,
            enabled: AtomicBool::new(enabled),
        })
    }

    /// Returns whether the allowlist currently gates logins.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Enables or disables allowlist enforcement at runtime.
    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Re-reads `whitelist.json` from disk (vanilla `PlayerList.reloadWhiteList`).
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub async fn reload(&self) -> io::Result<()> {
        self.list.reload().await
    }

    /// Vanilla `UserWhiteList.isWhiteListed`.
    #[must_use]
    pub fn is_allowed(&self, uuid: Uuid) -> bool {
        self.list.contains(&uuid.to_string())
    }

    /// Returns one allowlist entry, dropping nothing first (entries never expire).
    #[must_use]
    pub fn get(&self, uuid: Uuid) -> Option<AllowListEntry> {
        self.list.get(&uuid.to_string())
    }

    /// Returns every allowlist entry.
    #[must_use]
    pub fn entries(&self) -> Vec<AllowListEntry> {
        self.list.all()
    }

    /// Returns every allowlisted name, as vanilla `UserWhiteList.getUserList` does.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.entries()
            .into_iter()
            .map(|entry| entry.user.name)
            .collect()
    }

    /// Adds one entry, returning whether the list changed.
    pub async fn add(&self, entry: AllowListEntry) -> bool {
        self.list.insert(entry).await
    }

    /// Removes one entry, returning whether the list changed.
    pub async fn remove(&self, uuid: Uuid) -> bool {
        self.list.remove(&uuid.to_string()).await
    }

    /// Removes every entry.
    pub async fn clear(&self) {
        self.list.clear().await;
    }

    /// Returns the number of allowlisted players.
    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Returns whether no player is allowlisted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::AllowListEntry;

    #[test]
    fn allowlist_entries_round_trip_through_the_vanilla_json_shape() {
        let uuid = Uuid::from_u128(9);
        let entry = AllowListEntry::new(uuid, "Steve".to_owned());

        let serialized = serde_json::to_value(&entry);
        assert!(serialized.is_ok_and(|value| {
            value
                == serde_json::json!({
                    "uuid": uuid.to_string(),
                    "name": "Steve",
                })
        }));

        let parsed = serde_json::from_value::<AllowListEntry>(serde_json::json!({
            "uuid": uuid.to_string(),
            "name": "Steve",
        }));
        assert_eq!(parsed.expect("entry should parse"), entry);
    }
}
