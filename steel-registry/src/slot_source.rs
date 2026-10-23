use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::loot_table::ItemPredicate;

/// A `slot_source` entry from a data pack JSON file.
///
/// Vanilla's registry value is a `SlotSource`; the registry key lives outside it,
/// so Steel pairs the two here.
#[derive(Debug)]
pub struct SlotSourceEntry {
    /// The `slot_source` key.
    pub key: Identifier,
    pub source: SlotSource,
}

/// The registered `slot_source` types of vanilla's `SlotSources`.
#[derive(Debug)]
pub enum SlotSource {
    /// Concatenates its terms (vanilla `GroupSlotSource`).
    Group(GroupSlotSource),
    /// Keeps only slots whose stack matches the item filter (vanilla
    /// `FilteredSlotSource`).
    Filtered(FilteredSlotSource),
    /// Keeps at most `limit` slots (vanilla `LimitSlotSource`).
    Limit(LimitSlotSource),
    /// Selects a named slot range from a context object (vanilla
    /// `RangeSlotSource`).
    SlotRange(RangeSlotSource),
    /// Expands each slot into the contents of its container component (vanilla
    /// `ContentsSlotSource`).
    Contents(ContentsSlotSource),
    /// Provides no slots (vanilla `EmptySlotSource`).
    Empty(EmptySlotSource),
}

/// A `Holder<SlotSource>`: a reference to another `slot_source` entry or an
/// inline definition.
#[derive(Debug)]
pub enum SlotSourceHolder {
    /// A reference by `slot_source` key.
    Reference(Identifier),
    /// A definition written inline in the JSON.
    Inline(&'static SlotSource),
}

/// Vanilla `CompositeSlotSource`: the shared shape of slot sources combining
/// several terms.
#[derive(Debug)]
pub struct GroupSlotSource {
    /// The combined terms.
    pub terms: &'static [SlotSourceHolder],
}

/// Vanilla `FilteredSlotSource`.
#[derive(Debug)]
pub struct FilteredSlotSource {
    /// The source whose slots are filtered.
    pub slot_source: SlotSourceHolder,
    /// Vanilla `item_filter` (`net.minecraft.advancements.predicates.ItemPredicate`).
    pub item_filter: ItemPredicate,
}

/// Vanilla `LimitSlotSource`.
#[derive(Debug)]
pub struct LimitSlotSource {
    /// The source whose slots are limited.
    pub slot_source: SlotSourceHolder,
    /// Maximum number of slots to keep; vanilla requires a positive value.
    pub limit: i32,
}

/// Vanilla `RangeSlotSource`.
#[derive(Debug)]
pub struct RangeSlotSource {
    /// Which loot context object supplies the slots; defaults to `container`.
    pub source: SlotSourceContextArg,
    /// The vanilla `SlotRanges` name, e.g. `contents` or `container.0`.
    pub slots: &'static str,
}

/// Vanilla `ContentsSlotSource`.
#[derive(Debug)]
pub struct ContentsSlotSource {
    /// The source whose slots are expanded.
    pub slot_source: SlotSourceHolder,
    /// The container component whose item list replaces each slot.
    pub component: ContainerComponentManipulator,
}

/// Vanilla `EmptySlotSource`.
#[derive(Debug)]
pub struct EmptySlotSource;

/// The loot context argument a `RangeSlotSource` reads slots from.
///
/// Mirrors vanilla's `LootContextArg` union of `LootContext.EntityTarget`,
/// `LootContext.BlockEntityTarget`, and the `container` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotSourceContextArg {
    /// The entity being looted.
    This,
    /// The attacking entity.
    Attacker,
    /// The direct attacker (e.g. an arrow rather than its shooter).
    DirectAttacker,
    /// The player that dealt the last damage.
    AttackingPlayer,
    /// The target entity.
    TargetEntity,
    /// The entity that interacted with the block or entity.
    InteractingEntity,
    /// The looted block entity.
    BlockEntity,
    /// The looted container.
    Container,
}

/// The container components vanilla can expand into slots (vanilla
/// `ContainerComponentManipulators`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerComponentManipulator {
    /// `minecraft:container`.
    Container,
    /// `minecraft:bundle_contents`.
    BundleContents,
    /// `minecraft:charged_projectiles`.
    ChargedProjectiles,
}

/// A reference to a registered `slot_source` entry.
pub type SlotSourceRef = &'static SlotSourceEntry;

/// Registry for `slot_source` definitions.
pub struct SlotSourceRegistry {
    slot_sources_by_id: Vec<SlotSourceRef>,
    slot_sources_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl SlotSourceRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            slot_sources_by_id: Vec::new(),
            slot_sources_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    SlotSourceRegistry,
    SlotSourceRef,
    slot_sources_by_id,
    slot_sources_by_key,
    allows_registering
);

crate::impl_registry!(
    SlotSourceRegistry,
    SlotSourceEntry,
    slot_sources_by_id,
    slot_sources_by_key,
    slot_sources
);
