//! Wire mirrors of vanilla's `RecipeDisplay`/`SlotDisplay` codecs.
//!
//! Both codecs are static-registry dispatches in vanilla
//! (`ByteBufCodecs.registry(Registries.RECIPE_DISPLAY).dispatch(...)` and the
//! `SLOT_DISPLAY` equivalent): a `VarInt` registry id followed by that variant's
//! payload. The ids baked into the `WriteTo` impls below are the vanilla
//! registration order, which is the order `MappedRegistry` hands out ids in
//! (`RecipeDisplays.bootstrap` / `SlotDisplays.bootstrap`).
//!
//! Steel has no `recipe_display`/`slot_display` registry yet, so the ids live in
//! this mirror instead of being resolved through `REGISTRY`.

use std::fmt::{Debug, Formatter};
use std::io::{Error, Result, Write};

use steel_macros::ReadFrom;
use steel_registry::ItemStackTemplate;
use steel_registry::data_components::ComponentEntryRef;
use steel_registry::holder::RegistryHolder;
use steel_registry::items::ItemRef;
use steel_registry::recipe::RecipeBookCategoryRef;
use steel_registry::registry::TaggedRegistryExt as _;
use steel_registry::trim_pattern::TrimPattern;
use steel_registry::{REGISTRY, RegistryEntry as _};
use steel_utils::Identifier;
use steel_utils::codec::VarInt;
use steel_utils::serial::WriteTo;

/// Vanilla `RecipeDisplayId`: a `VarInt` index into the server's recipe display list.
#[derive(ReadFrom, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecipeDisplayId {
    /// Index of the display in the server's recipe display list.
    #[read(as = VarInt)]
    pub index: i32,
}

impl WriteTo for RecipeDisplayId {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.index).write(writer)
    }
}

/// A reference to a registered data component type.
///
/// Mirrors vanilla's `DataComponentType<?>` on the wire: the registry id of the
/// component type from `BuiltInRegistries.DATA_COMPONENT_TYPE`.
///
/// `ComponentEntry` has no `RegistryEntry` impl, so the id is resolved through
/// `DataComponentRegistry::id_from_key`, exactly like
/// [`DataComponentPatch`](steel_registry::data_components::DataComponentPatch).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ComponentTypeRef(pub ComponentEntryRef);

impl Debug for ComponentTypeRef {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ComponentTypeRef")
            .field(&self.0.key)
            .finish()
    }
}

impl WriteTo for ComponentTypeRef {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.0.id() as i32).write(writer)
    }
}

/// Vanilla's item-codec holder (`ByteBufCodecs.holderRegistry(Registries.ITEM)`).
///
/// A newtype so the display enums can derive both directions while `Item` stays
/// free of holder impls: Steel's `RegistryHolderEntry`/`RegistryHolderSetEntry`
/// are not implemented for `Item`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemHolder(pub ItemRef);

impl WriteTo for ItemHolder {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        VarInt(self.0.id() as i32).write(writer)
    }
}

/// Vanilla's homogeneous item `HolderSet` (`ByteBufCodecs.holderSet(Registries.ITEM)`).
///
/// Wire format: `VarInt(0)` followed by the tag identifier for a named tag,
/// otherwise `VarInt(count + 1)` followed by one item holder per entry.
///
/// Vanilla's `Ingredient` rejects empty direct sets, so callers must not send
/// `Direct(vec![])` in ingredient positions; an empty ingredient is expressed as
/// `None` in [`RecipeDisplayEntry::crafting_requirements`] instead.
#[derive(Clone, Debug)]
pub enum ItemHolderSet {
    /// A named item registry tag (wire id `0`).
    Tag(Identifier),
    /// An ordered list of direct item references.
    Direct(Vec<ItemRef>),
}

impl WriteTo for ItemHolderSet {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match self {
            Self::Tag(tag) => {
                if REGISTRY.items.get_tag(tag).is_none() {
                    return Err(Error::other(format!("Unknown item tag: {tag}")));
                }
                VarInt(0).write(writer)?;
                tag.write(writer)
            }
            Self::Direct(entries) => {
                let count = i32::try_from(entries.len())
                    .map_err(|_| Error::other("item holder set is too large"))?;
                VarInt(count + 1).write(writer)?;
                for entry in entries {
                    VarInt(entry.id() as i32).write(writer)?;
                }
                Ok(())
            }
        }
    }
}

