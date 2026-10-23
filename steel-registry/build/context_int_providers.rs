#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted context int provider data"
)]

//! Build script for the vanilla `minecraft:context_int_provider` registry.
//!
//! Every JSON file under the datapack's `context_int_provider` directory becomes
//! one [`ContextIntProviderEntry`] static, with its provider tree compiled into
//! `&'static` data (vanilla
//! `net.minecraft.world.level.storage.loot.providers.number.ints`).
//!
//! Provider fields that hold another provider may be either an inline object or a
//! string naming another entry of this registry, mirroring vanilla's
//! `RegistryCodecs.holder`. References become `&<ENTRY>.provider`, so a tree is
//! never re-parsed at runtime and shared sub-providers stay shared.

use std::fs;
use std::path::{Path, PathBuf};

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use rustc_hash::FxHashMap;
use serde::Deserialize;

/// A provider value: a bare number, a reference to another registry entry, or an
/// object with a `type`.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum ProviderJson {
    /// A bare number (vanilla's inline `constant` codec).
    Number(f64),
    /// A registry reference, e.g. `"minecraft:cooking/time_coal"`.
    Reference(String),
    /// A typed provider object.
    Object(Box<ProviderObjectJson>),
}

/// The union of every provider type's fields; each type only reads its own.
#[derive(Deserialize, Debug)]
pub(crate) struct ProviderObjectJson {
    /// Provider type id, e.g. `minecraft:div`.
    #[serde(rename = "type")]
    pub(crate) provider_type: String,
    /// `minecraft:constant` value.
    #[serde(default)]
    pub(crate) value: Option<ProviderJson>,
    /// `minecraft:uniform` lower bound.
    #[serde(default)]
    pub(crate) min: Option<ProviderJson>,
    /// `minecraft:uniform` upper bound.
    #[serde(default)]
    pub(crate) max: Option<ProviderJson>,
    /// `minecraft:binomial` trial count (an int provider).
    #[serde(default)]
    pub(crate) n: Option<ProviderJson>,
    /// `minecraft:binomial` success chance (a float provider).
    #[serde(default)]
    pub(crate) p: Option<ProviderJson>,
    /// Aggregate inputs (`add`, `avg`, `min`, `max`, `mul`, `length`).
    #[serde(default)]
    pub(crate) inputs: Option<Vec<ProviderJson>>,
    /// Binary left operand (`sub`, `div`, `floor_div`, `mod`, `floor_mod`).
    #[serde(default)]
    pub(crate) left: Option<ProviderJson>,
    /// Binary right operand.
    #[serde(default)]
    pub(crate) right: Option<ProviderJson>,
    /// `minecraft:pow` base.
    #[serde(default)]
    pub(crate) base: Option<ProviderJson>,
    /// `minecraft:pow` exponent.
    #[serde(default)]
    pub(crate) exponent: Option<ProviderJson>,
    /// Unary input (`abs`, `negate`, `from_float`, `from_int`).
    #[serde(default)]
    pub(crate) input: Option<ProviderJson>,
    /// `minecraft:conditional` condition.
    #[serde(default)]
    pub(crate) condition: Option<ConditionRefJson>,
    /// `minecraft:conditional` value used when the condition passes.
    #[serde(default)]
    pub(crate) on_true: Option<ProviderJson>,
    /// `minecraft:conditional` value used when the condition fails.
    #[serde(default)]
    pub(crate) on_false: Option<ProviderJson>,
    /// `minecraft:number_dispatcher` cases, tested in order.
    #[serde(default)]
    pub(crate) cases: Option<Vec<DispatchCaseJson>>,
    /// `minecraft:number_dispatcher` default value.
    #[serde(default)]
    pub(crate) default: Option<ProviderJson>,
    /// `minecraft:weighted_list` distribution.
    #[serde(default)]
    pub(crate) distribution: Option<Vec<WeightedJson>>,
    /// `minecraft:score` score holder.
    #[serde(default)]
    pub(crate) target: Option<ScoreTargetJson>,
    /// `minecraft:score` objective name.
    #[serde(default)]
    pub(crate) score: Option<String>,
    /// `minecraft:storage` storage id.
    #[serde(default)]
    pub(crate) storage: Option<String>,
    /// `minecraft:storage` NBT path.
    #[serde(default)]
    pub(crate) path: Option<String>,
    /// `minecraft:score`/`minecraft:storage` fallback value.
    #[serde(default)]
    pub(crate) fallback: Option<ProviderJson>,
    /// `minecraft:environment_attribute` attribute id.
    #[serde(default)]
    pub(crate) attribute: Option<String>,
    /// `minecraft:enchantment_level` amount (vanilla `LevelBasedValue`).
    #[serde(default)]
    pub(crate) amount: Option<serde_json::Value>,
}

