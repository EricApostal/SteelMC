//! `BannerBlockEntity` (`net.minecraft.world.level.block.entity.BannerBlockEntity`).
//!
//! Banners only persist their pattern layers; the base colour lives on the block. Vanilla sends
//! the full layer list in `getUpdateTag` so the client can render the banner without the item.

use std::sync::Weak;

use simdnbt::borrow::BaseNbtCompound as BorrowedNbtCompound;
use simdnbt::owned::NbtCompound;
use simdnbt::{FromNbtTag, ToNbtTag};
use steel_registry::data_components::components::BannerPatternLayers;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// `BannerBlockEntity`.
pub struct BannerBlockEntity {
    base: BlockEntityBase,
    patterns: SyncMutex<BannerPatternLayers>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BannerBlockEntity`.
unsafe impl DowncastType for BannerBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/banner");
}

impl BannerBlockEntity {
    /// Creates a banner block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::BANNER, level, pos, state),
            patterns: SyncMutex::new(BannerPatternLayers::empty()),
        }
    }

    /// Returns the banner's pattern layers.
    #[must_use]
    pub fn patterns(&self) -> BannerPatternLayers {
        self.patterns.lock().clone()
    }

    /// Replaces the banner's pattern layers.
    pub fn set_patterns(&self, patterns: BannerPatternLayers) {
        *self.patterns.lock() = patterns;
        BlockEntity::set_changed(self);
    }

    /// Vanilla `BannerBlockEntity.fromItem`: copies the `banner_patterns` component from `item`.
    pub fn from_item(&self, item: &ItemStack) {
        if let Some(layers) =
            item.get(steel_registry::data_components::vanilla_components::BANNER_PATTERNS)
        {
            self.set_patterns(layers.clone());
        }
    }
}

impl BlockEntity for BannerBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: simdnbt::borrow::NbtCompound<'_, '_> = nbt.into();
        let patterns = nbt
            .get("patterns")
            .and_then(BannerPatternLayers::from_nbt_tag)
            .unwrap_or_else(BannerPatternLayers::empty);
        *self.patterns.lock() = patterns;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let patterns = self.patterns.lock();
        if !patterns.layers().is_empty() {
            nbt.insert("patterns", patterns.clone().to_nbt_tag());
        }
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }

    fn apply_components_from_item(&self, item: &ItemStack) {
        self.from_item(item);
    }
}
