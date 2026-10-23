//! Vanilla `UserBanList` (`banned-players.json`) and its entry shape.

use std::{io, path::PathBuf, time::SystemTime};

use serde::{Deserialize, Serialize};
use steel_utils::translations;
use text_components::TextComponent;
use uuid::Uuid;

use super::stored_user_list::{
    NameAndId, StoredUserEntry, StoredUserList, deserialize_created, deserialize_expires,
    serialize_ban_date, serialize_created,
};

/// Vanilla `BanListEntry` fields, shared by player and IP bans.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct BanDetails {
    /// Vanilla `created`; defaults to now when absent or unparsable.
    #[serde(
        default = "SystemTime::now",
        serialize_with = "serialize_created",
        deserialize_with = "deserialize_created"
    )]
    pub(super) created: SystemTime,
    /// Vanilla `source`; defaults to the vanilla `(Unknown)` placeholder.
    #[serde(default = "default_source")]
    pub(super) source: String,
    /// Vanilla `expires`; `None` serializes as the vanilla `forever` sentinel.
    #[serde(
        default,
        serialize_with = "serialize_ban_date",
        deserialize_with = "deserialize_expires"
    )]
    pub(super) expires: Option<SystemTime>,
    /// Vanilla `reason`.
    #[serde(default)]
    pub(super) reason: Option<String>,
}

impl BanDetails {
    /// Creates one set of ban metadata, applying vanilla defaults.
    pub(super) fn new(
        created: Option<SystemTime>,
        source: String,
        expires: Option<SystemTime>,
        reason: Option<String>,
    ) -> Self {
        Self {
            created: created.unwrap_or_else(SystemTime::now),
            source,
            expires,
            reason,
        }
    }

    /// Vanilla `BanListEntry.hasExpired`.
    pub(super) fn has_expired(&self) -> bool {
        self.expires
            .is_some_and(|expires| expires < SystemTime::now())
    }

    /// Vanilla `BanListEntry.getReasonMessage`.
    pub(super) fn reason_message(&self) -> TextComponent {
        self.reason.as_ref().map_or_else(
            || {
                translations::MULTIPLAYER_DISCONNECT_BANNED_REASON_DEFAULT
                    .msg()
                    .into()
            },
            |reason| TextComponent::plain(reason.to_owned()),
        )
    }
}

fn default_source() -> String {
    "(Unknown)".to_owned()
}

/// One `banned-players.json` entry, mirroring vanilla `UserBanListEntry`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserBanEntry {
    /// Banned identity, flattened into the top-level `uuid`/`name` properties.
    #[serde(flatten)]
    user: NameAndId,
    /// Ban metadata, flattened into the entry's remaining properties.
    #[serde(flatten)]
    details: BanDetails,
}

impl UserBanEntry {
    /// Creates one player ban, applying vanilla defaults for absent values.
    #[must_use]
    pub fn new(
        uuid: Uuid,
        name: String,
        created: Option<SystemTime>,
        source: String,
        expires: Option<SystemTime>,
        reason: Option<String>,
    ) -> Self {
        Self {
            user: NameAndId::new(uuid, name),
            details: BanDetails::new(created, source, expires, reason),
        }
    }

    /// Returns the banned player's UUID.
    #[must_use]
    pub const fn uuid(&self) -> Uuid {
        self.user.uuid
    }

    /// Returns the banned player's recorded name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.user.name
    }

    /// Returns the identity used for list lookups.
    #[must_use]
    pub const fn name_and_id(&self) -> &NameAndId {
        &self.user
    }

    /// Returns who issued the ban.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.details.source
    }

    /// Returns the ban reason, when one was given.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.details.reason.as_deref()
    }

    /// Returns when the ban was created.
    #[must_use]
    pub const fn created(&self) -> SystemTime {
        self.details.created
    }

    /// Returns when the ban expires, or `None` for a permanent ban.
    #[must_use]
    pub const fn expires(&self) -> Option<SystemTime> {
        self.details.expires
    }

    /// Vanilla `UserBanListEntry.getDisplayName`.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        TextComponent::plain(self.user.name.clone())
    }

    /// Vanilla `BanListEntry.getReasonMessage`.
    #[must_use]
    pub fn reason_message(&self) -> TextComponent {
        self.details.reason_message()
    }

    /// Returns whether this ban has already expired.
    #[must_use]
    pub fn has_expired(&self) -> bool {
        self.details.has_expired()
    }
}

