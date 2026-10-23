//! `VaultBlockEntity` (`net.minecraft.world.level.block.entity.vault.VaultBlockEntity`).
//!
//! A vault holds three data groups in vanilla: `config` (key item, loot table, activation
//! ranges), `shared_data` (display item, connected players, connection particle range) and
//! `server_data` (rewarded players, ejection queue, cooldown). The `VaultState` block state
//! machine cycles connected players, ejects one loot item every 20 ticks and clears the
//! display item when going inactive.

use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, VaultState};
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::sound_events;
use steel_registry::vanilla_level_events;
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_block_entity_types, vanilla_items};
use steel_utils::uuid_ext::UuidExt as _;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex};

use crate::behavior::InventoryAccess;
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::player::Player;
use crate::world::World;

/// Vanilla `VaultConfig.DEFAULT.activationRange`.
pub const VAULT_ACTIVATION_RANGE: f64 = 4.0;
/// Vanilla `VaultConfig.DEFAULT.deactivationRange`.
pub const VAULT_DEACTIVATION_RANGE: f64 = 4.5;
/// Vanilla `VaultBlockEntity.Server.UNLOCKING_DELAY_TICKS`.
pub const VAULT_UNLOCKING_DELAY_TICKS: i64 = 14;
/// Vanilla `VaultState.DELAY_BETWEEN_EJECTIONS_TICKS` /
/// `VaultState.DELAY_AFTER_LAST_EJECTION_TICKS` /
/// `VaultState.DELAY_BEFORE_FIRST_EJECTION_TICKS`.
const EJECTION_PAUSE_TICKS: i64 = 20;
/// Vanilla `VaultState.UPDATE_CONNECTED_PLAYERS_TICK_RATE`.
const CONNECTED_PLAYERS_PAUSE_TICKS: i64 = 20;
/// Vanilla `VaultBlockEntity.Server.DISPLAY_CYCLE_TICK_RATE`.
const DISPLAY_CYCLE_TICK_RATE: i64 = 20;
/// Vanilla `VaultBlockEntity.Server.INSERT_FAIL_SOUND_BUFFER_TICKS`.
const INSERT_FAIL_SOUND_BUFFER_TICKS: i64 = 15;
/// Vanilla `VaultServerData.MAX_REWARD_PLAYERS`.
const MAX_REWARD_PLAYERS: usize = 128;

/// Vanilla `VaultConfig` (the `config` compound).
#[derive(Clone, Debug)]
pub struct VaultConfig {
    /// `loot_table`: the per-player reward table.
    pub loot_table: Identifier,
    /// `activation_range`.
    pub activation_range: f64,
    /// `deactivation_range`.
    pub deactivation_range: f64,
    /// `key_item`: the item consumed to unlock.
    pub key_item: ItemStack,
    /// `override_loot_table_to_display`: display-item override.
    pub override_loot_table_to_display: Option<Identifier>,
}

impl Default for VaultConfig {
    /// Vanilla `VaultConfig.DEFAULT` (`TrialChambersReward`, `4.0`/`4.5`, `TrialKey`).
    fn default() -> Self {
        Self {
            loot_table: Identifier::vanilla_static("chests/trial_chambers/reward"),
            activation_range: VAULT_ACTIVATION_RANGE,
            deactivation_range: VAULT_DEACTIVATION_RANGE,
            key_item: ItemStack::new(&vanilla_items::TRIAL_KEY),
            override_loot_table_to_display: None,
        }
    }
}

impl VaultConfig {
    fn read(view: &NbtCompoundView<'_, '_>) -> Self {
        let default = Self::default();
        Self {
            loot_table: view
                .string("loot_table")
                .and_then(|id| id.to_str().parse::<Identifier>().ok())
                .unwrap_or(default.loot_table),
            activation_range: view
                .get("activation_range")
                .and_then(|tag| tag.codec_f64())
                .unwrap_or(default.activation_range),
            deactivation_range: view
                .get("deactivation_range")
                .and_then(|tag| tag.codec_f64())
                .unwrap_or(default.deactivation_range),
            key_item: view
                .compound("key_item")
                .and_then(|compound| ItemStack::from_borrowed_compound(&compound))
                .unwrap_or(default.key_item),
            override_loot_table_to_display: view
                .string("override_loot_table_to_display")
                .and_then(|id| id.to_str().parse::<Identifier>().ok()),
        }
    }

