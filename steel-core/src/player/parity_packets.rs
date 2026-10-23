//! Handlers for the play-state packets added for vanilla parity (§8 of the parity list).
//!
//! Every handler here mirrors the corresponding vanilla `ServerGamePacketListenerImpl`
//! method: the same permission gate, the same state mutation and the same client-visible
//! side effects.
//!
//! Packets whose vanilla handler needs a foundation Steel does not have yet are routed to
//! [`Player::handle_awaiting_foundation`], which names the exact missing dependency
//! instead of silently dropping the packet.

use serde_json::Value;
use steel_protocol::packets::game::{
    CChangeDifficulty, CSetPlayerInventory, CTagQuery, SBlockEntityTagQuery, SEditBook,
    SLockDifficulty, SPaddleBoat, SSetGameRule, STeleportToEntity,
};
use steel_registry::REGISTRY;
use steel_registry::data_components::components::{
    Filterable, WritableBookContent, WrittenBookContent,
};
use steel_registry::data_components::vanilla_components::{
    WRITABLE_BOOK_CONTENT, WRITTEN_BOOK_CONTENT,
};
use steel_registry::game_rules::{ErasedGameRuleRef, GameRuleType};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_items;
use steel_utils::Identifier;
use steel_utils::serial::OptionalNbt;
use steel_utils::translations;
use steel_utils::types::GameType;
use text_components::TextComponent;

use crate::block_entity::BlockEntity as _;
use crate::inventory::container::Container as _;
use crate::permission::{PermissionExpr, PermissionLevel};
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;

/// Vanilla's `Permissions.COMMANDS_GAMEMASTER` (permission level 2).
fn gamemaster_permission() -> PermissionExpr {
    PermissionExpr::Level(PermissionLevel::Gamemasters)
}

impl Player {
    /// Reports a packet whose vanilla handler needs a foundation Steel does not have yet.
    ///
    /// Vanilla handles these packets with real state mutations; Steel decodes them so the
    /// wire contract is complete, then names the missing dependency here instead of
    /// pretending the mutation happened.
    pub fn handle_awaiting_foundation(&self, packet: &'static str, dependency: &'static str) {
        log::debug!(
            "Player {} sent {packet}, which is decoded but not applied: {dependency} is missing",
            self.gameprofile.name()
        );
    }

    /// Handles a client game-rule edit, mirroring `ServerGamePacketListenerImpl#handleSetGameRule`.
    ///
    /// Vanilla requires the gamemaster permission, resolves each key against the
    /// `game_rule` registry, deserializes the value with the rule's own argument type,
    /// applies it to the level and notifies every operator of the change.
    pub fn handle_set_game_rule(&self, packet: SSetGameRule) {
        if !self.has_permission(&gamemaster_permission()) {
            log::warn!(
                "Player {} tried to set game rule values without required permissions",
                self.gameprofile.name()
            );
            return;
        }

        let world = self.get_world();
        for entry in packet.entries {
            let Some(rule) = REGISTRY.game_rules.by_key(&entry.game_rule) else {
                log::warn!(
                    "Received request to set unknown game rule: {}",
                    entry.game_rule
                );
                continue;
            };

            // Vanilla parses with the rule's Brigadier argument type, so a bool rule only
            // accepts `true`/`false` and an int rule only accepts an integer.
            let parsed = match rule.value_type() {
                GameRuleType::Bool => entry.value.parse::<bool>().ok().map(Value::Bool),
                GameRuleType::Int => entry.value.parse::<i32>().ok().map(Value::from),
            };
            let Some(value) = parsed.and_then(|parsed| rule.deserialize_erased_value(&parsed))
            else {
                log::warn!(
                    "Failed to deserialize game rule value {} for {}",
                    entry.value,
                    entry.game_rule
                );
                continue;
            };

            if !world.set_erased_game_rule(rule, value) {
                log::warn!("Game rule {} rejected its parsed value", entry.game_rule);
                continue;
            }
            self.broadcast_game_rule_change(rule, &entry.value);
        }
    }

    /// Notifies operators of a game-rule change, mirroring
    /// `ServerGamePacketListenerImpl#broadcastGameRuleChangeToOperators`.
    fn broadcast_game_rule_change(&self, rule: ErasedGameRuleRef, serialized: &str) {
        let message = translations::COMMANDS_GAMERULE_SET
            .message([
                TextComponent::from(rule_display_name(rule)),
                TextComponent::from(serialized.to_owned()),
            ])
            .component();
        for player in self.server().get_players() {
            if player.is_operator() {
                player.send_message(&message);
            }
        }
    }

    /// Handles a client difficulty-lock request, mirroring
    /// `ServerGamePacketListenerImpl#handleLockDifficulty`.
    ///
    /// Vanilla requires the gamemaster permission, writes the flag to the level data and
    /// resends the difficulty to every player.
    pub fn handle_lock_difficulty(&self, packet: SLockDifficulty) {
        if !self.has_permission(&gamemaster_permission()) {
            log::warn!(
                "Player {} tried to lock the difficulty without required permissions",
                self.gameprofile.name()
            );
            return;
        }

        let world = self.get_world();
        let (difficulty, locked) = {
            let mut level_data = world.level_data.write();
            level_data.data_mut().difficulty_locked = packet.locked;
            let data = level_data.data();
            (data.difficulty, data.difficulty_locked)
        };
        // Vanilla `MinecraftServer.setDifficultyLocked` resends the difficulty to every
        // player, not just the requester.
        for player in self.server().get_players() {
            player.send_packet(CChangeDifficulty { difficulty, locked });
        }
    }

