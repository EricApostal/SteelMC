use std::{net::SocketAddr, time::SystemTime};

use steel_utils::{Identifier, translations};
use text_components::Modifier;
use text_components::TextComponent;
use text_components::translation::Translation;

use crate::permission::{LevelBasedPermissionSet, PermissionLevel, PermissionMetadataValue};
use crate::server::player_lists::{
    AllowList, IpBanEntry, IpBanList, OpList, ServerOpEntry, UserBanEntry, UserBanList,
    format_ban_expiry,
};
use steel_utils::entity_events::EntityStatus;

use super::{
    Arc, FnServerJob, OP_GROUP, PermissionGroupManager, PermissionGroupManagerError,
    PermissionGroupUpdateError, PermissionGroupsConfig, PermissionSet, PermissionSubjectState,
    Player, PlayerPermissionUpdateError, Server, ServerJobContext, Uuid,
};

const BYPASSES_PLAYER_LIMIT: Identifier =
    Identifier::new_static(Identifier::STEEL_NAMESPACE, "bypasses_player_limit");

pub(super) fn validate_player_permission_group_update<E>(
    manager: &PermissionGroupManager,
    previous_groups: &[String],
    updated_groups: &[String],
) -> Result<(), PlayerPermissionUpdateError<E>> {
    for group in updated_groups {
        let already_assigned = previous_groups.iter().any(|current| current == group);
        if !already_assigned && !manager.contains_group(group) {
            return Err(PlayerPermissionUpdateError::UnknownGroup(group.clone()));
        }
    }
    Ok(())
}

impl Server {
    /// Returns the vanilla operator list.
    #[must_use]
    pub const fn ops(&self) -> &OpList {
        &self.player_lists.ops
    }

    /// Returns the vanilla player ban list.
    #[must_use]
    pub const fn bans(&self) -> &UserBanList {
        &self.player_lists.bans
    }

    /// Returns the vanilla IP ban list.
    #[must_use]
    pub const fn ip_bans(&self) -> &IpBanList {
        &self.player_lists.ip_bans
    }

    /// Returns the vanilla allowlist.
    #[must_use]
    pub const fn allowlist(&self) -> &AllowList {
        &self.player_lists.allowlist
    }

    /// Returns the vanilla op level one player commands with.
    ///
    /// An `ops.json` entry wins; otherwise a player holding Steel's built-in
    /// operator group commands with the `op-permission-level` property, keeping
    /// group-assigned operators from the group era working.
    #[must_use]
    pub fn op_level(&self, uuid: Uuid) -> PermissionLevel {
        if let Some(entry) = self.ops().get(uuid) {
            return entry.level();
        }
        if self.has_operator_group(uuid) {
            return self.config.op_permission_level;
        }
        PermissionLevel::All
    }

    /// Vanilla `MinecraftServer.getProfilePermissions`.
    #[must_use]
    pub fn profile_permission_set(&self, uuid: Uuid) -> LevelBasedPermissionSet {
        LevelBasedPermissionSet::for_level(self.op_level(uuid))
    }

    /// Returns the level command functions run with (`function-permission-level`).
    #[must_use]
    pub fn function_permission_set(&self) -> LevelBasedPermissionSet {
        LevelBasedPermissionSet::for_level(self.config.function_permission_level)
    }

    /// Grants or refreshes one operator, preserving any existing bypass flag.
    ///
    /// The level is the `op-permission-level` property, as vanilla
    /// `PlayerList.op` uses `MinecraftServer.operatorUserPermissions`.
    pub async fn add_operator(&self, uuid: Uuid, name: &str) -> bool {
        let bypasses = self
            .ops()
            .get(uuid)
            .is_some_and(|entry| entry.bypasses_player_limit());
        self.ops()
            .add(ServerOpEntry::new(
                uuid,
                name.to_owned(),
                self.config.op_permission_level,
                bypasses,
            ))
            .await
    }

    /// Revokes one operator.
    pub async fn remove_operator(&self, uuid: Uuid) -> bool {
        self.ops().remove(uuid).await
    }