    fn write(&self, nbt: &mut NbtCompound) {
        nbt.insert("loot_table", self.loot_table.to_string());
        nbt.insert("activation_range", self.activation_range);
        nbt.insert("deactivation_range", self.deactivation_range);
        if !self.key_item.is_empty() {
            nbt.insert("key_item", self.key_item.to_nbt_tag_ref());
        }
        if let Some(override_loot_table) = &self.override_loot_table_to_display {
            nbt.insert(
                "override_loot_table_to_display",
                override_loot_table.to_string(),
            );
        }
    }
}

/// Mutable vault state.
struct VaultStateData {
    config: VaultConfig,
    /// Vanilla `VaultServerData.rewardedPlayers`.
    rewarded_players: Vec<uuid::Uuid>,
    /// Vanilla `VaultServerData.stateUpdatingResumesAt`.
    state_updating_resumes_at: i64,
    /// Vanilla `VaultServerData.itemsToEject`.
    items_to_eject: Vec<ItemStack>,
    /// Vanilla `VaultServerData.totalEjectionsNeeded`.
    total_ejections_needed: i32,
    last_insert_fail_timestamp: i64,
    /// Vanilla `VaultSharedData.displayItem`.
    display_item: ItemStack,
    /// Vanilla `VaultSharedData.connectedPlayers`.
    connected_players: Vec<uuid::Uuid>,
    /// Vanilla `VaultSharedData.connectedParticlesRange`.
    connected_particles_range: f64,
    server_dirty: bool,
    shared_dirty: bool,
}

impl VaultStateData {
    fn new() -> Self {
        Self {
            config: VaultConfig::default(),
            rewarded_players: Vec::new(),
            state_updating_resumes_at: 0,
            items_to_eject: Vec::new(),
            total_ejections_needed: 0,
            last_insert_fail_timestamp: 0,
            display_item: ItemStack::empty(),
            connected_players: Vec::new(),
            connected_particles_range: VAULT_DEACTIVATION_RANGE,
            server_dirty: false,
            shared_dirty: false,
        }
    }

    /// Vanilla `VaultSharedData.setDisplayItem` (only marks dirty on change).
    fn set_display_item(&mut self, item: ItemStack) {
        if !ItemStack::matches(&self.display_item, &item) {
            self.display_item = item.clone();
            self.shared_dirty = true;
        }
    }
}

/// `VaultBlockEntity`.
pub struct VaultBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<VaultStateData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `VaultBlockEntity`.
unsafe impl DowncastType for VaultBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/vault");
}

