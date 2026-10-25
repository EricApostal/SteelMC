//! Data-driven villager trades: `minecraft:villager_trade` offers and
//! `minecraft:trade_set` level sets, mirroring vanilla
//! `world.item.trading.{VillagerTrade,TradeCost,TradeSet}`.

use rustc_hash::FxHashMap;
use serde_json::Value;
use steel_utils::Identifier;

use crate::data_component_predicate::DataComponentExactPredicate;
use crate::data_components::{ComponentData, ComponentEntryRef};
use crate::enchantment::EnchantmentRef;
use crate::item_stack_template::ItemStackTemplate;
use crate::items::ItemRef;
use crate::loot_table::NumberProvider;
use crate::villager_type::VillagerTypeRef;
use crate::{REGISTRY, RegistryExt, RegistryTags};

/// Vanilla `LootItemFunction` (`net.minecraft.world.level.storage.loot.functions.LootItemFunction`).
///
/// Vanilla's `LootItemFunction` is the conditional wrapper
/// (`LootItemConditionalFunction`), which Steel models as
/// [`crate::loot_table::ConditionalLootFunction`]; the un-conditional body is
/// [`crate::loot_table::LootFunction`].
pub use crate::loot_table::ConditionalLootFunction as LootItemFunction;

/// Vanilla `TradeCost`: one item cost of an offer.
#[derive(Debug, Clone)]
pub struct TradeCost {
    pub item: ItemRef,
    pub count: NumberProvider,
    /// Vanilla `TradeCost.components`.
    ///
    /// Resolved lazily: the exact predicate is built from the data-component
    /// registry, which is not yet published while the vanilla registry itself is
    /// being constructed (trade statics are registered at that point).
    pub components: &'static std::sync::LazyLock<DataComponentExactPredicate>,
}

impl TradeCost {
    /// The resolved component predicate.
    #[must_use]
    pub fn components(&self) -> &DataComponentExactPredicate {
        self.components
    }
}

/// Shared empty component predicate for trades without `components`.
pub static EMPTY_COMPONENTS: std::sync::LazyLock<DataComponentExactPredicate> =
    std::sync::LazyLock::new(|| DataComponentExactPredicate::EMPTY);

/// Vanilla `VillagerTrade`: one `villager_trade` registry entry.
#[derive(Debug)]
pub struct VillagerTrade {
    pub key: Identifier,
    pub wants: TradeCost,
    pub additional_wants: Option<TradeCost>,
    pub gives: ItemStackTemplate,
    pub max_uses: NumberProvider,
    pub xp: NumberProvider,
    pub reputation_discount: NumberProvider,
    /// Vanilla `merchant_predicate`. The extracted data only ever uses the
    /// `minecraft:entity_properties` predicate checking `minecraft:villager/variant`
    /// against `this`, so the generator rejects any other shape and stores the
    /// referenced villager types directly.
    pub variant_predicate: Option<&'static [VillagerTypeRef]>,
    pub given_item_modifier: &'static [LootItemFunction],
    pub double_trade_price_enchantments: &'static [EnchantmentRef],
}

/// Vanilla `TradeSet`: one `trade_set` registry entry.
#[derive(Debug)]
pub struct TradeSet {
    pub key: Identifier,
    pub trades: &'static [&'static VillagerTrade],
    pub amount: NumberProvider,
    pub allow_duplicates: bool,
    pub random_sequence: Option<Identifier>,
}

pub type VillagerTradeRef = &'static VillagerTrade;
pub type TradeSetRef = &'static TradeSet;

pub struct VillagerTradeRegistry {
    villager_trades_by_id: Vec<VillagerTradeRef>,
    villager_trades_by_key: FxHashMap<Identifier, usize>,
    tags: RegistryTags,
    allows_registering: bool,
}

impl VillagerTradeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            villager_trades_by_id: Vec::new(),
            villager_trades_by_key: FxHashMap::default(),
            tags: RegistryTags::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    VillagerTradeRegistry,
    VillagerTradeRef,
    villager_trades_by_id,
    villager_trades_by_key,
    allows_registering
);

crate::impl_registry!(
    VillagerTradeRegistry,
    VillagerTrade,
    villager_trades_by_id,
    villager_trades_by_key,
    villager_trades
);
crate::impl_tagged_registry!(
    VillagerTradeRegistry,
    villager_trades_by_key,
    "villager trade"
);

pub struct TradeSetRegistry {
    trade_sets_by_id: Vec<TradeSetRef>,
    trade_sets_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl TradeSetRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            trade_sets_by_id: Vec::new(),
            trade_sets_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    TradeSetRegistry,
    TradeSetRef,
    trade_sets_by_id,
    trade_sets_by_key,
    allows_registering
);

crate::impl_registry!(
    TradeSetRegistry,
    TradeSet,
    trade_sets_by_id,
    trade_sets_by_key,
    trade_sets
);

