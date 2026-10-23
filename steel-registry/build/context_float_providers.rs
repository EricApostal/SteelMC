#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted context float provider data"
)]

//! Build script for the vanilla `minecraft:context_float_provider` registry.
//!
//! Every JSON file under the datapack's `context_float_provider` directory
//! becomes one [`ContextFloatProviderEntry`] static, with its provider tree
//! compiled into `&'static` data (vanilla
//! `net.minecraft.world.level.storage.loot.providers.number.floats`).
//!
//! The JSON and generation helpers mirror `super::context_int_providers`: int and
//! float domains share field shapes, and a float provider may nest an int
//! provider through `from_int` (and vice versa through `from_float`), so one
//! generator delegates to the other for the cross-domain operand.

use std::fs;
use std::path::{Path, PathBuf};

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;

use super::context_int_providers::{
    Placement, ProviderJson, ProviderObjectJson, field, float_literal, generate_condition_ref,
    generate_int_ref, reference_key,
};

pub(crate) fn build() -> TokenStream {
    let provider_dir =
        "../steel-utils/build_assets/builtin_datapacks/minecraft/context_float_provider";
    println!("cargo:rerun-if-changed={provider_dir}");

    let mut files = Vec::new();
    collect_provider_files(Path::new(provider_dir), "", &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut stream = TokenStream::new();
    stream.extend(quote! {
        use crate::context_float_provider::{
            ContextFloatProvider, ContextFloatProviderEntry, ContextFloatProviderRegistry,
        };
        use crate::context_int_provider::{DispatcherCase, WeightedValue};
        use crate::enchantment_effect::LevelBasedValue;
        use crate::loot_table::{LootCondition, PropertyCheck};
        use steel_utils::Identifier;
    });

    let mut register_stream = TokenStream::new();
    for (key, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let content = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("Failed to read {}: {error}", path.display()));
        let provider: ProviderJson = serde_json::from_str(&content).unwrap_or_else(|error| {
            panic!("Failed to parse context float provider {key}: {error}")
        });

        let ident = Ident::new(&key.to_shouty_snake_case(), Span::call_site());
        let provider = generate_float_ref(&provider, Placement::Float);
        let key_literal = key.as_str();

        stream.extend(quote! {
            pub static #ident: ContextFloatProviderEntry = ContextFloatProviderEntry {
                key: Identifier::vanilla_static(#key_literal),
                provider: #provider,
            };
        });

        register_stream.extend(quote! {
            registry.register(&#ident);
        });
    }

    stream.extend(quote! {
        pub fn register_context_float_providers(registry: &mut ContextFloatProviderRegistry) {
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

/// Emits `&'static ContextFloatProvider` for one provider value, placed in
/// `placement`'s generated module.
///
/// Every nested operand is a reference, so a reference to another registry entry
/// shares that entry's tree and an inline provider is const-promoted.
pub(crate) fn generate_float_ref(value: &ProviderJson, placement: Placement) -> TokenStream {
    match value {
        ProviderJson::Number(number) => {
            let value = float_literal(*number);
            quote! { &ContextFloatProvider::Constant(#value) }
        }
        ProviderJson::Reference(name) => {
            let ident = Ident::new(
                &reference_key(name).to_shouty_snake_case(),
                Span::call_site(),
            );
            if placement == Placement::Float {
                quote! { &#ident.provider }
            } else {
                quote! { &crate::vanilla_context_float_providers::#ident.provider }
            }
        }
        ProviderJson::Object(object) => {
            let object = generate_float_object(object, placement);
            quote! { &#object }
        }
    }
}

/// Emits the `ContextFloatProvider` expression for a typed provider object.
fn generate_float_object(object: &ProviderObjectJson, placement: Placement) -> TokenStream {
    let provider_type = object
        .provider_type
        .strip_prefix("minecraft:")
        .unwrap_or(&object.provider_type);

    match provider_type {
        "constant" => {
            let value = float_literal(constant_number(object, provider_type));
            quote! { ContextFloatProvider::Constant(#value) }
        }
        "uniform" => {
            let min =
                generate_float_ref(field(object.min.as_ref(), provider_type, "min"), placement);
            let max =
                generate_float_ref(field(object.max.as_ref(), provider_type, "max"), placement);
            quote! { ContextFloatProvider::Uniform { min: #min, max: #max } }
        }
        "add" => aggregate("Sum", object, provider_type, placement),
        "avg" => aggregate("Average", object, provider_type, placement),
        "min" => aggregate("Minimum", object, provider_type, placement),
        "max" => aggregate("Maximum", object, provider_type, placement),
        "mul" => aggregate("Product", object, provider_type, placement),
        "length" => aggregate("Length", object, provider_type, placement),
        "sub" => binary("Difference", object, provider_type, placement),
        "div" => binary("Quotient", object, provider_type, placement),
        "mod" => binary("Modulus", object, provider_type, placement),
        "pow" => {
            let base = generate_float_ref(
                field(object.base.as_ref(), provider_type, "base"),
                placement,
            );
            let exponent = generate_float_ref(
                field(object.exponent.as_ref(), provider_type, "exponent"),
                placement,
            );
            quote! { ContextFloatProvider::Power { base: #base, exponent: #exponent } }
        }
        "negate" => unary("Negate", object, provider_type, placement),
        "abs" => unary("Absolute", object, provider_type, placement),
        "ceil" => unary("Ceiling", object, provider_type, placement),
        "floor" => unary("Floor", object, provider_type, placement),
        "round" => unary("Round", object, provider_type, placement),
        "truncate" => unary("Truncate", object, provider_type, placement),
        "sqrt" => unary("SquareRoot", object, provider_type, placement),
        "sin" => unary("Sine", object, provider_type, placement),
        "cos" => unary("Cosine", object, provider_type, placement),
        "from_int" => {
            let input = generate_int_ref(
                field(object.input.as_ref(), provider_type, "input"),
                placement,
            );
            quote! { ContextFloatProvider::FromInt { input: #input } }
        }
        "conditional" => {
            let condition = generate_condition_ref(field(
                object.condition.as_ref(),
                provider_type,
                "condition",
            ));
            let on_true = generate_float_ref(
                field(object.on_true.as_ref(), provider_type, "on_true"),
                placement,
            );
            // Vanilla defaults `on_false` to `exactly(0.0)`.
            let on_false = fallback_or_zero(object.on_false.as_ref(), placement);
            quote! {
                ContextFloatProvider::Conditional {
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
                    let value = generate_float_ref(&case.value, placement);
                    quote! { DispatcherCase { condition: &(#condition), value: #value } }
                })
                .collect();
            // Vanilla defaults `default` to `exactly(0.0)`.
            let default = fallback_or_zero(object.default.as_ref(), placement);
            quote! {
                ContextFloatProvider::NumberDispatcher {
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
                    let data = generate_float_ref(&entry.data, placement);
                    let weight = entry.weight;
                    quote! { WeightedValue { data: #data, weight: #weight } }
                })
                .collect();
            quote! { ContextFloatProvider::WeightedList(&[#(#entries),*]) }
        }
        "storage" => {
            let storage = object
                .storage
                .as_deref()
                .unwrap_or_else(|| panic!("minecraft:storage is missing storage"));
            let storage = reference_key(storage);
            let path = object
                .path
                .as_deref()
                .unwrap_or_else(|| panic!("minecraft:storage is missing path"));
            let fallback = fallback_or_zero(object.fallback.as_ref(), placement);
            quote! {
                ContextFloatProvider::Storage {
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
            let attribute = reference_key(attribute);
            quote! {
                ContextFloatProvider::EnvironmentAttribute {
                    attribute: Identifier::vanilla_static(#attribute),
                }
            }
        }
        "enchantment_level" => {
            let amount = object
                .amount
                .as_ref()
                .unwrap_or_else(|| panic!("minecraft:enchantment_level is missing amount"));
            let amount = generate_level_based_value(amount);
            quote! { ContextFloatProvider::EnchantmentLevel { amount: &(#amount) } }
        }
        other => panic!("Unsupported context float provider type: minecraft:{other}"),
    }
}

/// A `minecraft:enchantment_level` amount (vanilla `LevelBasedValue`).
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum LevelBasedValueJson {
    /// A bare number.
    Constant(f32),
    /// A typed formula object.
    Typed(LevelBasedValueTypedJson),
}

/// A typed `LevelBasedValue` object (vanilla's per-type codecs).
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum LevelBasedValueTypedJson {
    /// A constant value (`minecraft:constant`).
    #[serde(rename = "minecraft:constant")]
    Constant {
        /// The value.
        value: Box<LevelBasedValueJson>,
    },
    /// A value clamped to `[min, max]` (`minecraft:clamped`).
    #[serde(rename = "minecraft:clamped")]
    Clamped {
        /// Inner value.
        value: Box<LevelBasedValueJson>,
        /// Lower bound.
        min: f32,
        /// Upper bound.
        max: f32,
    },
    /// An exponent (`minecraft:exponent`).
    #[serde(rename = "minecraft:exponent")]
    Exponent {
        /// Base.
        base: Box<LevelBasedValueJson>,
        /// Power.
        power: Box<LevelBasedValueJson>,
    },
    /// A fraction (`minecraft:fraction`).
    #[serde(rename = "minecraft:fraction")]
    Fraction {
        /// Numerator.
        numerator: Box<LevelBasedValueJson>,
        /// Denominator.
        denominator: Box<LevelBasedValueJson>,
    },
    /// A squared level (`minecraft:levels_squared`).
    #[serde(rename = "minecraft:levels_squared")]
    LevelsSquared {
        /// Value added to the squared level.
        added: f32,
    },
    /// A linear value (`minecraft:linear`).
    #[serde(rename = "minecraft:linear")]
    Linear {
        /// Base value at level 1.
        base: f32,
        /// Amount added per level above the first.
        per_level_above_first: f32,
    },
    /// A lookup table (`minecraft:lookup`).
    #[serde(rename = "minecraft:lookup")]
    Lookup {
        /// Values indexed by `level - 1`.
        values: Vec<f32>,
        /// Value used past the end of the table.
        fallback: Box<LevelBasedValueJson>,
    },
}

/// Emits the `LevelBasedValue` expression for an `enchantment_level` amount.
fn generate_level_based_value(value: &serde_json::Value) -> TokenStream {
    let amount: LevelBasedValueJson = serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("Invalid LevelBasedValue amount {value}: {error}"));
    generate_level_based_value_typed(&amount)
}

/// Emits the `LevelBasedValue` expression for a parsed amount.
fn generate_level_based_value_typed(value: &LevelBasedValueJson) -> TokenStream {
    match value {
        LevelBasedValueJson::Constant(value) => {
            quote! { LevelBasedValue::Constant(#value) }
        }
        LevelBasedValueJson::Typed(value) => match value {
            LevelBasedValueTypedJson::Constant { value } => {
                let value = generate_level_based_value_typed(value);
                quote! { LevelBasedValue::Constant(#value) }
            }
            LevelBasedValueTypedJson::Clamped { value, min, max } => {
                let value = generate_level_based_value_typed(value);
                quote! {
                    LevelBasedValue::Clamped {
                        value: &#value,
                        min: #min,
                        max: #max,
                    }
                }
            }
            LevelBasedValueTypedJson::Exponent { base, power } => {
                let base = generate_level_based_value_typed(base);
                let power = generate_level_based_value_typed(power);
                quote! {
                    LevelBasedValue::Exponent {
                        base: &#base,
                        power: &#power,
                    }
                }
            }
            LevelBasedValueTypedJson::Fraction {
                numerator,
                denominator,
            } => {
                let numerator = generate_level_based_value_typed(numerator);
                let denominator = generate_level_based_value_typed(denominator);
                quote! {
                    LevelBasedValue::Fraction {
                        numerator: &#numerator,
                        denominator: &#denominator,
                    }
                }
            }
            LevelBasedValueTypedJson::LevelsSquared { added } => {
                quote! { LevelBasedValue::LevelsSquared { added: #added } }
            }
            LevelBasedValueTypedJson::Linear {
                base,
                per_level_above_first,
            } => {
                quote! {
                    LevelBasedValue::Linear {
                        base: #base,
                        per_level_above_first: #per_level_above_first,
                    }
                }
            }
            LevelBasedValueTypedJson::Lookup { values, fallback } => {
                let fallback = generate_level_based_value_typed(fallback);
                quote! {
                    LevelBasedValue::Lookup {
                        values: &[#(#values),*],
                        fallback: &#fallback,
                    }
                }
            }
        },
    }
}

/// Emits an aggregate provider (`add`/`avg`/`min`/`max`/`mul`/`length`).
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
        .map(|input| generate_float_ref(input, placement))
        .collect();
    quote! { ContextFloatProvider::#variant { inputs: &[#(#inputs),*] } }
}

/// Emits a binary provider (`sub`/`div`/`mod`).
fn binary(
    variant: &str,
    object: &ProviderObjectJson,
    provider_type: &str,
    placement: Placement,
) -> TokenStream {
    let variant = Ident::new(variant, Span::call_site());
    let left = generate_float_ref(
        field(object.left.as_ref(), provider_type, "left"),
        placement,
    );
    let right = generate_float_ref(
        field(object.right.as_ref(), provider_type, "right"),
        placement,
    );
    quote! { ContextFloatProvider::#variant { left: #left, right: #right } }
}

/// Emits a unary provider (`negate`/`abs`/`ceil`/`floor`/`round`/`truncate`/
/// `sqrt`/`sin`/`cos`).
fn unary(
    variant: &str,
    object: &ProviderObjectJson,
    provider_type: &str,
    placement: Placement,
) -> TokenStream {
    let variant = Ident::new(variant, Span::call_site());
    let input = generate_float_ref(
        field(object.input.as_ref(), provider_type, "input"),
        placement,
    );
    quote! { ContextFloatProvider::#variant { input: #input } }
}

/// The `fallback` field, defaulting to vanilla's `exactly(0.0)`.
fn fallback_or_zero(value: Option<&ProviderJson>, placement: Placement) -> TokenStream {
    static ZERO: ProviderJson = ProviderJson::Number(0.0);
    generate_float_ref(value.unwrap_or(&ZERO), placement)
}

/// The `value` field of a `constant` provider, or a panic when absent.
fn constant_number(object: &ProviderObjectJson, provider_type: &str) -> f64 {
    match field(object.value.as_ref(), provider_type, "value") {
        ProviderJson::Number(number) => *number,
        other => panic!("minecraft:{provider_type} value must be a number, got {other:?}"),
    }
}
