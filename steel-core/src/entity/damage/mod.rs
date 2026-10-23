//! Exact damage attribution, recent damage history, and combat tracking.

mod combat_tracker;
mod history;
mod source;

pub use combat_tracker::{CombatEntry, CombatStateChange, CombatTracker, FallLocation};
pub(crate) use history::DamageHistoryBinding;
pub use history::{DamageHistory, RecentDamageSource};
pub use source::DamageSource;

use std::borrow::Cow;

use steel_registry::data_components::components::Rarity;
use steel_registry::data_components::vanilla_components::{CUSTOM_NAME, RARITY};
use steel_registry::item_stack::ItemStack;
use steel_registry::item_stack_template::ItemStackTemplate;
use steel_utils::translations;
use text_components::translation::TranslatedMessage;
use text_components::{Modifier as _, TextComponent};

use crate::behavior::ITEM_BEHAVIORS;
use crate::entity::SharedEntity;
use crate::inventory::equipment::EquipmentSlot;

/// Builds a vanilla translation component with a runtime-built key.
///
/// Vanilla composes death-message keys as `"death.attack." + source.getMsgId()`,
/// so the key is only known at runtime.
pub(crate) fn death_message(
    key: impl Into<Cow<'static, str>>,
    args: Vec<TextComponent>,
) -> TextComponent {
    let args = (!args.is_empty()).then(|| args.into_boxed_slice());
    TextComponent::translated(TranslatedMessage {
        key: key.into(),
        fallback: None,
        args,
    })
}

/// Returns the stack an entity holds in its main hand, or an empty stack.
///
/// Mirrors vanilla `LivingEntity.getMainHandItem`; non-living entities hold nothing.
pub(crate) fn main_hand_item(entity: Option<&SharedEntity>) -> ItemStack {
    let Some(living) = entity.and_then(|entity| entity.as_living_entity()) else {
        return ItemStack::empty();
    };
    let mut item = ItemStack::empty();
    living.with_equipment_slot(EquipmentSlot::MainHand, &mut |stack| item = stack.clone());
    item
}

/// Mirrors vanilla `ItemStack.has(DataComponents.CUSTOM_NAME)`.
pub(crate) fn has_custom_name(stack: &ItemStack) -> bool {
    stack.get(CUSTOM_NAME).is_some()
}

/// Vanilla `ItemStack.getDisplayName`: the bracketed hover name with the item's
/// rarity colour and an item tooltip.
///
/// **Deviation from vanilla:** the rarity colour comes from the stack's effective
/// `minecraft:rarity` component, which the item registry seeds with each item's
/// default, so the result matches vanilla without a separate item-level lookup.
pub(crate) fn item_display_name(stack: &ItemStack) -> TextComponent {
    let mut hover_name =
        TextComponent::new().add_child(ITEM_BEHAVIORS.hover_name(stack).into_owned());
    if has_custom_name(stack) {
        hover_name = hover_name.italic(true);
    }
    let mut result = translations::CHAT_SQUARE_BRACKETS
        .message([hover_name])
        .component();
    if !stack.is_empty() {
        let rarity = stack.get(RARITY).copied().unwrap_or(Rarity::Common);
        result = result.color(rarity.color());
        if let Ok(template) = ItemStackTemplate::from_stack(stack)
            && let Ok(hover) = template.to_hover_event()
        {
            result = result.hover_event(hover);
        }
    }
    result
}

#[cfg(test)]
mod tests;