/// Builds the `DataComponentExactPredicate` of a `TradeCost` from its extracted
/// JSON form.
///
/// Vanilla reads `TradeCost.components` with `DataComponentExactPredicate.CODEC`,
/// which is backed by the component JSON codecs. Steel stores components as NBT,
/// and the extracted trade data only uses string/number shapes whose JSON and NBT
/// forms coincide, so the JSON is converted to NBT and decoded through each
/// component's persistent codec. A shape that cannot round-trip is a hard error.
///
/// # Panics
/// Panics if `json` is not a JSON object of valid vanilla data components.
#[must_use]
pub fn exact_components_from_json(json: &str) -> DataComponentExactPredicate {
    let value: Value = serde_json::from_str(json)
        .unwrap_or_else(|error| panic!("invalid trade cost components {json}: {error}"));
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("trade cost components must be a JSON object: {json}"));

    let mut values: Vec<(ComponentEntryRef, ComponentData)> = Vec::with_capacity(object.len());
    for (key, component) in object {
        let id: Identifier = key
            .parse()
            .unwrap_or_else(|error| panic!("invalid component key {key:?}: {error}"));
        let entry = REGISTRY
            .data_components
            .by_key(&id)
            .unwrap_or_else(|| panic!("unknown data component {id}"));
        let decoded = entry
            .read_nbt_owned(&json_to_nbt(component))
            .unwrap_or_else(|| panic!("data component {id} rejected {component}"));
        values.push((entry, decoded));
    }

    DataComponentExactPredicate::new(values)
        .unwrap_or_else(|| panic!("trade cost components are not an exact predicate: {json}"))
}

