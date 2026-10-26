//! Vanilla `IpBanList` (`banned-ips.json`).

use std::{io, net::SocketAddr, path::PathBuf, time::SystemTime};

use serde::{Deserialize, Serialize};
use text_components::TextComponent;

use super::stored_user_list::{StoredUserEntry, StoredUserList};
use super::user_ban_list::BanDetails;

/// One `banned-ips.json` entry, mirroring vanilla `IpBanListEntry`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpBanEntry {
    /// Vanilla `ip`; the banned address without a port.
    ip: String,
    /// Ban metadata, flattened into the entry's remaining properties.
    #[serde(flatten)]
    details: BanDetails,
}

impl IpBanEntry {
    /// Creates one IP ban, applying vanilla defaults for absent values.
    #[must_use]
    pub fn new(
        ip: String,
        created: Option<SystemTime>,
        source: String,
        expires: Option<SystemTime>,
        reason: Option<String>,
    ) -> Self {
        Self {
            ip,
            details: BanDetails::new(created, source, expires, reason),
        }
    }

    /// Returns the banned address.
    #[must_use]
    pub fn ip(&self) -> &str {
        &self.ip
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

    /// Vanilla `IpBanListEntry.getDisplayName`.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        TextComponent::plain(self.ip.clone())
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

impl StoredUserEntry for IpBanEntry {
    fn key(&self) -> String {
        self.ip.clone()
    }

    fn has_expired(&self) -> bool {
        self.details.has_expired()
    }
}

/// Vanilla `IpBanList`.
pub struct IpBanList {
    list: StoredUserList<IpBanEntry>,
}

impl IpBanList {
    /// Creates an IP ban list that never touches disk.
    pub(super) fn in_memory() -> Self {
        Self {
            list: StoredUserList::in_memory(),
        }
    }

    /// Loads `banned-ips.json`, returning an empty list when absent.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub(super) async fn load(file: Option<PathBuf>) -> io::Result<Self> {
        Ok(Self {
            list: StoredUserList::load(file).await?,
        })
    }

    /// Re-reads `banned-ips.json` from disk.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the file exists but cannot be read or parsed.
    pub async fn reload(&self) -> io::Result<()> {
        self.list.reload().await
    }

    /// Vanilla `IpBanList.isBanned(SocketAddress)`, reducing the address to its IP.
    #[must_use]
    pub fn is_banned(&self, address: SocketAddr) -> bool {
        self.is_banned_ip(&address.ip().to_string())
    }

    /// Returns whether one address without a port is banned.
    #[must_use]
    pub fn is_banned_ip(&self, ip: &str) -> bool {
        self.list.contains(ip)
    }

    /// Returns one IP ban entry, dropping expired bans first.
    #[must_use]
    pub fn get(&self, address: SocketAddr) -> Option<IpBanEntry> {
        self.get_ip(&address.ip().to_string())
    }

    /// Returns one IP ban entry by address string.
    #[must_use]
    pub fn get_ip(&self, ip: &str) -> Option<IpBanEntry> {
        self.list.get(ip)
    }

    /// Returns every active IP ban entry.
    #[must_use]
    pub fn entries(&self) -> Vec<IpBanEntry> {
        self.list.all()
    }

    /// Returns every banned address, as vanilla `IpBanList.getUserList` does.
    #[must_use]
    pub fn user_list(&self) -> Vec<String> {
        self.list.keys()
    }

    /// Bans one address, returning whether the list changed.
    pub async fn add(&self, entry: IpBanEntry) -> bool {
        self.list.insert(entry).await
    }

    /// Pardons one address, returning whether the list changed.
    pub async fn remove(&self, ip: &str) -> bool {
        self.list.remove(ip).await
    }

    /// Removes every IP ban.
    pub async fn clear(&self) {
        self.list.clear().await;
    }

    /// Returns the number of active IP bans.
    #[must_use]
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Returns whether no address is banned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// Returns the IP string vanilla derives from a socket address.
#[must_use]
pub fn ip_from_socket_address(address: &SocketAddr) -> String {
    address.ip().to_string()
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::{Duration, SystemTime};

    use serde_json::json;

    use super::IpBanEntry;

    #[test]
    fn ip_bans_serialize_with_the_vanilla_ip_property() {
        let expires = SystemTime::now() + Duration::from_secs(60);
        let entry = IpBanEntry::new(
            "203.0.113.7".to_owned(),
            None,
            "Server".to_owned(),
            Some(expires),
            None,
        );

        assert!(serde_json::to_value(&entry).is_ok_and(|value| {
            value["ip"] == json!("203.0.113.7")
                && value["reason"] == json!(null)
                && value["expires"].is_string()
        }));
    }

    #[test]
    fn socket_addresses_reduce_to_their_address() {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 25565);
        assert_eq!(super::ip_from_socket_address(&address), "10.0.0.1");
    }
}
