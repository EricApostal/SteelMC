//! `BannerBlockEntity` (`net.minecraft.world.level.block.entity.BannerBlockEntity`).
//!
//! Banners only persist their pattern layers; the base color lives on the block. Vanilla sends
//! the full layer list in `getUpdateTag` so the client can render the banner without the item.

use std::sync::Weak;

use simdnbt::borrow::BaseNbtCompound as BorrowedNbtCompound;
use simdnbt::owned::NbtCompound;
use simdnbt::{FromNbtTag, ToNbtTag};
use steel_registry::data_components::components::BannerPatternLayers;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};
use text_components::TextComponent;

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// `BannerBlockEntity`.
pub struct BannerBlockEntity {
    base: BlockEntityBase,
    patterns: SyncMutex<BannerPatternLayers>,
    custom_name: SyncMutex<Option<TextComponent>>,
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
            custom_name: SyncMutex::new(None),
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

    /// Returns the banner's custom name, if one was set.
    ///
    /// Vanilla `BannerBlockEntity.getCustomName`; maps show it as the marker's
    /// hover name (`MapBanner.fromWorld`).
    #[must_use]
    pub fn custom_name(&self) -> Option<TextComponent> {
        self.custom_name.lock().clone()
    }

    /// Sets or clears the banner's custom name.
    pub fn set_custom_name(&self, name: Option<TextComponent>) {
        *self.custom_name.lock() = name;
        BlockEntity::set_changed(self);
    }

    /// Vanilla `BannerBlockEntity.applyImplicitComponents`: copies the
    /// `banner_patterns` and `custom_name` components from `item`.
    pub fn from_item(&self, item: &ItemStack) {
        let patterns = item
            .get(steel_registry::data_components::vanilla_components::BANNER_PATTERNS)
            .cloned()
            .unwrap_or_else(BannerPatternLayers::empty);
        *self.patterns.lock() = patterns;
        *self.custom_name.lock() = item
            .get(steel_registry::data_components::vanilla_components::CUSTOM_NAME)
            .cloned();
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
        let name = nbt
            .get("CustomName")
            .map(|tag| tag.to_owned())
            .and_then(|tag| TextComponent::from_nbt(&tag));
        *self.custom_name.lock() = name;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let patterns = self.patterns.lock();
        if !patterns.layers().is_empty() {
            nbt.insert("patterns", patterns.clone().to_nbt_tag());
        }
        drop(patterns);
        if let Some(name) = self.custom_name() {
            nbt.insert("CustomName", name.to_codec_nbt());
        }
    }

    /// Vanilla `BannerBlockEntity.getUpdateTag`: `saveWithoutMetadata`, so the
    /// patterns, the custom name and the stored component map travel to clients.
    fn get_update_tag(&self) -> Option<NbtCompound> {
        Some(self.save_without_metadata())
    }

    fn apply_components_from_item(&self, item: &ItemStack) {
        self.from_item(item);
    }
}