    /// Returns whether the published subject state assigns the operator group.
    #[must_use]
    pub(crate) fn has_operator_group(&self, uuid: Uuid) -> bool {
        self.player_permission_states
            .read()
            .get(uuid)
            .is_some_and(|state| state.groups().iter().any(|group| group == OP_GROUP))
    }

    /// Vanilla `PlayerList.isWhiteListed`: operators bypass the allowlist.
    #[must_use]
    pub fn is_whitelisted(&self, uuid: Uuid) -> bool {
        !self.allowlist().is_enabled()
            || self.ops().is_op(uuid)
            || self.allowlist().is_allowed(uuid)
    }

    /// Vanilla `MinecraftServer.kickUnlistedPlayers`.
    ///
    /// Only runs when the allowlist is both enabled and enforced.
    pub async fn kick_unlisted_players(self: &Arc<Self>) {
        if !self.allowlist().is_enabled() || !self.config.enforce_whitelist {
            return;
        }
        let unlisted: Vec<Arc<Player>> = self
            .get_players()
            .into_iter()
            .filter(|player| !self.is_whitelisted(player.gameprofile.id()))
            .collect();
        for player in unlisted {
            log::info!(
                "Kicking {} because they are not allow-listed",
                player.gameprofile.name()
            );
            player.disconnect(
                steel_utils::translations::MULTIPLAYER_DISCONNECT_NOT_WHITELISTED.msg(),
            );
        }
    }

    /// Resolves vanilla's `bypassesPlayerLimit` flag.
    ///
    /// An operator's stored `bypassesPlayerLimit` flag grants it directly; Steel's
    /// global boolean metadata `steel:bypasses_player_limit` additionally grants it
    /// through inherited groups.
    #[must_use]
    pub(crate) fn can_bypass_player_limit(&self, uuid: Uuid) -> bool {
        if self.ops().can_bypass_player_limit(uuid) {
            return true;
        }
        let subject = self.player_permission_state(uuid).unwrap_or_default();
        self.permission_groups
            .effective_metadata(subject.groups(), subject.metadata_overrides())
            .resolve(&BYPASSES_PLAYER_LIMIT)
            .and_then(PermissionMetadataValue::as_bool)
            == Some(true)
    }

    pub(super) fn apply_cached_or_default_permission_state(&self, player: &Player) -> u64 {
        let state = self
            .player_permission_states
            .read()
            .get(player.gameprofile.id())
            .cloned()
            .unwrap_or_default();
        self.apply_player_permission_state(player, state)
    }

    fn apply_player_permission_state(&self, player: &Player, state: PermissionSubjectState) -> u64 {
        let (groups, overrides, metadata_overrides) = state.into_parts();
        for group in &groups {
            if !self.permission_groups.contains_group(group) {
                log::warn!(
                    "Player {} has unknown permission group {group}",
                    player.gameprofile.name()
                );
            }
        }
        let effective = self
            .permission_groups
            .effective_permissions(&groups, &overrides);
        let effective_metadata = self
            .permission_groups
            .effective_metadata(&groups, &metadata_overrides);
        player.set_permission_state(
            groups,
            overrides,
            metadata_overrides,
            effective,
            effective_metadata,
        )
    }

    /// Returns one player's cached persisted permission state.
    #[must_use]
    pub fn player_permission_state(&self, uuid: Uuid) -> Option<PermissionSubjectState> {
        self.player_permission_states.read().get(uuid).cloned()
    }

    /// Returns whether the latest published subject state assigns the operator group,
    /// or an `ops.json` entry makes the player an operator.
    #[must_use]
    pub(crate) fn is_operator(&self, uuid: Uuid) -> bool {
        self.ops().is_op(uuid) || self.has_operator_group(uuid)
    }

    /// Captures effective command permissions from the latest published subject and group state.
    #[must_use]
    pub(crate) fn command_permission_snapshot(&self, uuid: Uuid) -> PermissionSet {
        let subject = self.player_permission_state(uuid).unwrap_or_default();
        self.permission_groups
            .effective_permissions(subject.groups(), subject.overrides())
    }

