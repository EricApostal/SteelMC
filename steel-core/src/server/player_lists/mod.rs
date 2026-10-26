//! Vanilla `StoredUserList` files (`ops.json`, `banned-players.json`,
//! `banned-ips.json`, `whitelist.json`).
//!
//! Steel keeps the vanilla file names and entry shapes, but stores them under
//! the server save root where Steel's server-level player files already live,
//! and writes them atomically.

mod allowlist;
mod ip_ban_list;
mod op_list;
mod stored_user_list;
mod user_ban_list;

use std::io;
use std::path::{Path, PathBuf};

pub use allowlist::{AllowList, AllowListEntry};
pub use ip_ban_list::{IpBanEntry, IpBanList, ip_from_socket_address};
pub use op_list::{OpList, ServerOpEntry};
pub use stored_user_list::format_ban_expiry;
pub use user_ban_list::{UserBanEntry, UserBanList};

/// Vanilla `PlayerList.OPLIST_FILE`.
pub const OPS_FILE: &str = "ops.json";
/// Vanilla `PlayerList.USERBANLIST_FILE`.
pub const USER_BAN_LIST_FILE: &str = "banned-players.json";
/// Vanilla `PlayerList.IPBANLIST_FILE`.
pub const IP_BAN_LIST_FILE: &str = "banned-ips.json";
/// Vanilla `PlayerList.WHITELIST_FILE`.
pub const ALLOW_LIST_FILE: &str = "whitelist.json";

/// Returns the path of one vanilla player-list file under `save_root`.
#[must_use]
pub fn list_path(save_root: &Path, file: &str) -> PathBuf {
    save_root.join(file)
}

/// The four vanilla player lists owned by one server.
pub(crate) struct PlayerLists {
    pub(crate) ops: OpList,
    pub(crate) bans: UserBanList,
    pub(crate) ip_bans: IpBanList,
    pub(crate) allowlist: AllowList,
}

impl PlayerLists {
    /// Creates lists that never touch disk.
    pub(crate) fn in_memory(white_list: bool) -> Self {
        Self {
            ops: OpList::in_memory(),
            bans: UserBanList::in_memory(),
            ip_bans: IpBanList::in_memory(),
            allowlist: AllowList::in_memory(white_list),
        }
    }

    /// Loads every list from `save_root`, applying the `white-list` property.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when a list file exists but cannot be read or parsed.
    pub(crate) async fn load(save_root: &Path, white_list: bool) -> io::Result<Self> {
        Ok(Self {
            ops: OpList::load(Some(list_path(save_root, OPS_FILE))).await?,
            bans: UserBanList::load(Some(list_path(save_root, USER_BAN_LIST_FILE))).await?,
            ip_bans: IpBanList::load(Some(list_path(save_root, IP_BAN_LIST_FILE))).await?,
            allowlist: AllowList::load(Some(list_path(save_root, ALLOW_LIST_FILE)), white_list)
                .await?,
        })
    }
}
