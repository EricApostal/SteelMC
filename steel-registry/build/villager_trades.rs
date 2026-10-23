//! Build-time generation for the `minecraft:villager_trade` registry.
//!
//! Reads the vendored vanilla datapack `villager_trade/**` entries plus the
//! default-enabled `trade_rebalance` overlay, which overrides individual
//! `villager_trade` files.

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use steel_utils::Identifier;

use crate::generator_functions::generate_item_stack_template;
use crate::loot_tables::{
    ModifierJson, NumberProviderJson, generate_function, generate_number_provider,
};
use crate::shared_structs::ItemStackTemplateJson;
use crate::tag_walk;

pub(crate) const BASE_DIR: &str =
    "../steel-utils/build_assets/builtin_datapacks/minecraft/villager_trade";
pub(crate) const OVERLAY_DIR: &str = "../steel-utils/build_assets/builtin_datapacks/minecraft/datapacks/trade_rebalance/data/minecraft/villager_trade";

/// Vanilla `TradeCost` / `ItemStackTemplate` JSON.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TradeCostJson {
    id: Identifier,
    #[serde(default = "default_count")]
    count: NumberProviderJson,
    /// Vanilla `DataComponentExactPredicate` JSON, passed verbatim to the runtime
    /// `trade::exact_components_from_json` bridge.
    #[serde(default)]
    components: Option<serde_json::Value>,
}

fn default_count() -> NumberProviderJson {
    NumberProviderJson::Constant(1.0)
}

/// Vanilla `VillagerTrade.CODEC`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VillagerTradeJson {
    wants: TradeCostJson,
    #[serde(default)]
    additional_wants: Option<TradeCostJson>,
    gives: ItemStackTemplateJson,
    #[serde(default)]
    max_uses: Option<NumberProviderJson>,
    #[serde(default)]
    xp: Option<NumberProviderJson>,
    #[serde(default)]
    reputation_discount: Option<NumberProviderJson>,
    #[serde(default)]
    merchant_predicate: Option<serde_json::Value>,
    #[serde(default)]
    given_item_modifier: Option<ModifierJson>,
    #[serde(default)]
    double_trade_price_enchantments: Option<serde_json::Value>,
}

/// Vanilla `VillagerTrade` defaults (`ContextIntProviders.exactly`).
const DEFAULT_MAX_USES: f32 = 4.0;
const DEFAULT_XP: f32 = 1.0;
const DEFAULT_REPUTATION_DISCOUNT: f32 = 0.0;

