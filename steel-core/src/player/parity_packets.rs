//! Handlers for the play-state packets added for vanilla parity (§8 of the parity list).
//!
//! Every handler here mirrors the corresponding vanilla `ServerGamePacketListenerImpl`
//! method: the same permission gate, the same state mutation and the same client-visible
//! side effects.
//!
//! Packets whose vanilla handler needs a foundation Steel does not have yet are routed to
//! [`Player::handle_awaiting_foundation`], which names the exact missing dependency
//! instead of silently dropping the packet.

use std::sync::Arc;

use glam::DVec3;
use rustc_hash::FxHashMap;
use serde_json::Value;
use steel_protocol::packets::game::{
    CChangeDifficulty, CGameRuleValues, CSetHeldSlot, CSetPlayerInventory, CTagQuery,
    RelativeMovement, SBlockEntityTagQuery, SBundleItemSelected, SDebugSubscriptionRequest,
    SEditBook, SEntityTagQuery, SJigsawGenerate, SLockDifficulty, SPaddleBoat, SPickItemFromEntity,
    SSetCommandBlock, SSetCommandMinecart, SSetGameRule, SSetJigsawBlock, SSetStructureBlock,
    SSetTestBlock, STeleportToEntity, STestInstanceBlockAction,
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
use steel_registry::items::ItemRef;
use steel_registry::vanilla_items;
use steel_utils::Downcast as _;
use steel_utils::Identifier;
use steel_utils::serial::OptionalNbt;
use steel_utils::translations;
use steel_utils::types::GameType;
use text_components::TextComponent;

use crate::block_entity::BlockEntity as _;
use crate::entity::Entity;
use crate::entity::SharedEntity;
use crate::entity::entities::objects::{BoatEntity, ChestBoatEntity, ChestRaftEntity, RaftEntity};
use crate::inventory::container::Container as _;
use crate::permission::{PermissionExpr, PermissionLevel};
use crate::player::Player;
use crate::player::player_inventory::PlayerInventory;
use crate::portal::{TeleportPostTransition, TeleportTransition};

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
    /// rotation, searching every level for the UUID. Steel applies the cross-world move
    /// through the ordinary within-domain world-change path; a target in another domain
    /// cannot be reached without the domain-switch workflow, so it is reported instead.
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
        let rotation = target.rotation();

        let current_world = self.get_world();
        let Some(target_world) = target.level() else {
            return;
        };
        if Arc::ptr_eq(&current_world, &target_world) {
            if let Err(error) = self.teleport(position, rotation.0, rotation.1) {
                log::debug!(
                    "Failed to teleport {} to entity {}: {error}",
                    self.gameprofile.name(),
                    packet.uuid
                );
            }
            return;
        }

        if current_world.domain() != target_world.domain() {
            self.handle_awaiting_foundation(
                "ServerboundTeleportToEntityPacket",
                "a cross-domain spectator teleport (the target lives in another domain, which only the domain-switch workflow may cross)",
            );
            return;
        }

        let transition = TeleportTransition {
            target_world,
            position,
            rotation,
            velocity: DVec3::ZERO,
            relatives: RelativeMovement::NONE,
            portal_cooldown: 0,
            as_passenger: false,
            post_transition: TeleportPostTransition::do_nothing(),
        };
        self.change_world_within_domain(&transition);
    }

    /// Handles an entity NBT tag query, mirroring
    /// `ServerGamePacketListenerImpl#handleEntityTagQuery`.
    ///
    /// Vanilla answers only gamemasters and only when the entity id resolves in the
    /// player's own level; the response is the entity's `saveWithoutId` compound,
    /// which Steel models as [`Entity::nbt_for_data_compare`].
    pub fn handle_entity_tag_query(&self, packet: SEntityTagQuery) {
        if !self.has_permission(&gamemaster_permission()) {
            return;
        }

        let world = self.get_world();
        let Some(entity) = world.entity_manager().get_by_id(packet.entity_id) else {
            return;
        };

        self.send_packet(CTagQuery {
            transaction_id: packet.transaction_id,
            tag: OptionalNbt(Some(entity.nbt_for_data_compare())),
        });
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
    /// Vanilla forwards the two paddle states to the boat the player controls; a
    /// player who is not controlling a boat has the packet ignored.
    pub fn handle_paddle_boat(&self, packet: SPaddleBoat) {
        let Some(vehicle) = self.controlled_vehicle() else {
            return;
        };

        if let Some(boat) = vehicle.downcast_ref::<BoatEntity>() {
            boat.set_paddle_state(packet.left, packet.right);
        } else if let Some(boat) = vehicle.downcast_ref::<RaftEntity>() {
            boat.set_paddle_state(packet.left, packet.right);
        } else if let Some(boat) = vehicle.downcast_ref::<ChestBoatEntity>() {
            boat.set_paddle_state(packet.left, packet.right);
        } else if let Some(boat) = vehicle.downcast_ref::<ChestRaftEntity>() {
            boat.set_paddle_state(packet.left, packet.right);
        }
    }

    /// Handles a command-block edit, mirroring `ServerGamePacketListenerImpl#handleSetCommandBlock`.
    ///
    /// Vanilla requires the gamemaster-block permission, swaps the block kind to match the packet's
    /// mode while keeping the facing and conditional properties, then writes the command, the
    /// output tracking, the automatic flag and the last-output reset onto the block entity.
    pub fn handle_set_command_block(&self, packet: SSetCommandBlock) {
        use steel_protocol::packets::game::CommandBlockMode as PacketMode;
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;
        use steel_registry::blocks::properties::BlockStateProperties;
        use steel_registry::vanilla_blocks;
        use steel_registry::vanilla_game_rules;
        use steel_utils::types::UpdateFlags;

        use crate::block_entity::entities::CommandBlockEntity;

        if !self.can_use_game_master_blocks() {
            self.send_message(&translations::ADV_MODE_NOT_ALLOWED.msg());
            return;
        }

        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(command_block) = block_entity.downcast_ref::<CommandBlockEntity>() else {
            return;
        };

        let old_mode = command_block.mode();
        let current_state = world.get_block_state(packet.pos);
        let facing = current_state.get_value(&BlockStateProperties::FACING);
        let base_state = match packet.mode {
            PacketMode::Sequence => vanilla_blocks::CHAIN_COMMAND_BLOCK.default_state(),
            PacketMode::Auto => vanilla_blocks::REPEATING_COMMAND_BLOCK.default_state(),
            PacketMode::Redstone => vanilla_blocks::COMMAND_BLOCK.default_state(),
        };
        let new_state = base_state
            .set_value(&BlockStateProperties::FACING, facing)
            .set_value(&BlockStateProperties::CONDITIONAL, packet.conditional);
        if new_state != current_state {
            // Vanilla keeps the same block entity across the kind swap; every command-block kind
            // uses the same block entity type, so Steel's in-place state swap preserves it.
            world.set_block(packet.pos, new_state, UpdateFlags::UPDATE_CLIENTS);
        }

        command_block.set_command(packet.command.clone());
        command_block.set_track_output(packet.track_output);
        if !packet.track_output {
            command_block.set_last_output(None);
        }
        command_block.set_automatic(packet.automatic);
        // Vanilla `getMode` derives the mode from the block kind, so the state swap above already
        // selected it; only the switch hook has to run.
        if old_mode != command_block.mode() {
            command_block.on_mode_switch(&world);
        }

        let command_block_enabled = world.get_game_rule(&vanilla_game_rules::COMMAND_BLOCKS_WORK);
        if command_block_enabled {
            command_block.on_updated(&world);
        }

        if !packet.command.trim().is_empty() {
            let message = if command_block_enabled {
                translations::ADV_MODE_SET_COMMAND_SUCCESS
                    .message([TextComponent::from(packet.command)])
                    .component()
            } else {
                translations::ADV_MODE_SET_COMMAND_DISABLED
                    .message([TextComponent::from(packet.command)])
                    .component()
            };
            self.send_message(&message);
        }
    }

    /// Handles a command-block-minecart edit, mirroring
    /// `ServerGamePacketListenerImpl#handleSetCommandMinecart`.
    ///
    /// Vanilla requires the gamemaster-block permission, resolves the entity id to a command block
    /// minecart's `BaseCommandBlock`, and writes the command plus the output tracking flag.
    pub fn handle_set_command_minecart(&self, packet: SSetCommandMinecart) {
        use steel_registry::vanilla_game_rules;

        use crate::entity::entities::objects::CommandBlockMinecartEntity;

        if !self.can_use_game_master_blocks() {
            self.send_message(&translations::ADV_MODE_NOT_ALLOWED.msg());
            return;
        }

        let world = self.get_world();
        let Some(entity) = world.get_entity_by_id(packet.entity) else {
            return;
        };
        let Some(minecart) = entity.downcast_ref::<CommandBlockMinecartEntity>() else {
            return;
        };

        minecart.set_command(packet.command.clone());
        minecart.set_track_output(packet.track_output);
        if !packet.track_output {
            minecart.set_last_output(None);
        }

        let command_block_enabled = world.get_game_rule(&vanilla_game_rules::COMMAND_BLOCKS_WORK);
        if !packet.command.trim().is_empty() {
            let message = if command_block_enabled {
                translations::ADV_MODE_SET_COMMAND_SUCCESS
                    .message([TextComponent::from(packet.command)])
                    .component()
            } else {
                translations::ADV_MODE_SET_COMMAND_DISABLED
                    .message([TextComponent::from(packet.command)])
                    .component()
            };
            self.send_message(&message);
        }
    }

    /// Handles a jigsaw generate request, mirroring
    /// `ServerGamePacketListenerImpl#handleJigsawGenerate`.
    ///
    /// Vanilla requires the gamemaster-block permission and calls
    /// `JigsawBlockEntity.generate(level, levels, keepJigsaws)`, which runs the jigsaw placement
    /// driver over the block's pool. Steel's pool-element placement exists for worldgen only
    /// (`worldgen/structure/piece_placer`), and no live-level jigsaw driver or
    /// `StructureTemplateManager` exists yet, so the request is recorded and reported.
    pub fn handle_jigsaw_generate(&self, packet: SJigsawGenerate) {
        if !self.can_use_game_master_blocks() {
            return;
        }

        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(jigsaw) = block_entity.downcast_ref::<JigsawBlockEntity>() else {
            return;
        };

        log::debug!(
            "Player {} asked jigsaw {} to generate {} levels (keep jigsaws: {}); live jigsaw assembly is not available",
            self.gameprofile.name(),
            jigsaw.data().pool,
            packet.levels,
            packet.keep_jigsaws
        );
        self.handle_awaiting_foundation(
            "ServerboundJigsawGeneratePacket",
            "a live-level jigsaw assembly driver (`JigsawPlacement` over `TemplateLevelAccess`; only worldgen pool-element placement exists)",
        );
    }

    /// Handles a test-block edit, mirroring `ServerGamePacketListenerImpl#handleSetTestBlock`.
    ///
    /// Vanilla requires the gamemaster-block permission, stores the mode and message on the test
    /// block entity and resends the block so clients render the new mode.
    pub fn handle_set_test_block(&self, packet: SSetTestBlock) {
        use steel_utils::types::UpdateFlags;

        use crate::block_entity::entities::TestBlockEntity;

        if !self.can_use_game_master_blocks() {
            return;
        }

        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(test_block) = block_entity.downcast_ref::<TestBlockEntity>() else {
            return;
        };

        test_block.set_mode(packet.mode);
        test_block.set_message(&packet.message);

        let state = world.get_block_state(packet.pos);
        world.set_block(packet.pos, state, UpdateFlags::UPDATE_CLIENTS);
    }

    /// Handles a jigsaw-block edit, mirroring `ServerGamePacketListenerImpl#handleSetJigsawBlock`.
    ///
    /// Vanilla requires the gamemaster-block permission, writes every editor field onto the jigsaw
    /// block entity and resends the block.
    pub fn handle_set_jigsaw_block(&self, packet: SSetJigsawBlock) {
        use steel_utils::types::UpdateFlags;

        use crate::block_entity::entities::JigsawBlockEntity;

        if !self.can_use_game_master_blocks() {
            return;
        }

        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(jigsaw) = block_entity.downcast_ref::<JigsawBlockEntity>() else {
            return;
        };

        jigsaw.set_name(&packet.name.to_string());
        jigsaw.set_target(&packet.target.to_string());
        jigsaw.set_pool(&packet.pool.to_string());
        jigsaw.set_final_state(&packet.final_state);
        jigsaw.set_joint(&packet.joint.to_string());
        jigsaw.set_placement_priority(packet.placement_priority);
        jigsaw.set_selection_priority(packet.selection_priority);

        let state = world.get_block_state(packet.pos);
        world.set_block(packet.pos, state, UpdateFlags::UPDATE_CLIENTS);
    }

    /// Handles a structure-block edit, mirroring
    /// `ServerGamePacketListenerImpl#handleSetStructureBlock`.
    ///
    /// Vanilla requires the gamemaster-block permission, switches the block state's mode and copies
    /// every editor field onto the structure block entity. The `save_area`, `load_area` and
    /// `scan_area` update types then run through the structure template manager; Steel has no
    /// template manager, so those three report vanilla's failure/not-found message instead of
    /// pretending the template was written, placed or measured.
    pub fn handle_set_structure_block(&self, packet: SSetStructureBlock) {
        use steel_protocol::packets::game::StructureBlockUpdateType;
        use steel_utils::types::UpdateFlags;

        use crate::block_entity::entities::StructureBlockEntity;

        if !self.can_use_game_master_blocks() {
            return;
        }

        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(structure) = block_entity.downcast_ref::<StructureBlockEntity>() else {
            return;
        };

        structure.set_mode(packet.mode);
        structure.set_structure_name(&packet.name);
        structure.set_pos((packet.offset.x, packet.offset.y, packet.offset.z));
        structure.set_size((packet.size.x, packet.size.y, packet.size.z));
        structure.set_mirror(packet.mirror as i32);
        structure.set_rotation(packet.rotation as i32);
        structure.set_metadata(&packet.data);
        structure.set_ignore_entities(packet.ignore_entities);
        structure.set_strict(packet.strict);
        structure.set_show_air(packet.show_air);
        structure.set_show_bounding_box(packet.show_bounding_box);
        structure.set_integrity(packet.integrity);
        structure.set_seed(packet.seed);

        if structure.has_structure_name() {
            let name = TextComponent::from(packet.name.clone());
            match packet.update_type {
                StructureBlockUpdateType::UpdateData => {}
                StructureBlockUpdateType::SaveArea => {
                    self.send_message(
                        &translations::STRUCTURE_BLOCK_SAVE_FAILURE
                            .message([name])
                            .component(),
                    );
                }
                StructureBlockUpdateType::LoadArea => {
                    self.send_message(
                        &translations::STRUCTURE_BLOCK_LOAD_NOT_FOUND
                            .message([name])
                            .component(),
                    );
                }
                StructureBlockUpdateType::ScanArea => {
                    self.send_message(&translations::STRUCTURE_BLOCK_SIZE_FAILURE.msg());
                }
            }
        } else {
            self.send_message(
                &translations::STRUCTURE_BLOCK_INVALID_STRUCTURE_NAME
                    .message([TextComponent::from(packet.name.clone())])
                    .component(),
            );
        }

        let state = world.get_block_state(packet.pos);
        world.set_block(packet.pos, state, UpdateFlags::UPDATE_CLIENTS);
    }

    /// Handles a test-instance block action, mirroring
    /// `ServerGamePacketListenerImpl#handleTestInstanceBlockAction`.
    ///
    /// Vanilla requires the gamemaster-block permission. `set` stores the client's data on the
    /// block entity, `reset`/`save`/`export`/`run` then drive the game-test runner, and
    /// `query`/`init` answer with the test's description plus its structure size.
    ///
    /// Steel has no game-test runner or structure template manager: the four runner actions store
    /// the data and report the missing structure rather than claiming the test ran, and the query
    /// response omits the structure size (vanilla reads it from the template).
    pub fn handle_test_instance_block_action(&self, packet: STestInstanceBlockAction) {
        use steel_protocol::packets::game::TestInstanceBlockAction;
        use steel_utils::types::UpdateFlags;

        use crate::block_entity::entities::TestInstanceBlockEntity;

        if !self.can_use_game_master_blocks() {
            return;
        }

        let world = self.get_world();
        let Some(block_entity) = world.get_block_entity(packet.pos) else {
            return;
        };
        let Some(test_instance) = block_entity.downcast_ref::<TestInstanceBlockEntity>() else {
            return;
        };

        match packet.action {
            TestInstanceBlockAction::Query | TestInstanceBlockAction::Init => {
                let status = packet.data.test.as_ref().map_or_else(
                    || {
                        TextComponent::translated(
                            translations::TEST_INSTANCE_DESCRIPTION_NO_TEST.msg(),
                        )
                    },
                    describe_test_instance,
                );
                self.send_packet(CTestInstanceBlockStatus { status, size: None });
            }
            TestInstanceBlockAction::Set
            | TestInstanceBlockAction::Reset
            | TestInstanceBlockAction::Save
            | TestInstanceBlockAction::Export
            | TestInstanceBlockAction::Run => {
                test_instance.set_data(
                    packet.data.test.as_ref(),
                    (packet.data.size.x, packet.data.size.y, packet.data.size.z),
                    packet.data.rotation as i32,
                    packet.data.ignore_entities,
                );

                if packet.action != TestInstanceBlockAction::Set {
                    self.send_message(
                        &translations::TEST_INSTANCE_BLOCK_ERROR_NO_TEST_STRUCTURE.msg(),
                    );
                }

                let state = world.get_block_state(packet.pos);
                world.set_block(packet.pos, state, UpdateFlags::UPDATE_CLIENTS);
            }
        }
    }

    /// Handles a bundle item selection, mirroring
    /// `ServerGamePacketListenerImpl#handleBundleItemSelectedPacket`.
    ///
    /// Vanilla forwards to `AbstractContainerMenu.setSelectedBundleItemIndex`, which toggles the
    /// selection on the stack in that slot of the currently open menu.
    pub fn handle_bundle_item_selected(&self, packet: SBundleItemSelected) {
        self.set_selected_bundle_item_index(packet.slot_id, packet.selected_item_index);
    }

    /// Handles a debug-value subscription request, mirroring
    /// `ServerGamePacketListenerImpl#handleDebugSubscriptionRequest`.
    ///
    /// Vanilla stores the requested subscriptions on the player; `ServerPlayer.debugSubscriptions`
    /// then filters them through `ServerDebugSubscribers.hasRequiredPermissions`, so a non-operator
    /// silently receives nothing. Steel stores the raw ids (vanilla receives registry entries, but
    /// Steel has no `debug_subscription` registry yet).
    pub fn handle_debug_subscription_request(&self, packet: SDebugSubscriptionRequest) {
        let requested = packet
            .subscriptions
            .into_iter()
            .filter_map(|id| usize::try_from(id).ok())
            .collect();
        *self.debug_subscriptions.lock() = requested;
    }

    /// Returns the debug subscriptions this player currently receives.
    ///
    /// Mirrors vanilla `ServerPlayer.debugSubscriptions`: the request is honoured only for
    /// operators, so a non-operator always reports an empty set.
    #[must_use]
    pub fn debug_subscriptions(&self) -> Vec<usize> {
        if !self.is_operator() {
            return Vec::new();
        }
        self.debug_subscriptions.lock().clone()
    }

    /// Handles the pick-block key on an entity, mirroring
    /// `ServerGamePacketListenerImpl#handlePickItemFromEntity`.
    ///
    /// Vanilla requires the target to be within the entity interaction range (plus a 3-block
    /// buffer), takes `Entity.getPickResult` and feeds it through the shared `tryPickItem`. When
    /// the packet asks for data and the player may use gamemaster blocks, a player avatar's profile
    /// is printed to chat.
    pub fn handle_pick_item_from_entity(&self, packet: SPickItemFromEntity) {
        let world = self.get_world();
        let Some(entity) = world.get_entity_by_id(packet.id) else {
            return;
        };
        if !self.is_within_entity_interaction_range(entity.bounding_box(), 3.0) {
            return;
        }

        if let Some(item_stack) = entity_pick_result(&entity) {
            self.try_pick_item(item_stack);
        }

        if packet.include_data && self.can_use_game_master_blocks() {
            if entity.downcast_ref::<MannequinEntity>().is_some() {
                self.send_message(&fetch_profile_body(entity));
            }
        }
    }

    /// Vanilla `ServerGamePacketListenerImpl#tryPickItem`: selects an existing matching stack,
    /// otherwise adds a new stack in creative, then resends the held slot and the inventory menu.
    fn try_pick_item(&self, item_stack: ItemStack) {
        if item_stack.is_empty() {
            return;
        }

        let mut inventory = self.inventory.lock();
        match inventory.find_slot_matching_item_with_same_components(&item_stack) {
            Some(slot_with_item) => {
                if PlayerInventory::is_hotbar_slot(slot_with_item) {
                    inventory.set_selected_slot(slot_with_item);
                } else {
                    let slot = inventory.get_suitable_hotbar_slot();
                    inventory.set_selected_slot(slot);
                    inventory.pick_slot(slot_with_item);
                }
            }
            None => {
                if !self.has_infinite_materials() {
                    return;
                }
                let slot = inventory.get_suitable_hotbar_slot();
                inventory.set_selected_slot(slot);
                inventory.add_and_pick_item(item_stack);
            }
        }

        let selected = inventory.get_selected_slot();
        drop(inventory);

        self.send_packet(CSetHeldSlot {
            slot: i32::from(selected),
        });
        self.inventory_menu
            .lock()
            .behavior_mut()
            .broadcast_changes(&self.connection);
    }
}