/// One `minecraft:number_dispatcher` case.
#[derive(Deserialize, Debug)]
pub(crate) struct DispatchCaseJson {
    /// Condition that selects this case.
    pub(crate) condition: ConditionRefJson,
    /// Value produced when the condition passes.
    pub(crate) value: ProviderJson,
}

/// One `minecraft:weighted_list` entry.
#[derive(Deserialize, Debug)]
pub(crate) struct WeightedJson {
    /// The provider (vanilla `data`).
    pub(crate) data: ProviderJson,
    /// Non-negative selection weight.
    #[serde(default = "default_weight")]
    pub(crate) weight: i32,
}

/// A `condition` field: an inline condition object, or a string naming a
/// `minecraft:predicate` entry.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum ConditionRefJson {
    /// Reference to `predicate/<path>.json`.
    Reference(String),
    /// Inline loot-condition object.
    Inline(Box<ConditionJson>),
}

/// The subset of loot-condition fields the context-provider data uses.
#[derive(Deserialize, Debug)]
pub(crate) struct ConditionJson {
    /// Condition type id, e.g. `minecraft:match_block`.
    #[serde(rename = "type")]
    pub(crate) condition: String,
    /// `minecraft:match_block` blocks (vanilla `HolderSet<Block>`).
    #[serde(default)]
    pub(crate) blocks: Option<BlocksJson>,
    /// `minecraft:match_block` state properties.
    #[serde(default, rename = "state")]
    pub(crate) properties: Option<FxHashMap<String, PropertyValueJson>>,
    /// `minecraft:random_chance` chance.
    #[serde(default)]
    pub(crate) chance: Option<f32>,
    /// `minecraft:inverted` operand.
    #[serde(default)]
    pub(crate) term: Option<Box<ConditionRefJson>>,
    /// `minecraft:any_of`/`minecraft:all_of` operands.
    #[serde(default)]
    pub(crate) terms: Option<Vec<ConditionRefJson>>,
}

/// A `match_block` block list: a single id or a list of ids.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum BlocksJson {
    /// One block id.
    Single(String),
    /// Several block ids.
    List(Vec<String>),
}

/// A block-state property value: an exact value or a range.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum PropertyValueJson {
    /// Exact property value.
    Exact(String),
    /// Property value range.
    Range {
        /// Inclusive lower bound.
        min: Option<String>,
        /// Inclusive upper bound.
        max: Option<String>,
    },
}

/// A `minecraft:score` score holder.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub(crate) enum ScoreTargetJson {
    /// Inline `minecraft:context` form: a loot-context entity target name.
    Context(String),
    /// Typed form (`minecraft:fixed` / `minecraft:context`).
    Typed {
        /// Provider type id.
        #[serde(rename = "type")]
        provider_type: String,
        /// `minecraft:fixed` score holder name.
        #[serde(default)]
        name: Option<String>,
        /// `minecraft:context` entity target name.
        #[serde(default)]
        target: Option<String>,
    },
}

fn default_weight() -> i32 {
    1
}

pub(crate) fn build() -> TokenStream {
    let provider_dir =
        "../steel-utils/build_assets/builtin_datapacks/minecraft/context_int_provider";
    println!("cargo:rerun-if-changed={provider_dir}");

    let mut files = Vec::new();
    collect_provider_files(Path::new(provider_dir), "", &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut stream = TokenStream::new();
    stream.extend(quote! {
        use crate::context_int_provider::{
            ContextIntProvider, ContextIntProviderEntry, ContextIntProviderRegistry, DispatcherCase,
            WeightedValue,
        };
        use crate::loot_table::{LootCondition, LootContextEntity, PropertyCheck, ScoreboardTarget};
        use steel_utils::Identifier;
    });

    let mut register_stream = TokenStream::new();
    for (key, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let content = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("Failed to read {}: {error}", path.display()));
        let provider: ProviderJson = serde_json::from_str(&content)
            .unwrap_or_else(|error| panic!("Failed to parse context int provider {key}: {error}"));

        let ident = Ident::new(&key.to_shouty_snake_case(), Span::call_site());
        let provider = generate_int_ref(&provider, Placement::Int);
        let key_literal = key.as_str();

        stream.extend(quote! {
            pub static #ident: ContextIntProviderEntry = ContextIntProviderEntry {
                key: Identifier::vanilla_static(#key_literal),
                provider: #provider,
            };
        });

        register_stream.extend(quote! {
            registry.register(&#ident);
        });
    }

    stream.extend(quote! {
        pub fn register_context_int_providers(registry: &mut ContextIntProviderRegistry) {
            #register_stream
        }
    });

    stream
}

/// Collects `(key, path)` pairs for every JSON file under `dir`, where the key is
/// the path below `dir` without its `.json` extension.
fn collect_provider_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("Failed to read {}: {error}", dir.display()))
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_str().unwrap();
            collect_provider_files(&path, &format!("{prefix}{name}/"), out);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            let name = path.file_stem().unwrap().to_str().unwrap();
            out.push((format!("{prefix}{name}"), path));
        }
    }
}

