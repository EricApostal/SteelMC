#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted predicate data"
)]

//! Build script for generating vanilla `predicate` registry entries.
//!
//! A `predicate` entry is a single loot condition. The decoder here is local to
//! this generator so the loot table generator's condition model stays private.

use std::{
    fs,
    path::{Path, PathBuf},
};

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use rustc_hash::FxHashMap;
use serde::Deserialize;

/// The `blocks` field of `match_block`: vanilla's `HolderSet<Block>` accepts a
/// single id/`#tag` or a list of them.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum BlocksJson {
    Single(String),
    List(Vec<String>),
}

/// `match_tool`'s `ItemPredicate.items` field, also a `HolderSet<Item>`.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum ItemsJson {
    Single(String),
    List(Vec<String>),
}

/// A `match_block` state property check.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum PropertyValueJson {
    Exact(String),
    Range {
        #[serde(default)]
        min: Option<String>,
        #[serde(default)]
        max: Option<String>,
    },
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct ToolPredicateJson {
    #[serde(default)]
    items: Option<ItemsJson>,
    #[serde(default)]
    predicates: Option<ToolPredicatesJson>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct ToolPredicatesJson {
    #[serde(rename = "minecraft:enchantments", default)]
    enchantments: Option<Vec<EnchantmentPredicateJson>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct EnchantmentPredicateJson {
    #[serde(default)]
    enchantments: Option<String>,
    #[serde(default)]
    levels: Option<LevelRangeJson>,
}

/// Vanilla `MinMaxBounds.Ints`: a bare integer or a `{min, max}` bounds object.
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum LevelRangeJson {
    Exact(i32),
    Bounds {
        #[serde(default)]
        min: Option<i32>,
        #[serde(default)]
        #[expect(
            dead_code,
            reason = "only the lower bound is needed to build ToolPredicate::HasEnchantment"
        )]
        max: Option<i32>,
    },
}

impl LevelRangeJson {
    const fn min_level(&self) -> Option<i32> {
        match self {
            Self::Exact(level) => Some(*level),
            Self::Bounds { min, .. } => *min,
        }
    }
}

/// The raw shape of a predicate file: a vanilla condition type id plus the
/// fields of the matching condition.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
struct PredicateJson {
    #[serde(rename = "type")]
    condition: String,
    // match_block (vanilla `BlockPredicate`)
    #[serde(default)]
    blocks: Option<BlocksJson>,
    #[serde(default, rename = "state")]
    properties: Option<FxHashMap<String, PropertyValueJson>>,
    // match_tool
    #[serde(default)]
    predicate: Option<ToolPredicateJson>,
}