pub(crate) fn build() -> TokenStream {
    println!("cargo:rerun-if-changed={BASE_DIR}");
    println!("cargo:rerun-if-changed={OVERLAY_DIR}");

    let trades = read_trades();
    let villager_types = tag_walk::villager_type_statics();
    let enchantment_tags = tag_walk::read_tags(
        "../steel-utils/build_assets/builtin_datapacks/minecraft/tags/enchantment",
        None,
        "enchantment",
    );
    let enchantment_statics = tag_walk::enchantment_names();
    let item_statics = tag_walk::item_names();

    let mut constants = TokenStream::new();
    let mut registrations = TokenStream::new();

    for (key, trade) in &trades {
        let ident = Ident::new(&key.to_shouty_snake_case(), Span::call_site());

        let wants = generate_trade_cost(&trade.wants, key, "wants", &item_statics);
        let additional_wants = trade.additional_wants.as_ref().map_or_else(
            || quote! { None },
            |cost| {
                let cost = generate_trade_cost(cost, key, "additional_wants", &item_statics);
                quote! { Some(#cost) }
            },
        );
        let gives = generate_item_stack_template(&trade.gives);
        let max_uses = generate_number_provider(
            trade
                .max_uses
                .as_ref()
                .unwrap_or(&NumberProviderJson::Constant(DEFAULT_MAX_USES)),
        );
        let xp = generate_number_provider(
            trade
                .xp
                .as_ref()
                .unwrap_or(&NumberProviderJson::Constant(DEFAULT_XP)),
        );
        let reputation_discount = generate_number_provider(
            trade
                .reputation_discount
                .as_ref()
                .unwrap_or(&NumberProviderJson::Constant(DEFAULT_REPUTATION_DISCOUNT)),
        );

        let mut slice_statics = TokenStream::new();

        // Slice literals inside a `LazyLock` closure are temporaries, so every
        // non-empty list is hoisted into its own `static` for a `'static` borrow.
        let variant_predicate = trade.merchant_predicate.as_ref().map_or_else(
            || quote! { None },
            |predicate| {
                let values = generate_variant_predicate(predicate, key, &villager_types);
                let slice_ident = Ident::new(
                    &format!("{}_VARIANT_PREDICATE", key.to_shouty_snake_case()),
                    Span::call_site(),
                );
                slice_statics.extend(quote! {
                    static #slice_ident: &[crate::villager_type::VillagerTypeRef] = #values;
                });
                quote! { Some(#slice_ident) }
            },
        );

        let given_item_modifier: Vec<TokenStream> = trade
            .given_item_modifier
            .as_ref()
            .map_or_default(ModifierJson::functions)
            .into_iter()
            .map(generate_function)
            .collect();
        let given_item_modifier = if given_item_modifier.is_empty() {
            quote! { &[] }
        } else {
            let slice_ident = Ident::new(
                &format!("{}_GIVEN_ITEM_MODIFIER", key.to_shouty_snake_case()),
                Span::call_site(),
            );
            slice_statics.extend(quote! {
                static #slice_ident: &[crate::loot_table::ConditionalLootFunction] =
                    &[#(#given_item_modifier),*];
            });
            quote! { #slice_ident }
        };

        let double_trade_price_enchantments =
            trade.double_trade_price_enchantments.as_ref().map_or_else(
                || quote! { &[] },
                |json| {
                    let values = generate_enchantment_holder_set(
                        json,
                        key,
                        &enchantment_tags,
                        &enchantment_statics,
                    );
                    let slice_ident = Ident::new(
                        &format!(
                            "{}_DOUBLE_TRADE_PRICE_ENCHANTMENTS",
                            key.to_shouty_snake_case()
                        ),
                        Span::call_site(),
                    );
                    slice_statics.extend(quote! {
                        static #slice_ident: &[crate::enchantment::EnchantmentRef] = #values;
                    });
                    quote! { #slice_ident }
                },
            );

        constants.extend(quote! {
            #slice_statics

            pub static #ident: LazyLock<VillagerTrade> = LazyLock::new(|| VillagerTrade {
                key: Identifier::vanilla_static(#key),
                wants: #wants,
                additional_wants: #additional_wants,
                gives: #gives,
                max_uses: #max_uses,
                xp: #xp,
                reputation_discount: #reputation_discount,
                variant_predicate: #variant_predicate,
                given_item_modifier: #given_item_modifier,
                double_trade_price_enchantments: #double_trade_price_enchantments,
            });
        });
        registrations.extend(quote! {
            registry.register(&#ident);
        });
    }

    quote! {
        use crate::data_component_predicate::DataComponentExactPredicate;
        use crate::item_stack_template::ItemStackTemplate;
        use crate::loot_table::{
            BlockPredicate, BonusFormula, ConditionalLootFunction, CopySource, DyeColor,
            EnchantedChance, EnchantmentMatch, EnchantmentOptions, EnchantmentSource,
            InstrumentOptions, ItemPredicate, ItemSetPredicate, LootCondition, LootContextEntity,
            LootFunction, NameTarget, NumberProvider, PotionOptions, PropertyCheck, StewEffect,
            ToolPredicate,
        };
        use crate::trade::{TradeCost, VillagerTrade, VillagerTradeRegistry};
        use crate::vanilla_enchantments;
        use crate::vanilla_items;
        use crate::vanilla_villager_types;
        use steel_utils::Identifier;
        use std::sync::LazyLock;

        #constants

        pub fn register_villager_trades(registry: &mut VillagerTradeRegistry) {
            #registrations
        }
    }
}

/// Reads every villager trade, applying the trade-rebalance overlay last.
pub(crate) fn read_trades() -> BTreeMap<String, VillagerTradeJson> {
    let mut trades: BTreeMap<String, VillagerTradeJson> = BTreeMap::new();
    for dir in [BASE_DIR, OVERLAY_DIR] {
        for (key, json) in tag_walk::read_registry_dir(dir, "villager_trade") {
            let trade: VillagerTradeJson = serde_json::from_str(&json)
                .unwrap_or_else(|error| panic!("Failed to parse villager trade {key}: {error}"));
            trades.insert(key, trade);
        }
    }
    assert!(!trades.is_empty(), "no villager trades found in {BASE_DIR}");
    trades
}

/// Emits a `TradeCost` from its extracted JSON form.
fn generate_trade_cost(
    cost: &TradeCostJson,
    key: &str,
    side: &str,
    item_statics: &BTreeSet<String>,
) -> TokenStream {
    let item = tag_walk::generate_item_ref(
        &cost.id,
        &format!("villager trade {key} {side}"),
        item_statics,
    );
    let count = generate_number_provider(&cost.count);
    let components = cost.components.as_ref().map_or_else(
        || quote! { &crate::trade::EMPTY_COMPONENTS },
        |components| {
            let json = components.to_string();
            let ident = Ident::new(
                &format!("{}_COMPONENTS", key.to_shouty_snake_case()),
                Span::call_site(),
            );
            quote! {
                {
                    static #ident: LazyLock<DataComponentExactPredicate> =
                        LazyLock::new(|| crate::trade::exact_components_from_json(#json));
                    &#ident
                }
            }
        },
    );
    quote! {
        TradeCost {
            item: #item,
            count: #count,
            components: #components,
        }
    }
}

/// Emits the `Option<&'static [VillagerTypeRef]>` of a `merchant_predicate`.
fn generate_variant_predicate(
    predicate: &serde_json::Value,
    key: &str,
    villager_types: &BTreeSet<String>,
) -> TokenStream {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct MerchantPredicateJson {
        #[serde(rename = "type")]
        predicate_type: String,
        entity: String,
        predicate: PredicateBodyJson,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PredicateBodyJson {
        #[serde(rename = "minecraft:predicates")]
        predicates: ComponentPredicatesJson,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ComponentPredicatesJson {
        #[serde(rename = "minecraft:villager/variant")]
        variant: OneOrMany,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(Identifier),
        Many(Vec<Identifier>),
    }

    let parsed: MerchantPredicateJson =
        serde_json::from_value(predicate.clone()).unwrap_or_else(|error| {
            panic!("villager trade {key} has an unsupported merchant_predicate: {error}")
        });
    assert_eq!(
        parsed.predicate_type, "minecraft:entity_properties",
        "villager trade {key} merchant_predicate must be `minecraft:entity_properties`"
    );
    assert_eq!(
        parsed.entity, "this",
        "villager trade {key} merchant_predicate must target `this`"
    );

    let variants = match parsed.predicate.predicates.variant {
        OneOrMany::One(variant) => vec![variant],
        OneOrMany::Many(variants) => variants,
    };
    assert!(
        !variants.is_empty(),
        "villager trade {key} merchant_predicate selects no villager variants"
    );

    let variants: Vec<TokenStream> = variants
        .into_iter()
        .map(|variant| {
            assert_eq!(
                variant.namespace.as_ref(),
                "minecraft",
                "villager trade {key} references a non-vanilla villager type {variant}"
            );
            assert!(
                villager_types.contains(variant.path.as_ref()),
                "villager trade {key} references unknown villager type {variant}"
            );
            let ident = Ident::new(&variant.path.to_shouty_snake_case(), Span::call_site());
            quote! { &vanilla_villager_types::#ident }
        })
        .collect();

    quote! { &[#(#variants),*] }
}

/// Emits the `&'static [EnchantmentRef]` of `double_trade_price_enchantments`,
/// resolving the referenced enchantment tag.
fn generate_enchantment_holder_set(
    json: &serde_json::Value,
    key: &str,
    tags: &BTreeMap<String, Vec<String>>,
    statics: &BTreeSet<String>,
) -> TokenStream {
    let reference = json.as_str().unwrap_or_else(|| {
        panic!("villager trade {key} has a non-string double_trade_price_enchantments: {json}")
    });
    let tag = reference.strip_prefix('#').unwrap_or_else(|| {
        panic!(
            "villager trade {key} double_trade_price_enchantments must reference a tag: {reference}"
        )
    });
    let tag = Identifier::from_str(tag).unwrap_or_else(|error| {
        panic!("villager trade {key} has an invalid enchantment tag: {error}")
    });
    assert_eq!(
        tag.namespace.as_ref(),
        "minecraft",
        "villager trade {key} references a non-vanilla enchantment tag {tag}"
    );

    let resolved = tag_walk::resolve_tag(tag.path.as_ref(), tags, "enchantment");
    let enchantments: Vec<TokenStream> = resolved
        .iter()
        .map(|path| {
            assert!(
                statics.contains(path),
                "villager trade {key} enchantment tag {} references unknown enchantment {path}",
                tag.path
            );
            let ident = Ident::new(&path.to_shouty_snake_case(), Span::call_site());
            quote! { &vanilla_enchantments::#ident }
        })
        .collect();
    assert!(
        !enchantments.is_empty(),
        "villager trade {key} enchantment tag {} resolved to no enchantments",
        tag.path
    );
    quote! { &[#(#enchantments),*] }
}