    /// Atomically edits one player's persisted permission state.
    ///
    /// Persistence completes before the cache is published. An online player is
    /// refreshed from the latest cached snapshot at the server job tick stage.
    ///
    /// # Errors
    ///
    /// Returns an edit error, an unknown newly assigned group, or a storage error.
    pub async fn try_update_player_permissions<T, E>(
        self: &Arc<Self>,
        uuid: Uuid,
        update: impl FnOnce(PermissionSubjectState) -> Result<(PermissionSubjectState, T), E> + Send,
    ) -> Result<(PermissionSubjectState, T), PlayerPermissionUpdateError<E>>
    where
        T: Send,
        E: Send,
    {
        let _guard = self.player_permission_updates.lock().await;
        let mut states = self.player_permission_states.read().clone();
        let current = states.get(uuid).cloned().unwrap_or_default();
        let previous_groups = current.groups().to_vec();
        let (updated, result) = update(current).map_err(PlayerPermissionUpdateError::Edit)?;
        validate_player_permission_group_update(
            &self.permission_groups,
            &previous_groups,
            updated.groups(),
        )?;

        if updated.is_empty() {
            states.remove(uuid);
        } else {
            states.set(uuid, updated.clone());
        }
        self.player_data_storage
            .save_permission_subjects(&states)
            .await?;

        *self.player_permission_states.write() = states;
        self.queue_player_permission_refresh(uuid);
        Ok((updated, result))
    }

    /// Replaces the complete permission group config and refreshes online players.
    ///
    /// # Errors
    ///
    /// Returns an error when validation or persistence fails.
    pub async fn replace_permission_groups(
        self: &Arc<Self>,
        config: PermissionGroupsConfig,
    ) -> Result<(), PermissionGroupManagerError> {
        self.permission_groups.replace_config(config).await?;
        self.queue_online_permission_group_refresh();
        Ok(())
    }

    /// Edits the latest permission group config and refreshes online players.
    ///
    /// # Errors
    ///
    /// Returns an error when validation or persistence fails.
    pub async fn update_permission_groups(
        self: &Arc<Self>,
        update: impl FnOnce(&mut PermissionGroupsConfig) + Send,
    ) -> Result<(), PermissionGroupManagerError> {
        self.permission_groups.update_config(update).await?;
        self.queue_online_permission_group_refresh();
        Ok(())
    }

    /// Applies a fallible permission group edit and refreshes online players.
    ///
    /// # Errors
    ///
    /// Returns the caller edit error or a validation/persistence error.
    pub async fn try_update_permission_groups<T, E>(
        self: &Arc<Self>,
        update: impl FnOnce(&mut PermissionGroupsConfig) -> Result<T, E> + Send,
    ) -> Result<T, PermissionGroupUpdateError<E>>
    where
        T: Send,
        E: Send,
    {
        let result = self.permission_groups.try_update_config(update).await?;
        self.queue_online_permission_group_refresh();
        Ok(result)
    }

    fn queue_player_permission_refresh(self: &Arc<Self>, uuid: Uuid) {
        self.jobs
            .spawn(FnServerJob::new(move |context: &mut ServerJobContext| {
                if let Some(server) = context.server() {
                    server.refresh_player_permission_state(uuid);
                }
            }));
    }

    pub(crate) fn refresh_player_permission_state(self: &Arc<Self>, uuid: Uuid) {
        let Some(player) = self.online_players.get_by_uuid(&uuid) else {
            return;
        };
        let state = self.player_permission_state(uuid).unwrap_or_default();
        self.apply_player_permission_state(&player, state);
        self.resend_player_permission_context(&player);
    }

    fn queue_online_permission_group_refresh(self: &Arc<Self>) {
        self.jobs
            .spawn(FnServerJob::new(|context: &mut ServerJobContext| {
                if let Some(server) = context.server() {
                    server.refresh_online_permission_groups();
                }
            }));
    }

