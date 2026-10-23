//! Vanilla `ItemCost`: one item requirement of a trade.

use std::io::{Cursor, Result as IoResult, Write};

use simdnbt::FromNbtTag as _;
use simdnbt::borrow::NbtCompound as NbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::data_component_predicate::DataComponentExactPredicate;
use steel_registry::data_components::{ComponentData, ComponentPatchEntry, DataComponentPatch};
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::{REGISTRY, RegistryEntry as _, RegistryExt as _};
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::nbt::NbtNumeric as _;
use steel_utils::serial::{ReadFrom, WriteTo};

/// Vanilla `ItemCost`: an item, a count and the exact components it must carry.
///
/// Fields are private to match vanilla's record accessors; every cost also caches the
/// `ItemStack` vanilla builds from the predicate (`ItemCost.itemStack()`).
#[derive(Debug, Clone)]
pub struct ItemCost {
    item: ItemRef,
    count: i32,
    components: DataComponentExactPredicate,
    stack: ItemStack,
}

impl ItemCost {
    /// Creates a cost and the stack vanilla's constructor derives from `components`.
    #[must_use]
    pub fn new(item: ItemRef, count: i32, components: DataComponentExactPredicate) -> Self {
        let stack = ItemStack::with_count_and_patch(item, count, components_patch(&components));
        Self {
            item,
            count,
            components,
            stack,
        }
    }

    /// The required item, vanilla `ItemCost.item()`.
    #[must_use]
    pub const fn item(&self) -> ItemRef {
        self.item
    }

    /// The required stack size, vanilla `ItemCost.count()`.
    #[must_use]
    pub const fn count(&self) -> i32 {
        self.count
    }

    /// The exact components the item must carry, vanilla `ItemCost.components()`.
    #[must_use]
    pub const fn components(&self) -> &DataComponentExactPredicate {
        &self.components
    }

    /// The stack vanilla caches on the record (`ItemCost.itemStack()`).
    #[must_use]
    pub const fn item_stack(&self) -> &ItemStack {
        &self.stack
    }

    /// Vanilla `ItemCost.test`.
    #[must_use]
    pub fn test(&self, stack: &ItemStack) -> bool {
        stack.is(self.item)
            && self
                .components
                .values()
                .iter()
                .all(|(entry, value)| stack.get_effective_value_raw(&entry.key) == Some(value))
    }

    /// Writes vanilla `ItemCost.CODEC` (`{id, count, components}`).
    ///
    /// Mirrors `optionalAlwaysPresentFieldOf`/`optionalFieldOf`: `count` is always
    /// written, `components` only when it differs from `DataComponentExactPredicate.EMPTY`.
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.insert("id", self.item.key.to_string());
        compound.insert("count", self.count);
        if !self.components.is_empty() {
            compound.insert(
                "components",
                components_patch(&self.components).to_nbt_tag_ref(),
            );
        }
        compound
    }

    /// Reads vanilla `ItemCost.CODEC`.
    ///
    /// Vanilla validates the count with `ExtraCodecs.POSITIVE_INT`, so a non-positive
    /// count is rejected rather than silently kept.
    #[must_use]
    pub fn from_nbt(nbt: &NbtCompoundView<'_, '_>) -> Option<Self> {
        let key = nbt.string("id")?.to_str().parse::<Identifier>().ok()?;
        let item = REGISTRY.items.by_key(&key)?;

        let count = match nbt.get("count") {
            Some(tag) => tag.codec_i32()?,
            None => 1,
        };
        if count < 1 {
            return None;
        }

        let components = match nbt.get("components") {
            Some(tag) => components_from_patch(&DataComponentPatch::from_nbt_tag(tag)?)?,
            None => DataComponentExactPredicate::EMPTY,
        };

        Some(Self::new(item, count, components))
    }
}

/// Vanilla `ItemCost.asPatch()` for the predicate's exact component values.
fn components_patch(components: &DataComponentExactPredicate) -> DataComponentPatch {
    let mut patch = DataComponentPatch::new();
    for (entry, value) in components.values() {
        patch.set_raw(entry.key.clone(), value.clone());
    }
    patch
}

/// Rebuilds the exact predicate vanilla's codec stored as a component map.
fn components_from_patch(patch: &DataComponentPatch) -> Option<DataComponentExactPredicate> {
    let values = patch
        .iter()
        .filter_map(|(key, entry)| match entry {
            ComponentPatchEntry::Set(data) => {
                Some((REGISTRY.data_components.by_key(key)?, data.clone()))
            }
            ComponentPatchEntry::Removed => None,
        })
        .collect::<Vec<(_, ComponentData)>>();
    DataComponentExactPredicate::new(values)
}

impl WriteTo for ItemCost {
    /// Vanilla `ItemCost.STREAM_CODEC`.
    fn write(&self, writer: &mut impl Write) -> IoResult<()> {
        VarInt(self.item.id() as i32).write(writer)?;
        VarInt(self.count).write(writer)?;
        self.components.write(writer)
    }
}

impl ReadFrom for ItemCost {
    fn read(data: &mut Cursor<&[u8]>) -> IoResult<Self> {
        let item_id = VarInt::read(data)?.0;
        let item_id = usize::try_from(item_id)
            .map_err(|_| std::io::Error::other(format!("Negative item id: {item_id}")))?;
        let item = REGISTRY
            .items
            .by_id(item_id)
            .ok_or_else(|| std::io::Error::other(format!("Unknown item id: {item_id}")))?;

        let count = VarInt::read(data)?.0;
        let components = DataComponentExactPredicate::read(data)?;
        Ok(Self::new(item, count, components))
    }
}
