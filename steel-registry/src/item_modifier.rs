use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::loot_table::ConditionalLootFunction;

/// An `item_modifier` entry from a data pack JSON file.
///
/// Vanilla registers these as `Registries.ITEM_MODIFIER`, a resource registry of
/// `LootItemFunction`s that other data pack files apply to item stacks.
#[derive(Debug)]
pub struct ItemModifier {
    /// The `item_modifier` key.
    pub key: Identifier,
    /// The loot function, including its conditions, exactly as vanilla stores a
    /// `LootItemFunction`.
    pub function: ConditionalLootFunction,
}

/// A reference to a registered `item_modifier`.
pub type ItemModifierRef = &'static ItemModifier;

/// Registry for `item_modifier` functions.
pub struct ItemModifierRegistry {
    item_modifiers_by_id: Vec<ItemModifierRef>,
    item_modifiers_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl ItemModifierRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            item_modifiers_by_id: Vec::new(),
            item_modifiers_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    ItemModifierRegistry,
    ItemModifierRef,
    item_modifiers_by_id,
    item_modifiers_by_key,
    allows_registering
);

crate::impl_registry!(
    ItemModifierRegistry,
    ItemModifier,
    item_modifiers_by_id,
    item_modifiers_by_key,
    item_modifiers
);
