//! Build-time generation for the `minecraft:trade_set` registry and the
//! `minecraft:villager_trade` tags that select offers per profession level.
//!
//! The default-enabled `trade_rebalance` datapack replaces the `armorer` and
//! `librarian` level tags, so tag resolution honours vanilla's `replace` flag.

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use steel_utils::Identifier;

use crate::loot_tables::{NumberProviderJson, generate_number_provider};
use crate::tag_walk;

pub(crate) const BASE_DIR: &str =
    "../steel-utils/build_assets/builtin_datapacks/minecraft/trade_set";
pub(crate) const OVERLAY_DIR: &str = "../steel-utils/build_assets/builtin_datapacks/minecraft/datapacks/trade_rebalance/data/minecraft/trade_set";
pub(crate) const TRADE_TAG_BASE_DIR: &str =
    "../steel-utils/build_assets/builtin_datapacks/minecraft/tags/villager_trade";
pub(crate) const TRADE_TAG_OVERLAY_DIR: &str = "../steel-utils/build_assets/builtin_datapacks/minecraft/datapacks/trade_rebalance/data/minecraft/tags/villager_trade";

/// Vanilla `TradeSet.CODEC`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TradeSetJson {
    /// Vanilla `RegistryCodecs.holderSet`, always a `#tag` for vanilla trade sets.
    trades: String,
    amount: NumberProviderJson,
    #[serde(default)]
    allow_duplicates: bool,
    #[serde(default)]
    random_sequence: Option<Identifier>,
}

/// All `trade_set` registry keys present in the vendored data, with the
/// default-enabled `trade_rebalance` overlay applied.
pub(crate) fn read_trade_set_keys() -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for dir in [BASE_DIR, OVERLAY_DIR] {
        for (key, _) in tag_walk::read_registry_dir(dir, "trade_set") {
            keys.insert(key);
        }
    }
    keys
}

pub(crate) fn build() -> TokenStream {
    println!("cargo:rerun-if-changed={BASE_DIR}");
    println!("cargo:rerun-if-changed={OVERLAY_DIR}");
    println!("cargo:rerun-if-changed={TRADE_TAG_BASE_DIR}");
    println!("cargo:rerun-if-changed={TRADE_TAG_OVERLAY_DIR}");

    let tags = tag_walk::read_tags(
        TRADE_TAG_BASE_DIR,
        Some(&[TRADE_TAG_OVERLAY_DIR]),
        "villager trade",
    );
    let trades = crate::villager_trades::read_trades();
    let trade_statics: BTreeSet<String> = trades.keys().cloned().collect();

    let mut trade_sets: BTreeMap<String, (TradeSetJson, Vec<String>)> = BTreeMap::new();
    for dir in [BASE_DIR, OVERLAY_DIR] {
        for (key, json) in tag_walk::read_registry_dir(dir, "trade_set") {
            let trade_set: TradeSetJson = serde_json::from_str(&json)
                .unwrap_or_else(|error| panic!("Failed to parse trade set {key}: {error}"));
            let tag = trade_set
                .trades
                .strip_prefix('#')
                .unwrap_or_else(|| panic!("trade set {key} must reference a trade tag"));
            let tag = Identifier::from_str(tag)
                .unwrap_or_else(|error| panic!("trade set {key} has an invalid tag: {error}"));
            assert_eq!(
                tag.namespace.as_ref(),
                "minecraft",
                "trade set {key} references a non-vanilla tag {tag}"
            );
            let resolved = tag_walk::resolve_tag(tag.path.as_ref(), &tags, "villager trade");
            assert!(
                !resolved.is_empty(),
                "trade set {key} tag {tag} resolved to no trades"
            );
            trade_sets.insert(key, (trade_set, resolved));
        }
    }
    assert!(!trade_sets.is_empty(), "no trade sets found in {BASE_DIR}");

    let mut constants = TokenStream::new();
    let mut registrations = TokenStream::new();

    for (key, (trade_set, resolved)) in &trade_sets {
        let ident = Ident::new(&key.to_shouty_snake_case(), Span::call_site());
        let trades: Vec<TokenStream> = resolved
            .iter()
            .map(|trade| {
                assert!(
                    trade_statics.contains(trade),
                    "trade set {key} references unknown villager trade {trade}"
                );
                let ident = Ident::new(&trade.to_shouty_snake_case(), Span::call_site());
                quote! { &*vanilla_villager_trades::#ident }
            })
            .collect();
        let amount = generate_number_provider(&trade_set.amount);
        let allow_duplicates = trade_set.allow_duplicates;
        let random_sequence = trade_set.random_sequence.as_ref().map_or_else(
            || quote! { None },
            |sequence| {
                assert_eq!(
                    sequence.namespace.as_ref(),
                    "minecraft",
                    "trade set {key} references a non-vanilla random sequence {sequence}"
                );
                let path = sequence.path.as_ref();
                quote! { Some(Identifier::vanilla_static(#path)) }
            },
        );

        let trades_ident = Ident::new(
            &format!("{}_TRADES", key.to_shouty_snake_case()),
            Span::call_site(),
        );

        constants.extend(quote! {
            static #trades_ident: LazyLock<Vec<&'static VillagerTrade>> =
                LazyLock::new(|| vec![#(#trades),*]);

            pub static #ident: LazyLock<TradeSet> = LazyLock::new(|| TradeSet {
                key: Identifier::vanilla_static(#key),
                // `#trades_ident` is a `static`, so this slice borrow is `'static`.
                trades: #trades_ident.as_slice(),
                amount: #amount,
                allow_duplicates: #allow_duplicates,
                random_sequence: #random_sequence,
            });
        });
        registrations.extend(quote! {
            registry.register(&#ident);
        });
    }

    // Register every villager trade tag, including the profession level tags that
    // trade sets reference. A few base `common_smith` tags are empty by default
    // (the `trade_rebalance` pack replaces armorer/librarian ones), and a
    // `trade_rebalance` data fixup in the vendored tree keeps `toolsmith` level 2
    // pointing at a removed set; both resolve to nothing and register empty.
    let mut tag_registrations = TokenStream::new();
    for (name, values) in &tags {
        let resolved = tag_walk::resolve_tag(name, &tags, "villager trade");
        for trade in &resolved {
            assert!(
                trade_statics.contains(trade),
                "villager trade tag {name} references unknown villager trade {trade} \
                 (declared values: {values:?})"
            );
        }
        let entries = resolved.iter().map(String::as_str);
        let path = name.as_str();
        tag_registrations.extend(quote! {
            registry.register_tag(Identifier::vanilla_static(#path), &[#(#entries),*]);
        });
    }

    quote! {
        use crate::trade::{TradeSet, TradeSetRegistry, VillagerTrade, VillagerTradeRegistry};
        use crate::vanilla_villager_trades;
        use crate::{TaggedRegistryExt, RegistryExt};
        use crate::loot_table::NumberProvider;
        use steel_utils::Identifier;
        use std::sync::LazyLock;

        #constants

        pub fn register_trade_sets(registry: &mut TradeSetRegistry) {
            #registrations
        }

        pub fn register_villager_trade_tags(registry: &mut VillagerTradeRegistry) {
            #tag_registrations
        }
    }
}