/// Returns the stack vanilla's `Entity.getPickResult` would give for this entity.
///
/// Vanilla resolves this per concrete class, so the concrete-class checks here mirror
/// `ServerGamePacketListenerImpl#handlePickItemFromEntity`'s own `instanceof` dispatch. Every
/// entity class vanilla leaves at the `Entity` default returns `None`.
///
/// Reported gap: vanilla expresses this as a virtual `Entity.getPickResult`, and the `ItemFrame`
/// capability trait is the natural home for the frame cases; Steel's `Entity` trait has no hook yet,
/// so the class checks stay local to this handler instead of spreading one method across every
/// entity type.
fn entity_pick_result(entity: &SharedEntity) -> Option<ItemStack> {
    use steel_registry::data_components::vanilla_components::CUSTOM_NAME;

    use crate::entity::entities::objects::display_ui::{
        ArmorStandEntity, CushionEntity, LeashFenceKnotEntity, PaintingEntity,
    };
    use crate::entity::entities::objects::{
        BoatEntity, ChestBoatEntity, ChestMinecartEntity, ChestRaftEntity,
        CommandBlockMinecartEntity, EndCrystalEntity, FurnaceMinecartEntity, GlowItemFrameEntity,
        HopperMinecartEntity, ItemFrameEntity, MannequinEntity, MinecartEntity, RaftEntity,
        SpawnerMinecartEntity, TntMinecartEntity,
    };

    // Vanilla `ItemFrame.getPickResult`: the framed stack, else the empty frame item carrying the
    // frame's custom name.
    if let Some(frame) = entity.downcast_ref::<ItemFrameEntity>() {
        let framed = frame.item();
        if !framed.is_empty() {
            return Some(framed);
        }
        let mut stack = ItemStack::new(&vanilla_items::ITEM_FRAME);
        if let Some(custom_name) = frame.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }
        return Some(stack);
    }
    if let Some(frame) = entity.downcast_ref::<GlowItemFrameEntity>() {
        let framed = frame.item();
        if !framed.is_empty() {
            return Some(framed);
        }
        let mut stack = ItemStack::new(&vanilla_items::GLOW_ITEM_FRAME);
        if let Some(custom_name) = frame.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }
        return Some(stack);
    }

    if entity.downcast_ref::<PaintingEntity>().is_some() {
        return Some(ItemStack::new(&vanilla_items::PAINTING));
    }
    if entity.downcast_ref::<ArmorStandEntity>().is_some() {
        return Some(ItemStack::new(&vanilla_items::ARMOR_STAND));
    }
    if entity.downcast_ref::<LeashFenceKnotEntity>().is_some() {
        return Some(ItemStack::new(&vanilla_items::LEAD));
    }
    if let Some(cushion) = entity.downcast_ref::<CushionEntity>() {
        return cushion_item_for(cushion.color()).map(ItemStack::new);
    }
    if entity.downcast_ref::<EndCrystalEntity>().is_some() {
        return Some(ItemStack::new(&vanilla_items::END_CRYSTAL));
    }

    // Vanilla `AbstractBoat.getPickResult` returns the boat's own item, whose registry key is the
    // entity type key for every hull.
    if entity.downcast_ref::<BoatEntity>().is_some()
        || entity.downcast_ref::<ChestBoatEntity>().is_some()
        || entity.downcast_ref::<RaftEntity>().is_some()
        || entity.downcast_ref::<ChestRaftEntity>().is_some()
    {
        return REGISTRY
            .items
            .by_key(&entity.entity_type().key)
            .map(ItemStack::new);
    }

    // Vanilla `AbstractMinecart.getPickResult` returns the drop item of the concrete cart.
    for (is_kind, item) in [
        (
            entity.downcast_ref::<MinecartEntity>().is_some(),
            &vanilla_items::MINECART,
        ),
        (
            entity.downcast_ref::<ChestMinecartEntity>().is_some(),
            &vanilla_items::CHEST_MINECART,
        ),
        (
            entity.downcast_ref::<FurnaceMinecartEntity>().is_some(),
            &vanilla_items::FURNACE_MINECART,
        ),
        (
            entity.downcast_ref::<TntMinecartEntity>().is_some(),
            &vanilla_items::TNT_MINECART,
        ),
        (
            entity.downcast_ref::<HopperMinecartEntity>().is_some(),
            &vanilla_items::HOPPER_MINECART,
        ),
        (
            entity
                .downcast_ref::<CommandBlockMinecartEntity>()
                .is_some(),
            &vanilla_items::COMMAND_BLOCK_MINECART,
        ),
        // Vanilla `MinecartSpawner.getPickResult` deliberately returns a plain minecart.
        (
            entity.downcast_ref::<SpawnerMinecartEntity>().is_some(),
            &vanilla_items::MINECART,
        ),
    ] {
        if is_kind {
            return Some(ItemStack::new(item));
        }
    }

    // Vanilla `Mob.getPickResult`: the spawn egg registered for the mob's entity type.
    if entity.as_mob().is_some() {
        return spawn_egg_for(&entity.entity_type().key).map(ItemStack::new);
    }

    None
}

