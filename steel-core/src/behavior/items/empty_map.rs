//! Empty map item behavior (`EmptyMapItem`).
//!
//! Vanilla `EmptyMapItem.use` (`EmptyMapItem.java:24-45`) is a single
//! composite effect:
//!
//! 1. consume one empty map (`ItemStack.consume(1, player)`),
//! 2. award `Stats.ITEM_USED`,
//! 3. play `SoundEvents.UI_CARTOGRAPHY_TABLE_TAKE_RESULT` at the player,
//! 4. build a filled map for the player's current block x/z with scale `0`,
//!    `trackPosition = true`, `unlimitedTracking = false`
//!    (`MapItem.create(serverLevel, player.getBlockX(), player.getBlockZ(),
//!    (byte) 0, true, false)`); when the hand stack empties the result is
//!    returned as `InteractionResult.SUCCESS.heldItemTransformedTo(map)`,
//!    otherwise the map is added to the inventory
//!    (`PlayerInventory.add`) and the overflow dropped
//!    (`Player.drop(map, false, Prediction.SERVER_ONLY)`).
//!
//! None of step 4 exists in Steel: there is no `MapItemSavedData`, no map-id
//! allocation and no level-level map storage (`MapItem::create` /
//! `MapItemSavedData` / `ServerLevel.getFreeMapId` / `setMapData` have zero
//! hits under `steel-core/src`; see [`super::map`] for the full evidence), so
//! `MapItem.create` cannot be mirrored without inventing map state.
//!
//! Because the *entire* payload of `use` is producing that filled map, no
//! partial version is implemented here: consuming the empty map and awarding
//! its stat/sound while producing no map would silently delete the player's
//! item. The behavior therefore inherits the trait default (`use_item`
//! returning `Pass`), and vanilla's `use` lands once the map data model
//! (owned by the ScoreboardMaps lane) plus a `MapItem::create`-equivalent
//! exist.

use steel_macros::item_behavior;

use crate::behavior::ItemBehavior;

/// Behavior for the empty map item.
///
/// Vanilla has no behavior beyond `use`, which is blocked on the map data
/// model — see the module documentation.
#[item_behavior]
pub struct EmptyMapItem;

impl ItemBehavior for EmptyMapItem {}
