//! Debug stick item behavior implementation.
//!
//! Mirrors vanilla `DebugStickItem.useOn`: a game-master player right-clicks a
//! block to cycle one of its state properties forward (or backward while
//! sneaking), writing the new state and reporting the change on the action bar.
//!
//! **Vanilla differences:** vanilla's left-click handler
//! (`canDestroyBlock`) selects *which* property is cycled and stores it in the
//! `minecraft:debug_stick_state` component. Steel's `ItemBehavior` has no
//! `can_destroy_block` hook, so the stored selection is only read here and the
//! cycle always starts from the block's first property.

use std::borrow::Cow;

use steel_macros::item_behavior;
use steel_registry::REGISTRY;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::data_components::components::DebugStickProperty;
use steel_registry::data_components::vanilla_components::DEBUG_STICK_STATE;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};
use text_components::TextComponent;
use text_components::translation::TranslatedMessage;

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseOnContext};
use crate::behavior::items::dynamic_name;

/// Vanilla `DebugStickItem` sets the cycled state with flags `18`, i.e.
/// `Block.UPDATE_CLIENTS | Block.UPDATE_KNOWN_SHAPE`.
const CYCLE_FLAGS: UpdateFlags = UpdateFlags::UPDATE_CLIENTS.union(UpdateFlags::UPDATE_KNOWN_SHAPE);

/// Behavior for the debug stick.
#[item_behavior]
pub struct DebugStickItem;

impl DebugStickItem {
    /// Sends a message on the player's action bar.
    fn message(context: &UseOnContext<'_>, message: &TextComponent) {
        context.player.send_overlay_message(message);
    }

    /// Mirrors vanilla `DebugStickItem.handleInteraction`'s cycling branch.
    fn handle_interaction(context: &UseOnContext<'_>, state: BlockStateId, pos: BlockPos) -> bool {
        if !context.player.is_operator() {
            return false;
        }

        let block = state.get_block();
        // Vanilla keys its messages off `Item.descriptionId`, which is the item
        // name key; fall back to the registry-key form when the item data has
        // no `item_name` component.
        let description_id = context.inv.with_item(|stack| {
            dynamic_name::description_id(stack).map_or_else(
                || {
                    format!(
                        "item.{}.{}",
                        stack.item().key.namespace,
                        stack.item().key.path
                    )
                },
                str::to_owned,
            )
        });

        if block.properties.is_empty() {
            let message = TextComponent::translated(TranslatedMessage {
                key: Cow::Owned(format!("{description_id}.empty")),
                fallback: None,
                args: Some(Box::new([TextComponent::plain(block.key.to_string())])),
            });
            Self::message(context, &message);
            return false;
        }

        // `Option<Option<&'static str>>`: outer `None` means the stack carries no
        // `debug_stick_state` component (vanilla fails the interaction), inner
        // `None` means this block has no stored property yet.
        let selection = context.inv.with_item(|stack| {
            stack.get(DEBUG_STICK_STATE).map(|debug_stick_state| {
                debug_stick_state
                    .properties()
                    .find(|property| property.block().key == block.key)
                    .map(DebugStickProperty::property)
            })
        });
        let Some(selected) = selection else {
            return false;
        };

        let property_name = selected.unwrap_or_else(|| block.properties[0].get_name());
        let Some(new_state) = Self::cycle_state(
            state,
            property_name,
            context.player.is_secondary_use_active(),
        ) else {
            return false;
        };

        context.world.set_block(pos, new_state, CYCLE_FLAGS);

        let value_name = new_state
            .get_property_str(property_name)
            .unwrap_or_default();
        let message = TextComponent::translated(TranslatedMessage {
            key: Cow::Owned(format!("{description_id}.update")),
            fallback: None,
            args: Some(Box::new([
                TextComponent::plain(property_name),
                TextComponent::plain(value_name),
            ])),
        });
        Self::message(context, &message);
        true
    }

    /// Mirrors vanilla `DebugStickItem.cycleState`: replaces the named property
    /// with the next (or previous, wrapping) value in the property's own value
    /// order.
    fn cycle_state(
        state: BlockStateId,
        property_name: &str,
        backward: bool,
    ) -> Option<BlockStateId> {
        let block = state.get_block();
        let property = block
            .properties
            .iter()
            .find(|property| property.get_name() == property_name)?;
        let names = property.get_possible_value_names();
        let current = state.get_property_str(property_name)?;
        let current_index = names.iter().position(|name| *name == current)?;
        let next_index = if backward {
            (current_index + names.len() - 1) % names.len()
        } else {
            (current_index + 1) % names.len()
        };
        REGISTRY
            .blocks
            .try_set_property_by_name(state, property_name, names[next_index])
    }
}

impl ItemBehavior for DebugStickItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let pos = context.hit_result.block_pos;
        let state = context.world.get_block_state(pos);

        if Self::handle_interaction(context, state, pos) {
            InteractionResult::Success
        } else {
            InteractionResult::Fail
        }
    }
}
