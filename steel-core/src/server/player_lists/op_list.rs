//! Vanilla `ServerOpList` (`ops.json`).

use std::{io, path::PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::stored_user_list::{NameAndId, StoredUserEntry, StoredUserList};
use crate::permission::PermissionLevel;

/// One `ops.json` entry, mirroring vanilla `ServerOpListEntry`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerOpEntry {
    /// Stored identity, flattened into the top-level `uuid`/`name` properties.
    #[serde(flatten)]
    user: NameAndId,
    /// Vanilla `level`; absent entries load as `ALL` like vanilla's default.
    #[serde(default)]
    level: PermissionLevel,
    /// Vanilla `bypassesPlayerLimit`.
    #[serde(rename = "bypassesPlayerLimit", default)]
    bypasses_player_limit: bool,
}

impl ServerOpEntry {
    /// Creates one operator entry.
    #[must_use]
    pub const fn new(
        uuid: Uuid,
        name: String,
        level: PermissionLevel,
        bypasses_player_limit: bool,
    ) -> Self {
        Self {
            user: NameAndId::new(uuid, name),
            level,
            bypasses_player_limit,
        }
    }

    /// Returns the operator's UUID.
    #[must_use]
    pub const fn uuid(&self) -> Uuid {
        self.user.uuid
    }

    /// Returns the operator's recorded name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.user.name
    }

    /// Returns the granted vanilla op level.
    #[must_use]
    pub const fn level(&self) -> PermissionLevel {
        self.level
    }

    /// Returns whether this operator ignores the player limit.
    #[must_use]
    pub const fn bypasses_player_limit(&self) -> bool {
        self.bypasses_player_limit
    }
}

impl StoredUserEntry for ServerOpEntry {
    fn key(&self) -> String {
        self.user.uuid.to_string()
    }
}

/// Vanilla `ServerOpList`.
pub struct OpList {
    list: StoredUserList<ServerOpEntry>,
}

impl OpList {
    /// Creates an operator list that never touches disk.
    /// Test/Flint-adapter only: lists that never touch disk.
    #[cfg(any(test, feature = "test-framework"))]
    pub(super) fn in_memory() -> Self {
        Self {
            list: StoredUserList::in_memory(),
        }
    }

    /// Loads `ops.json`, returning an empty list when the file is absent.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub(super) async fn load(file: Option<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            list: StoredUserList::load(file).await?,
        })
    }

    /// Re-reads `ops.json` from disk.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub async fn reload(&self) -> io::Result<()> {
        self.list.reload().await
    }

    /// Returns whether `uuid` is an operator.
    #[must_use]
    pub fn is_op(&self, uuid: Uuid) -> bool {
        self.list.contains(&uuid.to_string())
    }

    /// Returns one operator entry.
    #[must_use]
    pub fn get(&self, uuid: Uuid) -> Option<ServerOpEntry> {
        self.list.get(&uuid.to_string())
    }

    /// Returns the vanilla op level granted to `uuid`, defaulting to `ALL`.
    #[must_use]
    pub fn level(&self, uuid: Uuid) -> PermissionLevel {
        self.get(uuid)
            .map_or(PermissionLevel::All, |entry| entry.level)
    }

    /// Returns whether `uuid` may bypass the player limit, as vanilla `ServerOpList` does.
    #[must_use]
    pub fn can_bypass_player_limit(&self, uuid: Uuid) -> bool {
        self.get(uuid)
            .is_some_and(|entry| entry.bypasses_player_limit)
    }

    /// Returns every operator entry.
    #[must_use]
    pub fn entries(&self) -> Vec<ServerOpEntry> {
        self.list.all()
    }

    /// Returns every operator name, as vanilla `ServerOpList.getUserList` does.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.entries()
            .into_iter()
            .map(|entry| entry.user.name)
            .collect()
    }

    /// Adds one operator entry, returning whether the list changed.
    pub async fn add(&self, entry: ServerOpEntry) -> bool {
        self.list.insert(entry).await
    }

    /// Removes one operator, returning whether the list changed.
    pub async fn remove(&self, uuid: Uuid) -> bool {
        self.list.remove(&uuid.to_string()).await
    }

    /// Removes every operator.
    pub async fn clear(&self) {
        self.list.clear().await;
    }

    /// Returns the number of operators.
    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Returns whether no players are operators.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::{OpList, ServerOpEntry};
    use crate::permission::PermissionLevel;

    #[test]
    fn op_entries_use_vanilla_json_shape_and_uuid_key() {
        let _list = OpList::in_memory();
        let uuid = Uuid::from_u128(7);
        let entry = ServerOpEntry::new(uuid, "Notch".to_owned(), PermissionLevel::Admins, true);

        let serialized = serde_json::to_value(&entry);
        assert!(serialized.is_ok_and(|value| {
            value
                == serde_json::json!({
                    "uuid": uuid.to_string(),
                    "name": "Notch",
                    "level": 3,
                    "bypassesPlayerLimit": true,
                })
        }));

        let Ok(parsed) = serde_json::from_value::<ServerOpEntry>(serde_json::json!({
            "uuid": uuid.to_string(),
            "name": "Notch",
        })) else {
            panic!("an entry without level should still load");
        };
        assert_eq!(parsed.level(), PermissionLevel::All);
        assert!(!parsed.bypasses_player_limit());
    }

    #[tokio::test]
    async fn adding_the_same_entry_twice_only_changes_the_list_once() {
        let list = super::OpList::in_memory();
        let uuid = Uuid::from_u128(11);
        let entry = ServerOpEntry::new(uuid, "Alex".to_owned(), PermissionLevel::Owners, false);

        assert!(list.add(entry.clone()).await);
        assert!(!list.add(entry).await);
        assert!(list.is_op(uuid));
        assert_eq!(list.level(uuid), PermissionLevel::Owners);
        assert!(!list.can_bypass_player_limit(uuid));
        assert!(list.remove(uuid).await);
        assert!(!list.is_op(uuid));
    }
}