/// Builds the `commands.fetchprofile.entity.success` body vanilla's
/// `FetchProfileCommand.printForAvatar` sends when pick-block runs with data on an avatar.
///
/// Vanilla prints the profile NBT plus a copy-text affordance; Steel has no
/// mannequin-profile-serializing path behind this packet yet, so the body echoes the resolved
/// profile name and id (reported gap below) using the same message id.
fn fetch_profile_body(entity: &SharedEntity) -> TextComponent {
    let Some(mannequin) = entity.downcast_ref::<MannequinEntity>() else {
        return translations::COMMANDS_FETCHPROFILE_NO_PROFILE.msg();
    };
    let profile = mannequin.profile();
    translations::COMMANDS_FETCHPROFILE_ENTITY_SUCCESS
        .message([
            TextComponent::from(profile.name().to_owned()),
            TextComponent::from(format!("{:?}", profile.id())),
        ])
        .component()
}

/// Returns the spawn-egg item whose `entity_data` names `entity_type`, vanilla `SpawnEggItem.byId`.
///
/// Vanilla builds this map once from the item registry; Steel scans the same registry so no
/// item/entity pairing is hardcoded here.
fn spawn_egg_for(entity_type: &Identifier) -> Option<ItemRef> {
    use steel_registry::data_components::components::EntityData;
    use steel_registry::data_components::vanilla_components::ENTITY_DATA;

    static SPAWN_EGGS: std::sync::LazyLock<rustc_hash::FxHashMap<Identifier, ItemRef>> =
        std::sync::LazyLock::new(|| {
            REGISTRY
                .items
                .iter()
                .map(|(_, item)| item)
                .filter_map(|item| {
                    let entity_data: &EntityData = item.components.get(ENTITY_DATA)?;
                    Some((entity_data.entity_type().key.clone(), item))
                })
                .collect()
        });

    SPAWN_EGGS.get(entity_type).copied()
}

