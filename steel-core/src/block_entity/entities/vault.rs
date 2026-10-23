//! `VaultBlockEntity` (`net.minecraft.world.level.block.entity.vault.VaultBlockEntity`).
//!
//! A vault holds three data groups in vanilla: `config` (key item, loot table, activation ranges),
//! `shared_data` (display item, connected players) and `server_data` (rewarded players, cooldown).
//! This mirrors all three plus the `VaultState` machine in [`VaultState`].

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, VaultState};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_items;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::behavior::InventoryAccess;
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::player::Player;
use crate::world::World;

/// Vanilla `VaultBlockEntity.ACTIVATION_RANGE` (blocks).
pub const VAULT_ACTIVATION_RANGE: f64 = 4.0;
/// Vanilla `VaultBlockEntity.DEACTIVATION_RANGE` (blocks).
pub const VAULT_DEACTIVATION_RANGE: f64 = 4.5;

/// Mutable vault state.
struct VaultStateData {
    /// UUIDs of players who already received this vault's reward.
    rewarded_players: Vec<uuid::Uuid>,
    /// Game time at which the state machine may resume.
    state_updating_resumes_at: i64,
    /// Remaining reward ejections.
    total_ejections_needed: i32,
    /// Item displayed in the vault's mouth.
    display_item: ItemStack,
    /// Whether this is the ominous variant (`trial_spawner` ominous chain).
    ominous: bool,
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
            state: SyncMutex::new(VaultStateData {
                rewarded_players: Vec::new(),
                state_updating_resumes_at: 0,
                total_ejections_needed: 0,
                display_item: ItemStack::empty(),
                ominous: false,
            }),
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

    /// Vanilla `VaultServerData.addToRewardedPlayers`.
    pub fn add_rewarded_player(&self, player: &Player) {
        let mut state = self.state.lock();
        if !state.rewarded_players.contains(&player.gameprofile.id()) {
            state.rewarded_players.push(player.gameprofile.id());
        }
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
        self.state.lock().display_item = item;
        BlockEntity::set_changed(self);
    }

    /// Returns whether the vault currently holds a display item.
    #[must_use]
    pub fn has_display_item(&self) -> bool {
        !self.state.lock().display_item.is_empty()
    }

    /// Returns whether this vault is the ominous variant.
    #[must_use]
    pub fn is_ominous(&self) -> bool {
        self.state.lock().ominous
    }

    /// Sets the ominous flag (used when the block state is placed from an ominous item).
    pub fn set_ominous(&self, ominous: bool) {
        self.state.lock().ominous = ominous;
        BlockEntity::set_changed(self);
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
        // Vanilla `VaultConfig.keyItem` defaults to `trial_key`, with `ominous_trial_key` for
        // ominous vaults (set from the vault's `config` component).
        let expected = if self.is_ominous() {
            &*vanilla_items::OMINOUS_TRIAL_KEY
        } else {
            &*vanilla_items::TRIAL_KEY
        };
        if inserted.item() != expected || self.has_rewarded_player(player) {
            return false;
        }

        // Consume exactly one key from the interacting hand.
        inv.with_item(ItemStack::shrink_one);

        let mut data = self.state.lock();
        data.rewarded_players.push(player.gameprofile.id());
        // Vanilla `VaultState.UNLOCKING` lasts 20 ticks before ejection.
        data.state_updating_resumes_at = world.game_time() + 20;
        data.total_ejections_needed = 1;
        drop(data);

        world.set_block(
            self.get_block_pos(),
            state.set_value(&BlockStateProperties::VAULT_STATE, VaultState::Unlocking),
            steel_utils::types::UpdateFlags::UPDATE_ALL,
        );
        BlockEntity::set_changed(self);
        true
    }
}

impl BlockEntity for VaultBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();

        state.rewarded_players.clear();
        state.display_item = ItemStack::empty();
        state.state_updating_resumes_at = 0;
        state.total_ejections_needed = 0;

        if let Some(server_data) = view.compound("server_data") {
            if let Some(players) = server_data.list("rewarded_players") {
                if let Some(texts) = players.strings() {
                    for text in texts {
                        if let Ok(uuid) = text.to_str().parse::<uuid::Uuid>() {
                            state.rewarded_players.push(uuid);
                        }
                    }
                }
            }
            state.state_updating_resumes_at =
                server_data.long("state_updating_resumes_at").unwrap_or(0);
            state.total_ejections_needed = server_data.int("total_ejections_needed").unwrap_or(0);
        }

        if let Some(shared_data) = view.compound("shared_data")
            && let Some(item) = shared_data
                .compound("display_item")
                .and_then(|compound| ItemStack::from_borrowed_compound(&compound))
        {
            state.display_item = item;
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();

        let mut server_data = NbtCompound::new();
        let rewarded = NbtList::String(
            state
                .rewarded_players
                .iter()
                .map(|uuid| uuid.to_string().into())
                .collect(),
        );
        server_data.insert("rewarded_players", rewarded);
        server_data.insert("state_updating_resumes_at", state.state_updating_resumes_at);
        server_data.insert("total_ejections_needed", state.total_ejections_needed);
        nbt.insert("server_data", server_data);

        let mut shared_data = NbtCompound::new();
        if !state.display_item.is_empty() {
            shared_data.insert("display_item", state.display_item.to_nbt_tag_ref());
        }
        nbt.insert("shared_data", shared_data);
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla sends only `shared_data` to clients.
        let state = self.state.lock();
        let mut shared_data = NbtCompound::new();
        if !state.display_item.is_empty() {
            shared_data.insert("display_item", state.display_item.to_nbt_tag_ref());
        }
        let mut nbt = NbtCompound::new();
        nbt.insert("shared_data", shared_data);
        Some(nbt)
    }

    fn tick(&self, world: &std::sync::Arc<World>) {
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let current = state.get_value(&BlockStateProperties::VAULT_STATE);
        let resume_at = self.state.lock().state_updating_resumes_at;

        if world.game_time() < resume_at {
            return;
        }

        let next = match current {
            VaultState::Inactive => VaultState::Active,
            VaultState::Active => VaultState::Inactive,
            VaultState::Unlocking => VaultState::Ejecting,
            VaultState::Ejecting => {
                // TODO(Loot): eject the configured loot table item with level event
                // `ANIMATION_VAULT_EJECT_ITEM` and drop back to `inactive` once the loot lane
                // exposes a runtime loot-table roll from block behaviors.
                VaultState::Inactive
            }
        };
        if next != current {
            world.set_block(
                pos,
                state.set_value(&BlockStateProperties::VAULT_STATE, next),
                steel_utils::types::UpdateFlags::UPDATE_ALL,
            );
        }
    }
}