/// Vanilla's trim-pattern holder (`TrimPattern.STREAM_CODEC`, a `holderRegistry`).
///
/// Uses Steel's `RegistryHolder<TrimPattern>`, whose reference encoding matches
/// vanilla (`id + 1`, with `0` meaning an inline value).
///
/// Note: a direct (inline) value would be written with `TrimPatternValue`'s
/// stream impl, which emits the datafixer codec NBT rather than vanilla's direct
/// network stream codec. Display payloads are expected to carry references only.
#[derive(Clone, Debug)]
pub struct TrimPatternHolder(pub RegistryHolder<TrimPattern>);

impl WriteTo for TrimPatternHolder {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.0.write(writer)
    }
}

/// Wire mirror of vanilla's `SlotDisplay` dispatch.
///
/// Variant order matches the vanilla registry ids produced by
/// `SlotDisplays.bootstrap`.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "variants mirror vanilla SlotDisplays payloads in registry-id order; boxing them would distort the wire mirror"
)]
pub enum SlotDisplay {
    /// `minecraft:empty` (id 0): renders nothing, no payload.
    Empty,
    /// `minecraft:any_fuel` (id 1): every item that has the cooking fuel component, no payload.
    AnyFuel,
    /// `minecraft:with_any_potion` (id 2): the contents with every potion applied.
    WithAnyPotion(Box<SlotDisplay>),
    /// `minecraft:only_with_component` (id 3): the contents that carry `component`.
    OnlyWithComponent {
        /// Wrapped display whose resolved stacks are filtered.
        contents: Box<SlotDisplay>,
        /// Component type that must be present on the resolved stacks.
        component: ComponentTypeRef,
    },
    /// `minecraft:item` (id 4): a single item type.
    Item(ItemHolder),
    /// `minecraft:item_stack` (id 5): an item, count and component patch
    /// (vanilla `ItemStackTemplate.STREAM_CODEC`).
    ItemStack(ItemStackTemplate),
    /// `minecraft:tag` (id 6): every item of an item tag.
    Tag(ItemHolderSet),
    /// `minecraft:dyed` (id 7): the target dyed with the random dye color.
    Dyed {
        /// Display resolving to the dye item.
        dye: Box<SlotDisplay>,
        /// Display resolving to the item being dyed.
        target: Box<SlotDisplay>,
    },
    /// `minecraft:smithing_trim` (id 8): the base trimmed with the material and pattern.
    SmithingTrim {
        /// Display resolving to the base item.
        base: Box<SlotDisplay>,
        /// Display resolving to the trim material.
        material: Box<SlotDisplay>,
        /// Trim pattern applied to the base item.
        pattern: TrimPatternHolder,
    },
    /// `minecraft:with_remainder` (id 9): the input, plus its crafting remainder.
    WithRemainder {
        /// Display resolving to the input item.
        input: Box<SlotDisplay>,
        /// Display resolving to the crafting remainder.
        remainder: Box<SlotDisplay>,
    },
    /// `minecraft:composite` (id 10): the concatenation of every nested display.
    Composite(Vec<SlotDisplay>),
}

impl WriteTo for SlotDisplay {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match self {
            Self::Empty => VarInt(0).write(writer),
            Self::AnyFuel => VarInt(1).write(writer),
            Self::WithAnyPotion(display) => {
                VarInt(2).write(writer)?;
                display.write(writer)
            }
            Self::OnlyWithComponent {
                contents,
                component,
            } => {
                VarInt(3).write(writer)?;
                contents.write(writer)?;
                component.write(writer)
            }
            Self::Item(item) => {
                VarInt(4).write(writer)?;
                item.write(writer)
            }
            Self::ItemStack(stack) => {
                VarInt(5).write(writer)?;
                stack.write(writer)
            }
            Self::Tag(tag) => {
                VarInt(6).write(writer)?;
                tag.write(writer)
            }
            Self::Dyed { dye, target } => {
                VarInt(7).write(writer)?;
                dye.write(writer)?;
                target.write(writer)
            }
            Self::SmithingTrim {
                base,
                material,
                pattern,
            } => {
                VarInt(8).write(writer)?;
                base.write(writer)?;
                material.write(writer)?;
                pattern.write(writer)
            }
            Self::WithRemainder { input, remainder } => {
                VarInt(9).write(writer)?;
                input.write(writer)?;
                remainder.write(writer)
            }
            Self::Composite(contents) => {
                VarInt(10).write(writer)?;
                contents.write(writer)
            }
        }
    }
}