    /// Handles a spectator teleport-to-entity request, mirroring
    /// `ServerGamePacketListenerImpl#handleTeleportToEntityPacket`.
    ///
    /// Vanilla only serves this to spectators and teleports to the entity's position and
    /// rotation, searching every level for the UUID.
    pub fn handle_teleport_to_entity(&self, packet: STeleportToEntity) {
        if self.game_mode() != GameType::Spectator {
            return;
        }

        let target = self
            .server()
            .worlds
            .values()
            .find_map(|world| world.get_entity_by_uuid(&packet.uuid));
        let Some(target) = target else {
            return;
        };

        let position = target.position();
        let (yaw, pitch) = target.rotation();
        if let Err(error) = self.teleport(position, yaw, pitch) {
            log::debug!(
                "Failed to teleport {} to entity {}: {error}",
                self.gameprofile.name(),
                packet.uuid
            );
        }
    }

    /// Handles a block-entity NBT tag query, mirroring
    /// `ServerGamePacketListenerImpl#handleBlockEntityTagQuery`.
    ///
    /// Vanilla answers only gamemasters, and answers `null` when the position holds no
    /// block entity.
    pub fn handle_block_entity_tag_query(&self, packet: SBlockEntityTagQuery) {
        if !self.has_permission(&gamemaster_permission()) {
            return;
        }

        let tag = self
            .get_world()
            .get_block_entity(packet.pos)
            .map(|block_entity| block_entity.save_without_metadata());
        self.send_packet(CTagQuery {
            transaction_id: packet.transaction_id,
            tag: OptionalNbt(tag),
        });
    }

    /// Handles a writable-book edit, mirroring `ServerGamePacketListenerImpl#handleEditBook`.
    ///
    /// Vanilla accepts hotbar slots and the offhand slot. A packet with a title signs the
    /// book: the writable content is dropped, the item becomes a written book and the
    /// signed content is stored. Without a title only the pages are replaced.
    ///
    /// Vanilla passes the pages through its chat text filter before storing them; Steel has
    /// no chat filtering, so the raw text is stored unfiltered (vanilla's behaviour when
    /// filtering is disabled).
    pub fn handle_edit_book(&self, packet: SEditBook) {
        let Ok(slot) = u8::try_from(packet.slot) else {
            return;
        };
        if !PlayerInventory::is_hotbar_slot(slot)
            && usize::from(slot) != PlayerInventory::SLOT_OFFHAND
        {
            return;
        }
        let slot = usize::from(slot);

        let mut inventory = self.inventory.lock();
        if !inventory.get_item(slot).has(WRITABLE_BOOK_CONTENT) {
            return;
        }

        if let Some(title) = packet.title {
            let pages = packet
                .pages
                .into_iter()
                .map(|page| Filterable::pass_through(TextComponent::plain(page)))
                .collect();
            let Ok(content) = WrittenBookContent::new(
                Filterable::pass_through(title),
                self.gameprofile.name().to_owned(),
                0,
                pages,
                true,
            ) else {
                return;
            };
            let count = inventory.get_item(slot).count();
            let mut book = ItemStack::with_count(&vanilla_items::WRITTEN_BOOK, count);
            book.set(WRITTEN_BOOK_CONTENT, content);
            inventory.set_item(slot, book);
        } else {
            let Ok(content) = WritableBookContent::new(
                packet
                    .pages
                    .into_iter()
                    .map(Filterable::pass_through)
                    .collect(),
            ) else {
                return;
            };
            let mut stack = inventory.get_item(slot).clone();
            stack.set(WRITABLE_BOOK_CONTENT, content);
            inventory.set_item(slot, stack);
        }
        let item_stack = inventory.get_item(slot).clone();
        drop(inventory);

        // Vanilla `ServerPlayer.slotChanged` sends the new stack of a player-inventory slot
        // straight to its owner.
        self.send_packet(CSetPlayerInventory {
            slot: i32::try_from(slot).unwrap_or(0),
            item_stack,
        });
    }

    /// Handles boat paddling input, mirroring `ServerGamePacketListenerImpl#handlePaddleBoat`.
    ///
    /// Vanilla forwards the two paddle states to the boat the player controls. Steel has no
    /// boat entity yet (`AbstractBoat` is absent from `entity/vehicle/`), so the packet is
    /// decoded and reported rather than dropped silently.
    pub fn handle_paddle_boat(&self, _packet: SPaddleBoat) {
        self.handle_awaiting_foundation(
            "ServerboundPaddleBoatPacket",
            "an AbstractBoat entity with `set_paddle_state(left, right)`",
        );
    }
}

/// Formats a game rule's name the way vanilla's `commands.gamerule.set` message does.
fn rule_display_name(rule: ErasedGameRuleRef) -> String {
    if rule.key().namespace == Identifier::VANILLA_NAMESPACE {
        rule.key().path.to_string()
    } else {
        rule.key().to_string()
    }
}
