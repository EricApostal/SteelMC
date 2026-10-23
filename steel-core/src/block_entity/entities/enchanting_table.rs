//! Enchanting table block entity implementation.
//!
//! Mirrors Vanilla `EnchantingTableBlockEntity`: the entity owns no lockable
//! container and emits no redstone signal. It persists the optional custom name
//! that Vanilla's `Nameable` exposes as the menu title, plus the table's
//! enchantment seed (see [`EnchantingTableBlockEntity::enchantment_seed`]).
//! The book-animation fields (`time`, `flip`, `open`, `rot`, …) are written and
//! read exclusively by the client, so they are intentionally not mirrored.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use simdnbt::borrow::BaseNbtCompound as BorrowedNbtCompound;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex, translations,
};
use text_components::TextComponent;

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// Enchanting table block entity.
///
/// Behaves like Vanilla's `Nameable` block entity: it exists so the table has a
/// stable identity for menu validity checks and can carry a custom name. It
/// never syncs an update tag and never drops items when removed.
pub struct EnchantingTableBlockEntity {
    base: Arc<BlockEntityBase>,
    /// The table's enchantment seed, persisted as `EnchantSeed`.
    ///
    /// Vanilla keeps this per player (`XpSeed`) and rerolls it whenever an
    /// enchantment is applied, which makes the offers differ per player. Steel
    /// keeps it per table instead, so a table's offers are stable across
    /// sessions and identical for everyone using it; the offers themselves are
    /// computed with Vanilla's exact seeded roll.
    enchantment_seed: AtomicI32,
    custom_name: SyncMutex<Option<TextComponent>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies
// `EnchantingTableBlockEntity`.
unsafe impl DowncastType for EnchantingTableBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/enchanting_table");
}

impl EnchantingTableBlockEntity {
    /// Creates a new enchanting table block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: Arc::new(BlockEntityBase::new(
                &vanilla_block_entity_types::ENCHANTING_TABLE,
                level,
                pos,
                state,
            )),
            custom_name: SyncMutex::new(None),
            enchantment_seed: AtomicI32::new(rand::random()),
        }
    }

    /// Returns the table's enchantment seed.
    #[must_use]
    pub fn enchantment_seed(&self) -> i32 {
        self.enchantment_seed.load(Ordering::Relaxed)
    }

    /// Sets the table's enchantment seed.
    ///
    /// Vanilla rerolls the seed on every applied enchantment
    /// (`Player.onEnchantmentPerformed`).
    pub fn set_enchantment_seed(&self, seed: i32) {
        self.enchantment_seed.store(seed, Ordering::Relaxed);
    }

    /// Returns the custom name, if one was set.
    ///
    /// Vanilla parity: `EnchantingTableBlockEntity.getCustomName`.
    #[must_use]
    pub fn custom_name(&self) -> Option<TextComponent> {
        self.custom_name.lock().clone()
    }

    /// Sets or clears the custom name.
    ///
    /// Vanilla parity: `EnchantingTableBlockEntity.setCustomName`.
    pub fn set_custom_name(&self, name: Option<TextComponent>) {
        *self.custom_name.lock() = name;
    }

    /// Returns the name shown as the enchanting menu title.
    ///
    /// Vanilla parity: `Nameable.getDisplayName`, which falls back to
    /// `container.enchant` when no custom name is set.
    #[must_use]
    pub fn display_name(&self) -> TextComponent {
        self.custom_name()
            .unwrap_or_else(|| TextComponent::translated(translations::CONTAINER_ENCHANT.msg()))
    }
}

impl BlockEntity for EnchantingTableBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: BorrowedNbtCompoundView<'_, '_> = nbt.into();
        if let Some(seed) = nbt.int("EnchantSeed") {
            self.set_enchantment_seed(seed);
        }
        let name = nbt
            .get("CustomName")
            .map(|tag| tag.to_owned())
            .and_then(|tag| TextComponent::from_nbt(&tag));
        self.set_custom_name(name);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert("EnchantSeed", self.enchantment_seed());
        if let Some(name) = self.custom_name() {
            nbt.insert("CustomName", name.to_codec_nbt());
        }
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla sends no update tag for the enchanting table: the client
        // renders the book from its own local animation state.
        None
    }
}