impl VaultBlockEntity {
    /// Creates a vault block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::VAULT, level, pos, state),
            state: SyncMutex::new(VaultStateData::new()),
        }
    }

    /// Returns whether `player` already received this vault's reward.
    #[must_use]
    pub fn has_rewarded_player(&self, player: &Player) -> bool {
        self.state
            .lock()
            .rewarded_players
            .contains(&player.gameprofile.id())
    }

    /// Vanilla `VaultServerData.addToRewardedPlayers` (capped at 128 entries).
    pub fn add_rewarded_player(&self, player: &Player) {
        let mut state = self.state.lock();
        if state.rewarded_players.contains(&player.gameprofile.id()) {
            return;
        }
        state.rewarded_players.push(player.gameprofile.id());
        if state.rewarded_players.len() > MAX_REWARD_PLAYERS {
            state.rewarded_players.remove(0);
        }
        state.server_dirty = true;
        drop(state);
        BlockEntity::set_changed(self);
    }

    /// Returns the item currently displayed in the vault.
    #[must_use]
    pub fn display_item(&self) -> ItemStack {
        self.state.lock().display_item.clone()
    }

    /// Sets the displayed item.
    pub fn set_display_item(&self, item: ItemStack) {
        let mut state = self.state.lock();
        state.set_display_item(item);
        drop(state);
        BlockEntity::set_changed(self);
    }

    /// Returns whether the vault currently holds a display item.
    #[must_use]
    pub fn has_display_item(&self) -> bool {
        !self.state.lock().display_item.is_empty()
    }

    /// Returns whether the vault's block state is the ominous variant.
    #[must_use]
    pub fn is_ominous(&self) -> bool {
        self.get_block_state()
            .get_value(&BlockStateProperties::OMINOUS)
    }

    /// Vanilla `VaultBlockEntity.Server.tryInsertKey`: validates the key item and starts the
    /// unlocking sequence by moving the block state to `unlocking`.
    ///
    /// Returns `true` when the key was accepted and consumed.
    pub fn try_insert_key(
        &self,
        world: &std::sync::Arc<World>,
        state: BlockStateId,
        player: &Player,
        inserted: &ItemStack,
        inv: &mut InventoryAccess,
    ) -> bool {
        let pos = self.get_block_pos();
        let vault_state = state.get_value(&BlockStateProperties::VAULT_STATE);

        // Vanilla `Server.canEjectReward`: the key item must be non-empty and the state must not
        // be `INACTIVE` (the block behavior already gates on `ACTIVE`, but `tryInsertKey` itself
        // enforces the same rule).
        let needs_reward = {
            let config = self.state.lock().config.clone();
            if config.key_item.is_empty() || vault_state == VaultState::Inactive {
                return false;
            }
            if !ItemStack::is_same_item_same_components(inserted, &config.key_item)
                || inserted.count() < config.key_item.count()
            {
                Self::play_insert_fail_sound(self, world, pos, false);
                return false;
            }
            let rewarded = self.has_rewarded_player(player);
            if rewarded {
                Self::play_insert_fail_sound(self, world, pos, true);
                return false;
            }
            config
        };

        let items = resolve_items_to_eject(world, &needs_reward, pos, player, inserted);
        if items.is_empty() {
            return false;
        }
        inv.with_item(|item| item.shrink(needs_reward.key_item.count()));

        {
            let mut data = self.state.lock();
            data.items_to_eject = items;
            data.total_ejections_needed = data.items_to_eject.len() as i32;
            let display = data
                .items_to_eject
                .last()
                .cloned()
                .unwrap_or_else(ItemStack::empty);
            data.set_display_item(display);
            data.state_updating_resumes_at = world.game_time() + VAULT_UNLOCKING_DELAY_TICKS;
            data.rewarded_players.push(player.gameprofile.id());
            if data.rewarded_players.len() > MAX_REWARD_PLAYERS {
                data.rewarded_players.remove(0);
            }
            data.server_dirty = true;
            let range = data.config.deactivation_range;
            drop(data);
            self.update_connected_players_within_range(world, pos, range);
        }

        world.set_block(
            self.get_block_pos(),
            state.set_value(&BlockStateProperties::VAULT_STATE, VaultState::Unlocking),
            steel_utils::types::UpdateFlags::UPDATE_ALL,
        );
        world.play_sound(
            &sound_events::BLOCK_VAULT_INSERT_ITEM,
            SoundSource::Blocks,
            pos,
            1.0,
            1.0,
            None,
        );
        BlockEntity::set_changed(self);
        true
    }

    /// Vanilla `VaultBlockEntity.Server.playInsertFailSound`.
    fn play_insert_fail_sound(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        rewarded: bool,
    ) {
        let mut state = self.state.lock();
        if world.game_time() < state.last_insert_fail_timestamp + INSERT_FAIL_SOUND_BUFFER_TICKS {
            return;
        }
        state.last_insert_fail_timestamp = world.game_time();
        drop(state);
        world.play_sound(
            if rewarded {
                &sound_events::BLOCK_VAULT_REJECT_REWARDED_PLAYER
            } else {
                &sound_events::BLOCK_VAULT_INSERT_ITEM_FAIL
            },
            SoundSource::Blocks,
            pos,
            1.0,
            1.0,
            None,
        );
    }

    /// Vanilla `VaultSharedData.updateConnectedPlayersWithinRange` (vaults detect all
    /// non-spectator players, excluding already-rewarded ones).
    fn update_connected_players_within_range(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        limit: f64,
    ) {
        let rewarded = self.state.lock().rewarded_players.clone();
        let mut connected = Vec::new();
        world.players.iter_players(|uuid, player| {
            let block_pos = player.block_position();
            let dx = f64::from(block_pos.x() - pos.x());
            let dy = f64::from(block_pos.y() - pos.y());
            let dz = f64::from(block_pos.z() - pos.z());
            if dx * dx + dy * dy + dz * dz < limit * limit
                && !player.is_spectator()
                && !rewarded.contains(uuid)
            {
                connected.push(*uuid);
            }
            true
        });
        let mut state = self.state.lock();
        if state.connected_players != connected {
            state.connected_players = connected;
            state.shared_dirty = true;
        }
    }

    /// Vanilla `VaultState.tickAndGetNext` (`updateStateForConnectedPlayers` variant).
    fn update_state_for_connected_players(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        range: f64,
    ) -> VaultState {
        self.update_connected_players_within_range(world, pos, range);
        let mut state = self.state.lock();
        state.state_updating_resumes_at =
            world.game_time() + CONNECTED_PLAYERS_PAUSE_TICKS;
        let active = !state.connected_players.is_empty();
        state.server_dirty = true;
        drop(state);
        if active { VaultState::Active } else { VaultState::Inactive }
    }

    /// Vanilla `VaultBlockEntity.Server.setVaultState` (`VaultState.onTransition`).
    fn set_vault_state(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        current_state: BlockStateId,
        next: VaultState,
    ) {
        let next_state = current_state.set_value(&BlockStateProperties::VAULT_STATE, next);
        world.set_block(
            pos,
            next_state,
            steel_utils::types::UpdateFlags::UPDATE_ALL,
        );
        let is_ominous = current_state.get_value(&BlockStateProperties::OMINOUS);
        let ominous_param = i32::from(is_ominous);
        let current = current_state.get_value(&BlockStateProperties::VAULT_STATE);

        // Vanilla `VaultState.onExit` for the previous state.
        if current == VaultState::Ejecting {
            world.play_sound(
                &sound_events::BLOCK_VAULT_CLOSE_SHUTTER,
                SoundSource::Blocks,
                pos,
                1.0,
                1.0,
                None,
            );
        }

        // Vanilla `VaultState.onEnter` for the next state.
        match next {
            VaultState::Inactive => {
                let mut state = self.state.lock();
                state.set_display_item(ItemStack::empty());
                drop(state);
                world.level_event(
                    vanilla_level_events::ANIMATION_VAULT_DEACTIVATE,
                    pos,
                    ominous_param,
                    None,
                );
            }
            VaultState::Active => {
                if !self.has_display_item() {
                    self.cycle_display_item_from_loot_table(world, pos);
                }
                world.level_event(
                    vanilla_level_events::ANIMATION_VAULT_ACTIVATE,
                    pos,
                    ominous_param,
                    None,
                );
            }
            VaultState::Unlocking => {
                world.play_sound(
                    &sound_events::BLOCK_VAULT_OPEN_SHUTTER,
                    SoundSource::Blocks,
                    pos,
                    1.0,
                    1.0,
                    None,
                );
            }
            VaultState::Ejecting => {}
        }
    }

    /// Vanilla `VaultBlockEntity.Server.cycleDisplayItemFromLootTable`.
    fn cycle_display_item_from_loot_table(&self, world: &Arc<World>, pos: BlockPos) {
        let (loot_table_id, override_table) = {
            let state = self.state.lock();
            (
                state.config.loot_table.clone(),
                state.config.override_loot_table_to_display.clone(),
            )
        };
        let table_id = override_table.as_ref().unwrap_or(&loot_table_id);
        let display = random_display_item(world, pos, table_id);
        let mut state = self.state.lock();
        state.set_display_item(display);
    }
}