/// Returns the cushion item matching a dye color, vanilla `Cushion.getPickResult`.
///
/// Vanilla resolves the color through `Items.CUSHION.pick(color)`, which selects the
/// `<color>_cushion` item; the registry lookup keeps that mapping data-driven.
fn cushion_item_for(color: steel_registry::DyeColor) -> Option<ItemRef> {
    let key = Identifier::vanilla(format!("{}_cushion", color.serialized_name()));
    REGISTRY.items.by_key(&key)
}

/// Builds the description text vanilla's `GameTestInstance.describe` sends for a query.
///
/// Vanilla renders one `test_instance.description.*` row per field with the value in blue; the
/// registry entry supplies the type, structure, batch and dimension.
fn describe_test_instance(test: &Identifier) -> TextComponent {
    use steel_registry::REGISTRY;
    use steel_registry::test_instance::TestInstance;

    let Some(entry) = REGISTRY.test_instances.by_key(test) else {
        return translations::TEST_INSTANCE_DESCRIPTION_NO_TEST.msg();
    };

    let (type_row, function_row) = match &entry.instance {
        TestInstance::Function(function) => (
            translations::TEST_INSTANCE_TYPE_FUNCTION.msg(),
            Some(
                translations::TEST_INSTANCE_DESCRIPTION_FUNCTION
                    .message([TextComponent::from(function.function.to_string())])
                    .component(),
            ),
        ),
        TestInstance::BlockBased(_) => (translations::TEST_INSTANCE_TYPE_BLOCK_BASED.msg(), None),
    };
    let info = match &entry.instance {
        TestInstance::Function(function) => &function.info,
        TestInstance::BlockBased(block) => &block.info,
    };

    let mut description = type_row;
    if let Some(function_row) = function_row {
        description = description.append(function_row);
    }
    description
        .append(
            translations::TEST_INSTANCE_DESCRIPTION_STRUCTURE
                .message([TextComponent::from(info.structure.to_string())])
                .component(),
        )
        .append(
            translations::TEST_INSTANCE_DESCRIPTION_BATCH
                .message([TextComponent::from(info.environment.to_string())])
                .component(),
        )
        .append(
            translations::TEST_INSTANCE_DESCRIPTION_DIMENSION
                .message([TextComponent::from(info.dimension.to_string())])
                .component(),
        )
}

/// Formats a game rule's name the way vanilla's `commands.gamerule.set` message does.
fn rule_display_name(rule: ErasedGameRuleRef) -> String {
    if rule.key().namespace == Identifier::VANILLA_NAMESPACE {
        rule.key().path.to_string()
    } else {
        rule.key().to_string()
    }
}