/// Converts the extracted component JSON into the NBT shape the persistent
/// component codecs decode.
fn json_to_nbt(value: &Value) -> simdnbt::owned::NbtTag {
    use simdnbt::owned::{NbtCompound, NbtList, NbtTag};

    match value {
        Value::Null => NbtTag::Compound(NbtCompound::new()),
        Value::Bool(value) => NbtTag::Byte(i8::from(*value)),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                if let Ok(value) = i32::try_from(value) {
                    NbtTag::Int(value)
                } else {
                    NbtTag::Long(value)
                }
            } else {
                NbtTag::Double(number.as_f64().unwrap_or_default())
            }
        }
        Value::String(value) => NbtTag::String(value.as_str().into()),
        Value::Array(entries) => NbtTag::List(NbtList::from(
            entries.iter().map(json_to_nbt).collect::<Vec<_>>(),
        )),
        Value::Object(entries) => {
            let mut compound = NbtCompound::new();
            for (key, entry) in entries {
                compound.insert(key.as_str(), json_to_nbt(entry));
            }
            NbtTag::Compound(compound)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init_vanilla_registry;
    use crate::loot_table::LootFunction;
    use crate::vanilla_items;
    use crate::vanilla_trade_sets::{FARMER_LEVEL_1, FARMER_LEVEL_5, WANDERING_TRADER_COMMON};
    use crate::villager_profession::VillagerProfessionRef;
    use crate::{vanilla_trade_sets, vanilla_villager_trades};

    fn init() {
        init_vanilla_registry();
    }

    #[test]
    fn villager_trade_registry_is_contiguous() {
        init();
        let registry = &REGISTRY.villager_trades;
        assert!(registry.len() > 300);
        for id in 0..registry.len() {
            let trade = registry
                .by_id(id)
                .unwrap_or_else(|| panic!("missing villager trade at id {id}"));
            assert_eq!(registry.id_from_key(&trade.key), Some(id));
        }
    }

    #[test]
    fn trade_set_registry_is_contiguous() {
        init();
        let registry = &REGISTRY.trade_sets;
        assert_eq!(registry.len(), 68);
        for id in 0..registry.len() {
            let set = registry
                .by_id(id)
                .unwrap_or_else(|| panic!("missing trade set at id {id}"));
            assert_eq!(registry.id_from_key(&set.key), Some(id));
        }
    }

    #[test]
    fn farmer_level_one_wheat_trade_resolves() {
        init();
        let trade = &*vanilla_villager_trades::FARMER_1_WHEAT_EMERALD;
        assert_eq!(
            trade.key,
            Identifier::vanilla_static("farmer/1/wheat_emerald")
        );
        assert_eq!(trade.wants.item.key, vanilla_items::WHEAT.key);
        assert!(matches!(trade.wants.count, NumberProvider::Constant(20.0)));
        assert!(trade.wants.components().is_empty());
        assert_eq!(trade.gives.item().key, vanilla_items::EMERALD.key);
        assert_eq!(trade.gives.count(), 1);
        assert!(matches!(trade.max_uses, NumberProvider::Constant(16.0)));
        assert!(matches!(trade.xp, NumberProvider::Constant(2.0)));
        assert!(matches!(
            trade.reputation_discount,
            NumberProvider::Constant(value) if (value - 0.05).abs() < f32::EPSILON
        ));
        assert!(trade.additional_wants.is_none());
        assert!(trade.variant_predicate.is_none());
        assert!(trade.given_item_modifier.is_empty());
        assert_eq!(
            trade.double_trade_price_enchantments,
            [] as [EnchantmentRef; 0]
        );

        let set = &*FARMER_LEVEL_1;
        assert_eq!(set.key, Identifier::vanilla_static("farmer/level_1"));
        assert!(matches!(set.amount, NumberProvider::Constant(2.0)));
        assert!(!set.allow_duplicates);
        assert_eq!(
            set.random_sequence,
            Some(Identifier::vanilla_static("trade_set/farmer/level_1"))
        );
        assert!(
            set.trades
                .iter()
                .any(|candidate| candidate.key == trade.key),
            "farmer level 1 tag must contain the wheat trade"
        );
    }

    #[test]
    fn professions_cover_levels_one_to_five() {
        init();
        let with_trades: Vec<VillagerProfessionRef> = REGISTRY
            .villager_professions
            .iter()
            .filter(|(_, profession)| !profession.trade_sets_by_level.is_empty())
            .map(|(_, profession)| profession)
            .collect();
        assert_eq!(with_trades.len(), 13);
        for profession in with_trades {
            let levels: Vec<i32> = profession
                .trade_sets_by_level
                .iter()
                .map(|(level, _)| *level)
                .collect();
            assert_eq!(levels, vec![1, 2, 3, 4, 5], "{}", profession.key);
            for (level, set) in profession.trade_sets_by_level {
                let expected = format!("{}/level_{level}", profession.key.path);
                assert_eq!(set.key.path.as_ref(), expected);
                assert_ne!(set.trades, [] as [VillagerTradeRef; 0]);
            }
        }
    }

    #[test]
    fn trade_set_amounts_and_providers_parse() {
        init();
        assert!(matches!(
            vanilla_trade_sets::LIBRARIAN_LEVEL_5.amount,
            NumberProvider::Constant(3.0)
        ));
        assert!(matches!(
            WANDERING_TRADER_COMMON.amount,
            NumberProvider::Constant(5.0)
        ));
        assert!(matches!(
            FARMER_LEVEL_5.amount,
            NumberProvider::Constant(2.0)
        ));

        // Librarian level 5 enchanted books have a fixed base price (`count: 0`);
        // the enchantment value is added by the `enchant_randomly` modifier at
        // offer-generation time.
        let book = &*vanilla_villager_trades::LIBRARIAN_5_EMERALD_AND_BOOK_SNOW_ENCHANTED_BOOK;
        assert!(matches!(book.wants.count, NumberProvider::Constant(0.0)));
        assert_eq!(
            book.additional_wants
                .as_ref()
                .map(|cost| cost.item.key.clone()),
            Some(Identifier::vanilla_static("book"))
        );

        // The leatherworker dye trades use the vanilla `add` provider:
        // `{"type": "add", "inputs": [1, {"type": "binomial", "n": 2, "p": 0.75}]}`.
        let dyed = &*vanilla_villager_trades::LEATHERWORKER_1_EMERALD_DYED_LEATHER_CHESTPLATE;
        let LootItemFunction {
            function: LootFunction::SetRandomDyes { number_of_dyes },
            ..
        } = &dyed.given_item_modifier[0]
        else {
            panic!(
                "expected a set_random_dyes modifier, got {:?}",
                dyed.given_item_modifier
            );
        };
        let NumberProvider::Add { inputs } = number_of_dyes else {
            panic!("expected an add number provider, got {number_of_dyes:?}");
        };
        assert_eq!(inputs.len(), 2);
        assert!(matches!(inputs[0], NumberProvider::Constant(1.0)));
        assert!(matches!(
            inputs[1],
            NumberProvider::Binomial { n: 2, p } if (p - 0.75).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn merchant_predicates_and_enchantment_tags_resolve() {
        init();
        let plains = &*vanilla_villager_trades::ARMORER_4_EMERALD_ENCHANTED_IRON_BOOTS_PLAINS;
        let variants = plains.variant_predicate.expect("plains trade predicate");
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].key, Identifier::vanilla_static("plains"));

        let book = &*vanilla_villager_trades::LIBRARIAN_1_EMERALD_AND_BOOK_PLAINS_ENCHANTED_BOOK;
        assert_eq!(book.double_trade_price_enchantments.len(), 7);
        assert!(
            book.double_trade_price_enchantments
                .contains(&&crate::vanilla_enchantments::MENDING)
        );
        assert_eq!(book.given_item_modifier.len(), 2);
    }
}