fn strip_minecraft(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

/// Quotes a vanilla identifier, omitting the `minecraft:` namespace.
fn quote_identifier(value: &str) -> TokenStream {
    let path = strip_minecraft(value);
    quote! { Identifier::vanilla_static(#path) }
}

/// Quotes one `match_block` member. `properties` is shared across members.
fn quote_match_block(block: &str, properties: &[TokenStream]) -> TokenStream {
    assert!(
        !block.starts_with('#'),
        "match_block block tags are not modeled by Steel's LootCondition::MatchBlock: {block}"
    );
    let block = quote_identifier(block);
    quote! {
        LootCondition::MatchBlock {
            block: #block,
            properties: &[#(#properties),*],
        }
    }
}

fn quote_block_set(blocks: &BlocksJson, properties: &[TokenStream]) -> TokenStream {
    match blocks {
        BlocksJson::Single(block) => quote_match_block(block, properties),
        BlocksJson::List(blocks) => {
            let members: Vec<TokenStream> = blocks
                .iter()
                .map(|block| quote_match_block(block, properties))
                .collect();
            match members.as_slice() {
                [] => quote! { LootCondition::AnyOf(&[]) },
                [single] => single.clone(),
                _ => quote! { LootCondition::AnyOf(&[#(#members),*]) },
            }
        }
    }
}

/// Quotes a `ToolPredicate` for a single item id or `#tag`.
fn quote_tool_item(item: &str) -> TokenStream {
    if let Some(tag) = item.strip_prefix('#') {
        let tag = strip_minecraft(tag);
        quote! { ToolPredicate::Tag(Identifier::vanilla_static(#tag)) }
    } else {
        let item = quote_identifier(item);
        quote! { ToolPredicate::Item(#item) }
    }
}

/// Quotes a full `match_tool` condition. A holder set of several items becomes
/// an `AnyOf` over per-item `MatchTool` conditions, which is equivalent because
/// the tool predicate is the condition's only test.
fn quote_match_tool(predicate: &ToolPredicateJson) -> TokenStream {
    if let Some(items) = &predicate.items {
        return match items {
            ItemsJson::Single(item) => {
                let tool = quote_tool_item(item);
                quote! { LootCondition::MatchTool(#tool) }
            }
            ItemsJson::List(items) => {
                let members: Vec<TokenStream> = items
                    .iter()
                    .map(|item| {
                        let tool = quote_tool_item(item);
                        quote! { LootCondition::MatchTool(#tool) }
                    })
                    .collect();
                match members.as_slice() {
                    [] => quote! { LootCondition::MatchTool(ToolPredicate::Any) },
                    [single] => single.clone(),
                    _ => quote! { LootCondition::AnyOf(&[#(#members),*]) },
                }
            }
        };
    }

    if let Some(enchantments) = predicate
        .predicates
        .as_ref()
        .and_then(|predicates| predicates.enchantments.as_ref())
        && let Some(first) = enchantments.first()
        && let Some(enchantment) = &first.enchantments
    {
        let enchantment = quote_identifier(enchantment.trim_start_matches('#'));
        let min_level = first
            .levels
            .as_ref()
            .and_then(LevelRangeJson::min_level)
            .unwrap_or(1);
        return quote! {
            LootCondition::MatchTool(ToolPredicate::HasEnchantment {
                enchantment: #enchantment,
                min_level: #min_level,
            })
        };
    }

    quote! { LootCondition::MatchTool(ToolPredicate::Any) }
}

fn generate_condition(predicate: &PredicateJson) -> TokenStream {
    match predicate.condition.as_str() {
        "minecraft:match_block" => {
            let properties: Vec<TokenStream> =
                predicate
                    .properties
                    .as_ref()
                    .map_or_else(Vec::new, |properties| {
                        properties
                            .iter()
                            .map(|(name, value)| {
                                let value = match value {
                                    PropertyValueJson::Exact(value) => value.clone(),
                                    PropertyValueJson::Range { min, max } => format!(
                                        "{}..{}",
                                        min.as_deref().unwrap_or(""),
                                        max.as_deref().unwrap_or("")
                                    ),
                                };
                                quote! { PropertyCheck { name: #name, value: #value } }
                            })
                            .collect()
                    });
            match &predicate.blocks {
                Some(blocks) => quote_block_set(blocks, &properties),
                None => quote_match_block("minecraft:air", &properties),
            }
        }
        "minecraft:match_tool" => predicate.predicate.as_ref().map_or_else(
            || quote! { LootCondition::MatchTool(ToolPredicate::Any) },
            quote_match_tool,
        ),
        other => panic!(
            "predicate type {other} is not modeled by the predicate generator; \
             extend steel-registry/build/predicates.rs when vanilla ships it"
        ),
    }
}

/// A predicate key with its generated static identifier and condition tokens.
struct PredicateData {
    key: String,
    const_ident: Ident,
    condition: TokenStream,
}

/// Recursively collects every `.json` file under `dir`, keyed by its path
/// relative to `base_dir` without the extension.
fn read_predicates(dir: &Path, base_dir: &Path, predicates: &mut Vec<PredicateData>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();

    for path in paths {
        if path.is_dir() {
            read_predicates(&path, base_dir, predicates);
        } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let relative_path = path
                .strip_prefix(base_dir)
                .unwrap_or(&path)
                .with_extension("");
            let key = relative_path
                .to_str()
                .unwrap_or("unknown")
                .replace('\\', "/");

            let content = fs::read_to_string(&path).unwrap();
            let predicate: PredicateJson = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("Failed to parse predicate {key}: {e}"));

            let const_ident = Ident::new(
                &key.replace('/', "_").to_shouty_snake_case(),
                Span::call_site(),
            );

            predicates.push(PredicateData {
                key,
                const_ident,
                condition: generate_condition(&predicate),
            });
        }
    }
}

pub(crate) fn build() -> TokenStream {
    let predicate_dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/predicate";
    println!("cargo:rerun-if-changed={predicate_dir}");

    let mut predicates = Vec::new();
    read_predicates(
        Path::new(predicate_dir),
        Path::new(predicate_dir),
        &mut predicates,
    );

    let mut stream = TokenStream::new();

    stream.extend(quote! {
        use crate::loot_table::{LootCondition, PropertyCheck, ToolPredicate};
        use crate::predicate::{Predicate, PredicateRegistry};
        use steel_utils::Identifier;
    });

    let mut register_stream = TokenStream::new();
    for predicate in &predicates {
        let const_ident = &predicate.const_ident;
        let key = &predicate.key;
        let condition = &predicate.condition;

        stream.extend(quote! {
            pub static #const_ident: Predicate = Predicate {
                key: Identifier::vanilla_static(#key),
                condition: #condition,
            };
        });

        register_stream.extend(quote! {
            registry.register(&#const_ident);
        });
    }

    stream.extend(quote! {
        pub fn register_predicates(registry: &mut PredicateRegistry) {
            #register_stream
        }
    });

    stream
}