/// Vanilla `VaultBlockEntity.Server.getRandomDisplayItemFromLootTable`.
fn random_display_item(
    world: &Arc<World>,
    pos: BlockPos,
    loot_table_id: &Identifier,
) -> ItemStack {
    let Some(table) = REGISTRY.loot_tables.by_key(loot_table_id) else {
        return ItemStack::empty();
    };
    let mut rng = rand::rng();
    let mut ctx = LootContext::new(&mut rng);
    let results = table.get_random_items(&mut ctx);
    if results.is_empty() {
        return ItemStack::empty();
    }
    util_get_random(&results, &mut rng)
}

/// Vanilla `Util.getRandom(List, RandomSource)`.
fn util_get_random(items: &[ItemStack], rng: &mut impl rand::Rng) -> ItemStack {
    items
        .get(rng.random_range(0..items.len()))
        .cloned()
        .unwrap_or_else(ItemStack::empty)
}

/// Vanilla `VaultBlockEntity.Server.resolveItemsToEject`.
fn resolve_items_to_eject(
    world: &Arc<World>,
    config: &VaultConfig,
    pos: BlockPos,
    player: &Player,
    inserted: &ItemStack,
) -> Vec<ItemStack> {
    let Some(table) = REGISTRY.loot_tables.by_key(&config.loot_table) else {
        return Vec::new();
    };
    let mut rng = rand::rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(f64::from(pos.x()) + 0.5, f64::from(pos.y()), f64::from(pos.z()) + 0.5)
        .with_tool(inserted);
    let _ = (world, player);
    table.get_random_items(&mut ctx)
}

