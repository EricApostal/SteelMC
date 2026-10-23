//! Filled map item behavior (`MapItem`).
//!
//! Vanilla `MapItem` (`MapItem.java`) overrides three `Item` hooks, and every
//! one of them is driven by `MapItemSavedData` fetched through the `map_id`
//! component:
//!
//! - `inventoryTick` (`MapItem.java:277-289`): `MapItem.getSavedData(itemStack,
//!   level)` → `MapItemSavedData.tickCarriedBy(player, stack, null)` and, for an
//!   unlocked map held in a hand slot, `update(level, owner, data)`, the map
//!   color rasterizer (`MapItem.java:83-227`, including `checkBanners`,
//!   `updateColor`, heightmap sampling and water-depth shading).
//! - `onCraftedPostProcess` (`MapItem.java:291-303`): removes
//!   `DataComponents.MAP_POST_PROCESSING` and, for `LOCK`/`SCALE`, copies the
//!   saved data through `MapItemSavedData.locked()`/`scaled()` after allocating
//!   a new `MapId`.
//! - `useOn` (`MapItem.java:305-317`): for a block in `BlockTags.BANNERS`,
//!   `MapItemSavedData.toggleBanner(level, pos)` records the banner decoration
//!   and reports `FAIL` when the map is already at its decoration limit;
//!   otherwise it defers to `super.useOn`, the plain item `useOn` path which
//!   Steel's trait default already reproduces via
//!   `items::block_transformer::use_on` (see [`ItemBehavior::use_on`] at
//!   `steel-core/src/behavior/item.rs:74-76`).
//!
//! Steel has no map data model: `MapItemSavedData`, `MapItem::create`,
//! `MapItem.applyNewSavedData`, `MapId` allocation and level map storage are
//! absent from `steel-core` (only the `MAP_ID` / `MAP_DECORATIONS` /
//! `MAP_POST_PROCESSING` *components* exist, in `steel-registry`), and
//! `ItemBehavior` exposes no crafted-post-process hook at all
//! (`steel-core/src/behavior/item.rs` has no `crafted` entry).
//!
//! All three overrides are therefore unreachable: the rasterizer needs a
//! persistent per-map image, the crafting path needs a saved-data copy, and the
//! banner path needs the decoration limit. Implementing only the "clicked block
//! is a banner ⇒ `Success`" branch would drop the banner decoration (a silent
//! divergence), and the trait default already mirrors vanilla's `super.useOn`
//! fallback for every non-banner block. This type therefore carries no
//! override and inherits the defaults until the data model lands.

use steel_macros::item_behavior;

use crate::behavior::ItemBehavior;

/// Behavior for the filled map item.
///
/// Vanilla's `inventoryTick`/`onCraftedPostProcess`/`useOn` are entirely
/// data-model dependent and therefore unimplemented — see the module
/// documentation.
#[item_behavior]
pub struct MapItem;

impl ItemBehavior for MapItem {}
