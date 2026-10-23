//! Filled map item behavior (`MapItem`).
//!
//! Vanilla `MapItem` overrides three `Item` hooks:
//!
//! - `inventoryTick`: `MapItem.getSavedData(itemStack, level)` →
//!   `MapItemSavedData.tickCarriedBy(player, stack, null)` for every carried
//!   map, then `MapItem.update(level, owner, data)` — the color rasterizer in
//!   [`crate::map::colors`] — when the stack sits in a hand slot and the map is
//!   unlocked.
//! - `onCraftedPostProcess`: consumes `map_post_processing` and copies the saved
//!   data through `locked()`/`scaled()` under a fresh map id. Steel has no
//!   crafted-post-process hook: `ResultSlot::on_take` carries the TODO for
//!   vanilla's `onCraftedBy`/item callbacks, so the cartography table result
//!   keeps its `map_post_processing` component until that hook lands. This
//!   module exposes [`MapItem::lock_map`]/[`MapItem::scale_map`] so the hook can
//!   call straight into the saved-data copy.
//! - `useOn`: for a block in `#minecraft:banners`, `toggleBanner` records the
//!   banner decoration and reports `FAIL` at the decoration limit; every other
//!   block defers to the plain `Item.useOn` path.
//!
//! [`MapItem::create`]/[`MapItem::apply_new_saved_data`] are vanilla's statics
//! for minting a filled map; the empty-map item calls them.

use steel_macros::item_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::components::MapId;
use steel_registry::data_components::vanilla_components::MAP_ID;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_items;
use steel_utils::Downcast as _;

use super::block_transformer;
use crate::behavior::context::{InteractionResult, InventoryTickContext, UseOnContext};
use crate::behavior::item::ItemBehavior;
use crate::inventory::equipment::EquipmentSlot;
use crate::map::MapItemSavedData;
use crate::player::Player;
use crate::world::World;

/// Behavior for the filled map item.
#[item_behavior]
pub struct MapItem;

impl MapItem {
    /// Vanilla `MapItem.create`: a filled map centered on the scale-aligned area
    /// containing `(origin_x, origin_z)`.
    #[must_use]
    pub fn create(
        level: &World,
        origin_x: i32,
        origin_z: i32,
        scale: u8,
        track_position: bool,
        unlimited_tracking: bool,
    ) -> ItemStack {
        let mut map = ItemStack::new(&vanilla_items::FILLED_MAP);
        Self::apply_new_saved_data(
            level,
            &mut map,
            origin_x,
            origin_z,
            scale,
            track_position,
            unlimited_tracking,
        );
        map
    }

    /// Vanilla `MapItem.applyNewSavedData`: allocates a map id, stores fresh
    /// saved data under it and writes the id into `map`.
    pub fn apply_new_saved_data(
        level: &World,
        map: &mut ItemStack,
        origin_x: i32,
        origin_z: i32,
        scale: u8,
        track_position: bool,
        unlimited_tracking: bool,
    ) {
        let data = MapItemSavedData::create_fresh(
            f64::from(origin_x),
            f64::from(origin_z),
            scale,
            track_position,
            unlimited_tracking,
            level.key.clone(),
        );
        let id = level.get_free_map_id();
        level.set_map_data(id, data);
        map.set(MAP_ID, MapId::new(id));
    }

    /// Vanilla `MapItem.lockMap`: copies the saved data into a locked map under
    /// a fresh id.
    ///
    /// Returns `false` when the stack has no stored map.
    pub fn lock_map(map: &mut ItemStack, level: &World) -> bool {
        let Some(data) = Self::saved_data(map, level) else {
            return false;
        };
        let id = level.get_free_map_id();
        level.set_map_data(id, data.locked_copy());
        map.set(MAP_ID, MapId::new(id));
        true
    }

    /// Vanilla `MapItem.scaleMap`: copies the saved data one zoom step out under
    /// a fresh id.
    ///
    /// Returns `false` when the stack has no stored map.
    pub fn scale_map(map: &mut ItemStack, level: &World) -> bool {
        let Some(data) = Self::saved_data(map, level) else {
            return false;
        };
        let id = level.get_free_map_id();
        level.set_map_data(id, data.scaled());
        map.set(MAP_ID, MapId::new(id));
        true
    }

    /// Vanilla `MapItem.getSavedData`: the saved data behind a filled map.
    fn saved_data(map: &ItemStack, level: &World) -> Option<MapItemSavedData> {
        let id = map.get(MAP_ID)?.id();
        level.maps().with_map_mut(id, |data| data.clone())
    }
}

impl ItemBehavior for MapItem {
    fn inventory_tick(&self, context: &mut InventoryTickContext<'_>) {
        let Some(player) = context.owner.downcast_ref::<Player>() else {
            return;
        };
        let Some(map_id) = context
            .with_item(|stack| stack.get(MAP_ID).copied())
            .flatten()
        else {
            return;
        };
        let world = context.world;
        let maps = world.maps();
        // Vanilla `tickCarriedBy` runs for every carried map, locked or not.
        if !maps.tick_carried_by(world, map_id.id(), player) {
            return;
        }
        // Vanilla skips `update` for locked maps and for stacks outside a hand
        // slot; the inventory tick only reports the selected hotbar slot as one.
        if context.slot == Some(EquipmentSlot::MainHand)
            && maps.is_locked(map_id.id()) == Some(false)
        {
            maps.update_colors(world, map_id.id(), player);
        }
    }

    fn use_on(&self, context: &mut UseOnContext<'_>) -> InteractionResult {
        let clicked = context.world.get_block_state(context.hit_result.block_pos);
        if !clicked.get_block().has_tag(&BlockTag::BANNERS) {
            return block_transformer::use_on(context);
        }
        let map_id = context.inv.with_item(|stack| stack.get(MAP_ID).copied());
        if let Some(map_id) = map_id {
            let toggled = context.world.maps().toggle_banner(
                context.world.as_ref(),
                map_id.id(),
                context.hit_result.block_pos,
            );
            if toggled == Some(false) {
                // Vanilla `MapItemSavedData.toggleBanner` refuses past the
                // decoration limit, which makes the interaction fail.
                return InteractionResult::Fail;
            }
        }
        InteractionResult::Success
    }
}