/// Vanilla `VaultState.ejectResultItem`.
fn eject_result_item(world: &Arc<World>, pos: BlockPos, item: ItemStack) {
    use glam::DVec3;
    let spawn_pos = DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 1.2 - 0.125,
        f64::from(pos.z()) + 0.5,
    );
    world.spawn_item_with_velocity(spawn_pos, item, DVec3::new(0.0, 0.2, 0.0));
    world.level_event(
        vanilla_level_events::ANIMATION_VAULT_EJECT_ITEM,
        pos,
        0,
        None,
    );
    world.play_sound(
        &sound_events::BLOCK_VAULT_EJECT_ITEM,
        SoundSource::Blocks,
        pos,
        0.8,
        1.0,
        None,
    );
}

impl BlockEntity for VaultBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();

        if let Some(config) = view.compound("config") {
            state.config = VaultConfig::read(&config);
        } else {
            state.config = VaultConfig::default();
        }

        state.rewarded_players.clear();
        state.items_to_eject.clear();
        state.state_updating_resumes_at = 0;
        state.total_ejections_needed = 0;
        state.display_item = ItemStack::empty();
        state.connected_players.clear();
        state.connected_particles_range = VAULT_DEACTIVATION_RANGE;

        if let Some(server_data) = view.compound("server_data") {
            state.rewarded_players = read_uuid_list(&server_data, "rewarded_players");
            state.state_updating_resumes_at =
                server_data.long("state_updating_resumes_at").unwrap_or(0);
            if let Some(items) = server_data.list("items_to_eject") {
                if let Some(compounds) = items.compounds() {
                    for item in compounds {
                        if let Some(stack) = ItemStack::from_borrowed_compound(&item) {
                            state.items_to_eject.push(stack);
                        }
                    }
                }
            }
            state.total_ejections_needed =
                read_int_or(&server_data, "total_ejections_needed", 0);
        }

        if let Some(shared_data) = view.compound("shared_data") {
            if let Some(item) = shared_data
                .compound("display_item")
                .and_then(|compound| ItemStack::from_borrowed_compound(&compound))
            {
                state.display_item = item;
            }
            state.connected_players = read_uuid_list(&shared_data, "connected_players");
            state.connected_particles_range = shared_data
                .get("connected_particles_range")
                .and_then(|tag| tag.codec_f64())
                .unwrap_or(VAULT_DEACTIVATION_RANGE);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();

        let mut config = NbtCompound::new();
        state.config.write(&mut config);
        nbt.insert("config", config);

        let mut server_data = NbtCompound::new();
        server_data.insert("rewarded_players", write_uuid_list(&state.rewarded_players));
        server_data.insert(
            "state_updating_resumes_at",
            state.state_updating_resumes_at,
        );
        let items: Vec<NbtCompound> = state
            .items_to_eject
            .iter()
            .map(|item| item.to_nbt_tag_ref())
            .filter_map(|tag| match tag {
                simdnbt::owned::NbtTag::Compound(compound) => Some(compound),
                _ => None,
            })
            .collect();
        server_data.insert("items_to_eject", NbtList::Compound(items));
        server_data.insert("total_ejections_needed", state.total_ejections_needed);
        nbt.insert("server_data", server_data);

        let mut shared_data = NbtCompound::new();
        if !state.display_item.is_empty() {
            shared_data.insert("display_item", state.display_item.to_nbt_tag_ref());
        }
        shared_data.insert("connected_players", write_uuid_list(&state.connected_players));
        shared_data.insert(
            "connected_particles_range",
            state.connected_particles_range,
        );
        nbt.insert("shared_data", shared_data);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `VaultBlockEntity.getUpdateTag`: only `shared_data`.
        let state = self.state.lock();
        let mut shared_data = NbtCompound::new();
        if !state.display_item.is_empty() {
            shared_data.insert("display_item", state.display_item.to_nbt_tag_ref());
        }
        shared_data.insert("connected_players", write_uuid_list(&state.connected_players));
        shared_data.insert(
            "connected_particles_range",
            state.connected_particles_range,
        );
        let mut nbt = NbtCompound::new();
        nbt.insert("shared_data", shared_data);
        Some(nbt)
    }

    fn tick(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let current = state.get_value(&BlockStateProperties::VAULT_STATE);

        // Vanilla `VaultBlockEntity.Server.shouldCycleDisplayItem`.
        if world.game_time() % DISPLAY_CYCLE_TICK_RATE == 0 && current == VaultState::Active {
            let can_eject = {
                !self.state.lock().config.key_item.is_empty()
            };
            if can_eject {
                self.cycle_display_item_from_loot_table(world, pos);
            } else {
                let mut data = self.state.lock();
                data.set_display_item(ItemStack::empty());
            }
        }

        // Vanilla `VaultBlockEntity.Server.tick` state advance.
        let resumes_at = self.state.lock().state_updating_resumes_at;
        if world.game_time() >= resumes_at {
            let next = match current {
                VaultState::Inactive => {
                    let range = self.state.lock().config.activation_range;
                    self.update_state_for_connected_players(world, pos, range)
                }
                VaultState::Active => {
                    let range = self.state.lock().config.deactivation_range;
                    self.update_state_for_connected_players(world, pos, range)
                }
                VaultState::Unlocking => {
                    self.state.lock().state_updating_resumes_at =
                        world.game_time() + EJECTION_PAUSE_TICKS;
                    self.state.lock().server_dirty = true;
                    VaultState::Ejecting
                }
                VaultState::Ejecting => {
                    let empty = self.state.lock().items_to_eject.is_empty();
                    if empty {
                        let mut data = self.state.lock();
                        data.total_ejections_needed = 0;
                        data.server_dirty = true;
                        drop(data);
                        let range = self.state.lock().config.deactivation_range;
                        self.update_state_for_connected_players(world, pos, range)
                    } else {
                        let (item, sound_progress) = {
                            let mut data = self.state.lock();
                            let item = data
                                .items_to_eject
                                .pop()
                                .unwrap_or_else(ItemStack::empty);
                            let remaining = data.items_to_eject.len().max(1) as f32;
                            let total = (data.total_ejections_needed.max(1)) as f32;
                            let progress = 1.0 - total.recip() * (remaining - 1.0);
                            let display = data
                                .items_to_eject
                                .last()
                                .cloned()
                                .unwrap_or_else(ItemStack::empty);
                            data.set_display_item(display);
                            data.state_updating_resumes_at =
                                world.game_time() + EJECTION_PAUSE_TICKS;
                            data.server_dirty = true;
                            (item, progress)
                        };
                        eject_result_item(world, pos, item);
                        let _ = sound_progress;
                        VaultState::Ejecting
                    }
                }
            };
            if next != current {
                self.set_vault_state(world, pos, state, next);
            }
        }

        // Vanilla `Server.tick` dirty flush: `setChanged`, plus a block update for shared data.
        let (server_dirty, shared_dirty) = {
            let mut data = self.state.lock();
            (std::mem::replace(&mut data.server_dirty, false), std::mem::replace(
                &mut data.shared_dirty,
                false,
            ))
        };
        if server_dirty || shared_dirty {
            BlockEntity::set_changed(self);
            if shared_dirty {
                world.send_block_updated(pos);
            }
        }
    }
}

/// Vanilla `ValueInput.getIntOr`.
fn read_int_or(view: &NbtCompoundView<'_, '_>, key: &str, default: i32) -> i32 {
    use steel_utils::nbt::NbtNumeric as _;
    view.get(key)
        .and_then(|tag| tag.codec_i32())
        .unwrap_or(default)
}

/// Vanilla `UUIDUtil.CODEC` list encoding (int arrays).
fn write_uuid_list(ids: &[uuid::Uuid]) -> NbtList {
    NbtList::IntArray(
        ids.iter()
            .map(|uuid| uuid.to_int_array().to_vec())
            .collect::<Vec<_>>(),
    )
}

/// Vanilla `UUIDUtil.CODEC` list decoding.
fn read_uuid_list(view: &NbtCompoundView<'_, '_>, key: &str) -> Vec<uuid::Uuid> {
    let Some(list) = view.list(key) else {
        return Vec::new();
    };
    let Some(arrays) = list.int_arrays() else {
        return Vec::new();
    };
    arrays
        .iter()
        .filter_map(|raw| uuid::Uuid::from_int_array(&raw.to_vec()))
        .collect()
}