    fn refresh_online_permission_groups(self: &Arc<Self>) {
        for player in self.get_players() {
            let state = self
                .player_permission_state(player.gameprofile.id())
                .unwrap_or_default();
            self.apply_player_permission_state(&player, state);
            self.resend_player_permission_context(&player);
        }
    }
}

/// Vanilla `PlayerList.canPlayerLogin` outcome.
impl Server {
    /// Resolves whether a verified login may join, returning the vanilla kick reason.
    ///
    /// Mirrors `PlayerList.canPlayerLogin`: player ban, allowlist, IP ban, then the
    /// player limit with the operator bypass. A `None` address skips the IP ban check,
    /// which only happens for connections that never exposed a socket address.
    #[must_use]
    pub fn check_login(&self, address: Option<SocketAddr>, uuid: Uuid) -> Option<TextComponent> {
        if let Some(ban) = self.bans().get(uuid) {
            return Some(ban_kick_reason(
                &translations::MULTIPLAYER_DISCONNECT_BANNED_REASON,
                &ban,
            ));
        }
        if !self.is_whitelisted(uuid) {
            return Some(
                translations::MULTIPLAYER_DISCONNECT_NOT_WHITELISTED
                    .msg()
                    .into(),
            );
        }
        if let Some(address) = address
            && let Some(ban) = self.ip_bans().get(address)
        {
            return Some(ban_kick_reason(
                &translations::MULTIPLAYER_DISCONNECT_BANNED_IP_REASON,
                &ban,
            ));
        }
        if self.online_players.len() >= self.config.max_players as usize
            && !self.can_bypass_player_limit(uuid)
        {
            return Some(
                translations::MULTIPLAYER_DISCONNECT_SERVER_FULL
                    .msg()
                    .into(),
            );
        }
        None
    }

    /// Applies an operator grant to the live player and the persisted list.
    ///
    /// Vanilla `PlayerList.op` refreshes the client level immediately; Steel also
    /// assigns the built-in operator group so group-based configuration agrees.
    pub async fn op_player(self: &Arc<Self>, uuid: Uuid, name: &str) -> bool {
        let changed = self.add_operator(uuid, name).await;
        self.refresh_player_permission_state(uuid);
        changed
    }

    /// Revokes operator state and refreshes the live player.
    pub async fn deop_player(self: &Arc<Self>, uuid: Uuid) -> bool {
        let changed = self.remove_operator(uuid).await;
        self.refresh_player_permission_state(uuid);
        changed
    }

    /// Returns the vanilla client permission-level event for one player.
    #[must_use]
    pub fn client_permission_status(&self, uuid: Uuid) -> EntityStatus {
        self.profile_permission_set(uuid)
            .level()
            .client_entity_status()
    }
}

/// Builds the vanilla ban kick message.
fn ban_kick_reason(translation: &Translation<1>, ban: &impl BanReason) -> TextComponent {
    let mut reason: TextComponent = translation.message([ban.ban_reason_message()]).into();
    if let Some(expires) = ban.ban_expires() {
        reason = reason.add_child(
            translations::MULTIPLAYER_DISCONNECT_BANNED_EXPIRATION
                .message([TextComponent::plain(format_ban_expiry(expires))])
                .component(),
        );
    }
    reason
}

/// Shared accessors for player and IP ban entries.
trait BanReason {
    /// Vanilla `BanListEntry.getReasonMessage`.
    fn ban_reason_message(&self) -> TextComponent;

    /// Returns the expiration, or `None` for a permanent ban.
    fn ban_expires(&self) -> Option<SystemTime>;
}

impl BanReason for UserBanEntry {
    fn ban_reason_message(&self) -> TextComponent {
        UserBanEntry::reason_message(self)
    }

    fn ban_expires(&self) -> Option<SystemTime> {
        UserBanEntry::expires(self)
    }
}

impl BanReason for IpBanEntry {
    fn ban_reason_message(&self) -> TextComponent {
        IpBanEntry::reason_message(self)
    }

    fn ban_expires(&self) -> Option<SystemTime> {
        IpBanEntry::expires(self)
    }
}