/// The generated module a provider expression is emitted into.
///
/// Int and float provider trees live in separate generated modules, so a
/// reference to the other domain's entry must be qualified with that module's
/// path.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Placement {
    /// `vanilla_context_int_providers`.
    Int,
    /// `vanilla_context_float_providers`.
    Float,
}

/// Emits `&'static ContextIntProvider` for one provider value, placed in
/// `placement`'s generated module.
///
/// Every nested operand is a reference, so a reference to another registry entry
/// shares that entry's tree and an inline provider is const-promoted.
pub(crate) fn generate_int_ref(value: &ProviderJson, placement: Placement) -> TokenStream {
    match value {
        ProviderJson::Number(number) => {
            let value = int_literal(*number);
            quote! { &ContextIntProvider::Constant(#value) }
        }
        ProviderJson::Reference(name) => {
            let ident = Ident::new(
                &reference_key(name).to_shouty_snake_case(),
                Span::call_site(),
            );
            if placement == Placement::Int {
                quote! { &#ident.provider }
            } else {
                quote! { &crate::vanilla_context_int_providers::#ident.provider }
            }
        }
        ProviderJson::Object(object) => {
            let object = generate_int_object(object, placement);
            quote! { &#object }
        }
    }
}

/// Emits the `ContextIntProvider` expression (an owned value) for an inline
/// provider object.
fn generate_int_value(value: &ProviderJson, placement: Placement) -> TokenStream {
    match value {
        ProviderJson::Number(number) => {
            let value = int_literal(*number);
            quote! { ContextIntProvider::Constant(#value) }
        }
        ProviderJson::Object(object) => generate_int_object(object, placement),
        ProviderJson::Reference(name) => {
            let ident = Ident::new(
                &reference_key(name).to_shouty_snake_case(),
                Span::call_site(),
            );
            if placement == Placement::Int {
                quote! { *#ident.provider }
            } else {
                quote! { *crate::vanilla_context_int_providers::#ident.provider }
            }
        }
    }
}

/// Emits the `ContextIntProvider` expression for a typed provider object.
fn generate_int_object(object: &ProviderObjectJson, placement: Placement) -> TokenStream {
    let provider_type = object
        .provider_type
        .strip_prefix("minecraft:")
        .unwrap_or(&object.provider_type);

    match provider_type {
        "constant" => {
            let value = int_literal(constant_number(object, provider_type));
            quote! { ContextIntProvider::Constant(#value) }
        }
        "uniform" => {
            let min = generate_int_ref(field(object.min.as_ref(), provider_type, "min"), placement);
            let max = generate_int_ref(field(object.max.as_ref(), provider_type, "max"), placement);
            quote! { ContextIntProvider::Uniform { min: #min, max: #max } }
        }
        "binomial" => {
            let n = field(object.n.as_ref(), provider_type, "n");
            let p = field(object.p.as_ref(), provider_type, "p");
            match (n, p) {
                (ProviderJson::Number(n), ProviderJson::Number(p)) => {
                    let n = int_literal(*n);
                    let p = float_literal(*p);
                    quote! { ContextIntProvider::Binomial { n: #n, p: #p } }
                }
                (n, p) => {
                    let n = generate_int_ref(n, placement);
                    let p = super::context_float_providers::generate_float_ref(p, placement);
                    quote! { ContextIntProvider::BinomialProviders { n: #n, p: #p } }
                }
            }
        }
        "add" => aggregate("Sum", object, provider_type, placement),
        "avg" => aggregate("Average", object, provider_type, placement),
        "min" => aggregate("Minimum", object, provider_type, placement),
        "max" => aggregate("Maximum", object, provider_type, placement),
        "mul" => aggregate("Product", object, provider_type, placement),
        "sub" => binary("Difference", object, provider_type, placement),
        "div" => binary("Quotient", object, provider_type, placement),
        "floor_div" => binary("FloorQuotient", object, provider_type, placement),
        "mod" => binary("Modulus", object, provider_type, placement),
        "floor_mod" => binary("FloorModulus", object, provider_type, placement),
        "pow" => {
            let base = generate_int_ref(
                field(object.base.as_ref(), provider_type, "base"),
                placement,
            );
            let exponent = generate_int_ref(
                field(object.exponent.as_ref(), provider_type, "exponent"),
                placement,
            );
            quote! { ContextIntProvider::Power { base: #base, exponent: #exponent } }
        }
        "negate" => unary("Negate", object, provider_type, placement),
        "abs" => unary("Absolute", object, provider_type, placement),
        "from_float" => {
            let input = super::context_float_providers::generate_float_ref(
                field(object.input.as_ref(), provider_type, "input"),
                placement,
            );
            quote! { ContextIntProvider::FromFloat { input: #input } }
        }
        "conditional" => {
            let condition = generate_condition_ref(field(
                object.condition.as_ref(),
                provider_type,
                "condition",
            ));
            let on_true = generate_int_ref(
                field(object.on_true.as_ref(), provider_type, "on_true"),
                placement,
            );
            // Vanilla defaults `on_false` to `exactly(0)`.
            let on_false = generate_int_ref(fallback_or_zero(object.on_false.as_ref()), placement);
            quote! {
                ContextIntProvider::Conditional {
                    condition: &(#condition),
                    on_true: #on_true,
                    on_false: #on_false,
                }
            }
        }
        "number_dispatcher" => {
            let cases = object
                .cases
                .as_ref()
                .unwrap_or_else(|| panic!("minecraft:number_dispatcher is missing cases"));
            let cases: Vec<TokenStream> = cases
                .iter()
                .map(|case| {
                    let condition = generate_condition_ref(&case.condition);
                    let value = generate_int_ref(&case.value, placement);
                    quote! { DispatcherCase { condition: &(#condition), value: #value } }
                })
                .collect();
            // Vanilla defaults `default` to `exactly(0)`.
            let default = generate_int_ref(fallback_or_zero(object.default.as_ref()), placement);
            quote! {
                ContextIntProvider::NumberDispatcher {
                    cases: &[#(#cases),*],
                    default: #default,
                }
            }
        }
        "weighted_list" => {
            let distribution = object
                .distribution
                .as_ref()
                .unwrap_or_else(|| panic!("minecraft:weighted_list is missing distribution"));
            assert!(
                !distribution.is_empty(),
                "minecraft:weighted_list requires at least one entry"
            );
            let entries: Vec<TokenStream> = distribution
                .iter()
                .map(|entry| {
                    let data = generate_int_ref(&entry.data, placement);
                    let weight = entry.weight;
                    quote! { WeightedValue { data: #data, weight: #weight } }
                })
                .collect();
            quote! { ContextIntProvider::WeightedList(&[#(#entries),*]) }
        }
        "score" => {
            let target =
                generate_score_target(field(object.target.as_ref(), provider_type, "target"));
            let score = object
                .score
                .as_deref()
                .unwrap_or_else(|| panic!("minecraft:score is missing score"));
            let fallback = generate_int_ref(fallback_or_zero(object.fallback.as_ref()), placement);
            quote! {
                ContextIntProvider::Scoreboard {
                    target: #target,
                    score: #score,
                    fallback: #fallback,
                }
            }
        }
        "storage" => {
            let storage = object
                .storage
                .as_deref()
                .unwrap_or_else(|| panic!("minecraft:storage is missing storage"));
            let storage = storage.strip_prefix("minecraft:").unwrap_or(storage);
            let path = object
                .path
                .as_deref()
                .unwrap_or_else(|| panic!("minecraft:storage is missing path"));
            let fallback = generate_int_ref(fallback_or_zero(object.fallback.as_ref()), placement);
            quote! {
                ContextIntProvider::Storage {
                    storage: Identifier::vanilla_static(#storage),
                    path: #path,
                    fallback: #fallback,
                }
            }
        }
        "environment_attribute" => {
            let attribute = object
                .attribute
                .as_deref()
                .unwrap_or_else(|| panic!("minecraft:environment_attribute is missing attribute"));
            let attribute = attribute.strip_prefix("minecraft:").unwrap_or(attribute);
            quote! {
                ContextIntProvider::EnvironmentAttribute {
                    attribute: Identifier::vanilla_static(#attribute),
                }
            }
        }
        other => panic!("Unsupported context int provider type: minecraft:{other}"),
    }
}

/// Emits an aggregate provider (`add`/`avg`/`min`/`max`/`mul`).
fn aggregate(
    variant: &str,
    object: &ProviderObjectJson,
    provider_type: &str,
    placement: Placement,
) -> TokenStream {
    let variant = Ident::new(variant, Span::call_site());
    let inputs = object
        .inputs
        .as_ref()
        .unwrap_or_else(|| panic!("minecraft:{provider_type} is missing inputs"));
    assert!(
        !inputs.is_empty(),
        "minecraft:{provider_type} requires at least one input"
    );
    let inputs: Vec<TokenStream> = inputs
        .iter()
        .map(|input| generate_int_ref(input, placement))
        .collect();
    quote! { ContextIntProvider::#variant { inputs: &[#(#inputs),*] } }
}

/// Emits a binary provider (`sub`/`div`/`floor_div`/`mod`/`floor_mod`).
fn binary(
    variant: &str,
    object: &ProviderObjectJson,
    provider_type: &str,
    placement: Placement,
) -> TokenStream {
    let variant = Ident::new(variant, Span::call_site());
    let left = generate_int_ref(
        field(object.left.as_ref(), provider_type, "left"),
        placement,
    );
    let right = generate_int_ref(
        field(object.right.as_ref(), provider_type, "right"),
        placement,
    );
    quote! { ContextIntProvider::#variant { left: #left, right: #right } }
}

/// Emits a unary provider (`negate`/`abs`).
fn unary(
    variant: &str,
    object: &ProviderObjectJson,
    provider_type: &str,
    placement: Placement,
) -> TokenStream {
    let variant = Ident::new(variant, Span::call_site());
    let input = generate_int_ref(
        field(object.input.as_ref(), provider_type, "input"),
        placement,
    );
    quote! { ContextIntProvider::#variant { input: #input } }
}

/// A required field, or a panic naming the provider type and field.
pub(crate) fn field<'a, T>(value: Option<&'a T>, provider_type: &str, name: &str) -> &'a T {
    value.unwrap_or_else(|| panic!("minecraft:{provider_type} is missing {name}"))
}

/// The `fallback` field, defaulting to vanilla's `exactly(0)`.
fn fallback_or_zero(fallback: Option<&ProviderJson>) -> &ProviderJson {
    static ZERO: ProviderJson = ProviderJson::Number(0.0);
    fallback.unwrap_or(&ZERO)
}

/// The `value` field of a `constant` provider, or a panic when absent.
fn constant_number<'a>(object: &'a ProviderObjectJson, provider_type: &str) -> f64 {
    match field(object.value.as_ref(), provider_type, "value") {
        ProviderJson::Number(number) => *number,
        other => panic!("minecraft:{provider_type} value must be a number, got {other:?}"),
    }
}

/// An integer provider value: whole numbers only, matching `Codec.INT`.
fn int_literal(number: f64) -> i32 {
    assert!(
        number.fract() == 0.0 && number >= f64::from(i32::MIN) && number <= f64::from(i32::MAX),
        "context int provider value {number} is not an i32"
    );
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the value was just checked to be a whole i32"
    )]
    let value = number as i32;
    value
}

/// A float provider literal.
pub(crate) fn float_literal(number: f64) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "vanilla's Codec.FLOAT reads JSON numbers as f32"
    )]
    let value = number as f32;
    value
}

/// Strips the `minecraft:` namespace from a registry reference.
pub(crate) fn reference_key(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

/// Emits a `LootCondition` expression for an inline condition or a predicate
/// reference.
pub(crate) fn generate_condition_ref(condition: &ConditionRefJson) -> TokenStream {
    match condition {
        ConditionRefJson::Reference(name) => generate_condition(&load_predicate(name)),
        ConditionRefJson::Inline(condition) => generate_condition(condition),
    }
}

/// Loads a `minecraft:predicate` entry, exactly as vanilla's
/// `RegistryCodecs.holder` reference form resolves it.
fn load_predicate(name: &str) -> ConditionJson {
    let path_name = reference_key(name);
    let path = format!(
        "../steel-utils/build_assets/builtin_datapacks/minecraft/predicate/{path_name}.json"
    );
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("Failed to read predicate {name} at {path}: {error}"));
    serde_json::from_str(&content)
        .unwrap_or_else(|error| panic!("Failed to parse predicate {name}: {error}"))
}

/// Emits the `LootCondition` expression for one inline condition object.
fn generate_condition(condition: &ConditionJson) -> TokenStream {
    match condition.condition.as_str() {
        "minecraft:survives_explosion" => quote! { LootCondition::SurvivesExplosion },
        "minecraft:killed_by_player" => quote! { LootCondition::KilledByPlayer },
        "minecraft:random_chance" => {
            let chance = condition.chance.unwrap_or(0.5);
            quote! { LootCondition::RandomChance(#chance) }
        }
        "minecraft:inverted" => {
            let term = condition
                .term
                .as_deref()
                .map(generate_condition_ref)
                .unwrap_or_else(|| panic!("minecraft:inverted is missing term"));
            quote! { LootCondition::Inverted(&#term) }
        }
        "minecraft:any_of" | "minecraft:all_of" => {
            let variant = if condition.condition == "minecraft:any_of" {
                quote! { AnyOf }
            } else {
                quote! { AllOf }
            };
            let terms = condition
                .terms
                .as_ref()
                .unwrap_or_else(|| panic!("{} is missing terms", condition.condition));
            let terms: Vec<TokenStream> = terms.iter().map(generate_condition_ref).collect();
            quote! { LootCondition::#variant(&[#(#terms),*]) }
        }
        "minecraft:match_block" => {
            let blocks: Vec<&str> = match condition.blocks.as_ref() {
                Some(BlocksJson::Single(block)) => vec![block.as_str()],
                Some(BlocksJson::List(blocks)) => blocks.iter().map(String::as_str).collect(),
                None => panic!("minecraft:match_block is missing blocks"),
            };
            let properties: Vec<TokenStream> =
                condition.properties.as_ref().map_or_default(|properties| {
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

            let matches: Vec<TokenStream> = blocks
                .iter()
                .map(|block| {
                    let block = reference_key(block);
                    let properties = properties.clone();
                    quote! {
                        LootCondition::MatchBlock {
                            block: Identifier::vanilla_static(#block),
                            properties: &[#(#properties),*],
                        }
                    }
                })
                .collect();

            if let [single] = matches.as_slice() {
                single.clone()
            } else {
                quote! { LootCondition::AnyOf(&[#(#matches),*]) }
            }
        }
        other => panic!("Unsupported loot condition type in a context provider: {other}"),
    }
}

/// Emits the `ScoreboardTarget` for a `minecraft:score` provider.
fn generate_score_target(target: &ScoreTargetJson) -> TokenStream {
    match target {
        ScoreTargetJson::Context(entity) => {
            let entity = generate_entity_target(entity);
            quote! { ScoreboardTarget::Context(#entity) }
        }
        ScoreTargetJson::Typed {
            provider_type,
            name,
            target,
        } => match provider_type
            .strip_prefix("minecraft:")
            .unwrap_or(provider_type)
        {
            "fixed" => {
                let name = name
                    .as_deref()
                    .unwrap_or_else(|| panic!("minecraft:fixed is missing name"));
                quote! { ScoreboardTarget::Fixed(#name) }
            }
            "context" => {
                let entity = generate_entity_target(
                    target
                        .as_deref()
                        .unwrap_or_else(|| panic!("minecraft:context is missing target")),
                );
                quote! { ScoreboardTarget::Context(#entity) }
            }
            other => panic!("Unsupported loot score provider type: minecraft:{other}"),
        },
    }
}

/// Maps a vanilla `LootContext.EntityTarget` name to `LootContextEntity`.
fn generate_entity_target(entity: &str) -> TokenStream {
    match entity {
        "this" => quote! { LootContextEntity::This },
        "attacker" => quote! { LootContextEntity::Killer },
        "direct_attacker" => quote! { LootContextEntity::DirectKiller },
        "attacking_player" => quote! { LootContextEntity::KillerPlayer },
        "interacting_entity" => quote! { LootContextEntity::Interacting },
        other => panic!("Unsupported loot context entity target: {other}"),
    }
}