/// Wire mirror of vanilla's `RecipeDisplay` dispatch.
///
/// Variant order matches the vanilla registry ids produced by
/// `RecipeDisplays.bootstrap`.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "variants mirror vanilla RecipeDisplays payloads in registry-id order; boxing them would distort the wire mirror"
)]
pub enum RecipeDisplay {
    /// `minecraft:crafting_shapeless` (id 0).
    ShapelessCrafting {
        /// Slot displays for the shapeless ingredients.
        ingredients: Vec<SlotDisplay>,
        /// Slot display for the crafting result.
        result: SlotDisplay,
        /// Slot display for the crafting station (the crafting table).
        crafting_station: SlotDisplay,
    },
    /// `minecraft:crafting_shaped` (id 1).
    ShapedCrafting {
        /// Recipe width in slots.
        width: i32,
        /// Recipe height in slots.
        height: i32,
        /// `width * height` slot displays, row-major.
        ingredients: Vec<SlotDisplay>,
        /// Slot display for the crafting result.
        result: SlotDisplay,
        /// Slot display for the crafting station (the crafting table).
        crafting_station: SlotDisplay,
    },
    /// `minecraft:furnace` (id 2): smelting, blasting, smoking and campfire cooking.
    Furnace {
        /// Slot display for the smelted ingredient.
        ingredient: SlotDisplay,
        /// Slot display for the fuel, or any fuel.
        fuel: SlotDisplay,
        /// Slot display for the smelting result.
        result: SlotDisplay,
        /// Slot display for the crafting station (the furnace).
        crafting_station: SlotDisplay,
        /// Cooking time in ticks.
        duration: i32,
        /// Experience granted per smelted item.
        experience: f32,
    },
    /// `minecraft:stonecutter` (id 3).
    Stonecutter {
        /// Slot display for the stonecutter input.
        input: SlotDisplay,
        /// Slot display for the stonecutting result.
        result: SlotDisplay,
        /// Slot display for the crafting station (the stonecutter).
        crafting_station: SlotDisplay,
    },
    /// `minecraft:smithing` (id 4).
    Smithing {
        /// Slot display for the smithing template.
        template: SlotDisplay,
        /// Slot display for the base item.
        base: SlotDisplay,
        /// Slot display for the addition item.
        addition: SlotDisplay,
        /// Slot display for the smithing result.
        result: SlotDisplay,
        /// Slot display for the crafting station (the smithing table).
        crafting_station: SlotDisplay,
    },
}

impl WriteTo for RecipeDisplay {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        match self {
            Self::ShapelessCrafting {
                ingredients,
                result,
                crafting_station,
            } => {
                VarInt(0).write(writer)?;
                ingredients.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
            Self::ShapedCrafting {
                width,
                height,
                ingredients,
                result,
                crafting_station,
            } => {
                VarInt(1).write(writer)?;
                VarInt(*width).write(writer)?;
                VarInt(*height).write(writer)?;
                ingredients.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
            Self::Furnace {
                ingredient,
                fuel,
                result,
                crafting_station,
                duration,
                experience,
            } => {
                VarInt(2).write(writer)?;
                ingredient.write(writer)?;
                fuel.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)?;
                VarInt(*duration).write(writer)?;
                experience.write(writer)
            }
            Self::Stonecutter {
                input,
                result,
                crafting_station,
            } => {
                VarInt(3).write(writer)?;
                input.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
            Self::Smithing {
                template,
                base,
                addition,
                result,
                crafting_station,
            } => {
                VarInt(4).write(writer)?;
                template.write(writer)?;
                base.write(writer)?;
                addition.write(writer)?;
                result.write(writer)?;
                crafting_station.write(writer)
            }
        }
    }
}

/// Wire mirror of vanilla's `RecipeDisplayEntry`.
///
/// Field order and widths follow `RecipeDisplayEntry.STREAM_CODEC`: display id,
/// display, `OPTIONAL_VAR_INT` group, recipe-book category registry id, then the
/// optional crafting requirement ingredient list.
#[derive(Clone, Debug)]
pub struct RecipeDisplayEntry {
    /// Identifier of this display inside the recipe book.
    pub id: RecipeDisplayId,
    /// The display shown by the recipe book.
    pub display: RecipeDisplay,
    /// Optional group/row the display belongs to (`0` on the wire means absent).
    pub group: Option<i32>,
    /// Recipe-book category this display is filed under.
    pub category: RecipeBookCategoryRef,
    /// Ingredients required to craft the display, or `None` when the server did
    /// not send any. Each ingredient is a vanilla `Ingredient.CONTENTS_STREAM_CODEC`
    /// item `holderSet`.
    pub crafting_requirements: Option<Vec<ItemHolderSet>>,
}

impl WriteTo for RecipeDisplayEntry {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        self.id.write(writer)?;
        self.display.write(writer)?;
        // Vanilla `ByteBufCodecs.OPTIONAL_VAR_INT`: 0 means empty, otherwise `value + 1`.
        VarInt(self.group.map_or(0, |group| group + 1)).write(writer)?;
        VarInt(self.category.id() as i32).write(writer)?;
        self.crafting_requirements.write(writer)
    }
}