impl StoredUserEntry for UserBanEntry {
    fn key(&self) -> String {
        self.user.uuid.to_string()
    }

    fn has_expired(&self) -> bool {
        self.details.has_expired()
    }
}

/// Vanilla `UserBanList`.
pub struct UserBanList {
    list: StoredUserList<UserBanEntry>,
}

impl UserBanList {
    /// Creates a ban list that never touches disk.
    pub(super) fn in_memory() -> Self {
        Self {
            list: StoredUserList::in_memory(),
        }
    }

    /// Loads `banned-players.json`, returning an empty list when absent.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub(super) async fn load(file: Option<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            list: StoredUserList::load(file).await?,
        })
    }

    /// Re-reads `banned-players.json` from disk.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub async fn reload(&self) -> io::Result<()> {
        self.list.reload().await
    }

    /// Vanilla `UserBanList.isBanned`.
    #[must_use]
    pub fn is_banned(&self, uuid: Uuid) -> bool {
        self.list.contains(&uuid.to_string())
    }

    /// Returns one ban entry, dropping expired bans first.
    #[must_use]
    pub fn get(&self, uuid: Uuid) -> Option<UserBanEntry> {
        self.list.get(&uuid.to_string())
    }

    /// Returns every active ban entry.
    #[must_use]
    pub fn entries(&self) -> Vec<UserBanEntry> {
        self.list.all()
    }

    /// Returns every banned player name, as vanilla `UserBanList.getUserList` does.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.entries()
            .into_iter()
            .map(|entry| entry.user.name)
            .collect()
    }

    /// Bans one player, returning whether the list changed.
    pub async fn add(&self, entry: UserBanEntry) -> bool {
        self.list.insert(entry).await
    }

    /// Pardons one player, returning whether the list changed.
    pub async fn remove(&self, uuid: Uuid) -> bool {
        self.list.remove(&uuid.to_string()).await
    }

    /// Removes every ban.
    pub async fn clear(&self) {
        self.list.clear().await;
    }

    /// Returns the number of active bans.
    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Returns whether no player is banned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use serde_json::json;
    use uuid::Uuid;

    use super::UserBanEntry;

    #[test]
    fn ban_entries_round_trip_through_the_vanilla_json_shape() {
        let uuid = Uuid::from_u128(5);
        let expires = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let entry = UserBanEntry::new(
            uuid,
            "Herobrine".to_owned(),
            Some(SystemTime::UNIX_EPOCH),
            "Server".to_owned(),
            Some(expires),
            Some("griefing".to_owned()),
        );

        let value = serde_json::to_value(&entry);
        assert!(value.is_ok_and(|value| {
            value
                == json!({
                    "uuid": uuid.to_string(),
                    "name": "Herobrine",
                    "created": "1970-01-01 00:00:00 +0000",
                    "source": "Server",
                    "expires": "2023-11-14 22:13:20 +0000",
                    "reason": "griefing",
                })
        }));

        let Ok(parsed) = serde_json::from_value::<UserBanEntry>(json!({
            "uuid": uuid.to_string(),
            "name": "Herobrine",
            "expires": "forever",
            "reason": null,
        })) else {
            panic!("a minimal ban entry should load");
        };
        assert_eq!(parsed.uuid(), uuid);
        assert_eq!(parsed.source(), "(Unknown)");
        assert_eq!(parsed.expires(), None);
        assert_eq!(parsed.reason(), None);
        assert!(!parsed.has_expired());
    }

    #[test]
    fn expired_bans_are_detected() {
        let entry = UserBanEntry::new(
            Uuid::from_u128(6),
            "Old".to_owned(),
            None,
            "Server".to_owned(),
            Some(SystemTime::UNIX_EPOCH),
            None,
        );

        assert!(entry.has_expired());
    }
}
