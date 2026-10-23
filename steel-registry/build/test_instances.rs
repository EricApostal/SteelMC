#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted test instance data"
)]

//! Build script for generating vanilla `test_instance` definitions.

use std::fs;

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;

/// The raw shape of a `test_instance` entry: a vanilla type id plus vanilla's
/// `TestData` fields (and, for `function`, the test function id).
#[derive(Deserialize, Debug)]
struct TestInstanceJson {
    #[serde(rename = "type")]
    instance_type: String,
    /// `TestData.environment`: a `test_environment` key.
    environment: String,
    /// `TestData.dimension`, defaulting to the overworld.
    #[serde(default)]
    dimension: Option<String>,
    /// `TestData.structure`.
    structure: String,
    /// `TestData.max_ticks`; vanilla requires a positive value.
    max_ticks: i32,
    /// `TestData.setup_ticks`, defaulting to 0.
    #[serde(default)]
    setup_ticks: Option<i32>,
    /// `TestData.required`, defaulting to true.
    #[serde(default)]
    required: Option<bool>,
    /// `TestData.rotation`, defaulting to `none`.
    #[serde(default)]
    rotation: Option<String>,
    /// `TestData.manual_only`, defaulting to false.
    #[serde(default)]
    manual_only: Option<bool>,
    /// `TestData.max_attempts`, defaulting to 1.
    #[serde(default)]
    max_attempts: Option<i32>,
    /// `TestData.required_successes`, defaulting to 1.
    #[serde(default)]
    required_successes: Option<i32>,
    /// `TestData.sky_access`, defaulting to false.
    #[serde(default)]
    sky_access: Option<bool>,
    /// `TestData.padding`, defaulting to 0; vanilla allows 0..=128.
    #[serde(default)]
    padding: Option<i32>,
    /// `FunctionGameTestInstance.function`: a `test_function` id.
    #[serde(default)]
    function: Option<String>,
}

/// A `test_instance` key with its generated static identifier and body tokens.
struct TestInstanceData {
    key: String,
    const_ident: Ident,
    instance: TokenStream,
}

fn strip_minecraft(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

/// Quotes a vanilla identifier, omitting the `minecraft:` namespace.
fn quote_identifier(value: &str) -> TokenStream {
    let path = strip_minecraft(value);
    quote! { Identifier::vanilla_static(#path) }
}

fn quote_rotation(rotation: Option<&str>) -> Result<TokenStream, String> {
    match rotation {
        None | Some("none") => Ok(quote! { Rotation::None }),
        Some("clockwise_90") => Ok(quote! { Rotation::Clockwise90 }),
        Some("180") => Ok(quote! { Rotation::Clockwise180 }),
        Some("counterclockwise_90") => Ok(quote! { Rotation::CounterClockwise90 }),
        Some(other) => Err(format!("unknown rotation {other:?}")),
    }
}

/// Quotes the shared `TestData` record for one entry.
fn quote_test_data(instance: &TestInstanceJson) -> Result<TokenStream, String> {
    if instance.max_ticks <= 0 {
        return Err(format!(
            "`max_ticks` must be positive, got {}",
            instance.max_ticks
        ));
    }
    let setup_ticks = instance.setup_ticks.unwrap_or(0);
    if setup_ticks < 0 {
        return Err(format!(
            "`setup_ticks` must be non-negative, got {setup_ticks}"
        ));
    }
    let max_attempts = instance.max_attempts.unwrap_or(1);
    if max_attempts <= 0 {
        return Err(format!(
            "`max_attempts` must be positive, got {max_attempts}"
        ));
    }
    let required_successes = instance.required_successes.unwrap_or(1);
    if required_successes <= 0 {
        return Err(format!(
            "`required_successes` must be positive, got {required_successes}"
        ));
    }
    let padding = instance.padding.unwrap_or(0);
    if !(0..=128).contains(&padding) {
        return Err(format!("`padding` must be within 0..=128, got {padding}"));
    }

    let environment = quote_identifier(&instance.environment);
    let dimension = quote_identifier(
        instance
            .dimension
            .as_deref()
            .unwrap_or("minecraft:overworld"),
    );
    let structure = quote_identifier(&instance.structure);
    let max_ticks = instance.max_ticks;
    let required = instance.required.unwrap_or(true);
    let rotation = quote_rotation(instance.rotation.as_deref())?;
    let manual_only = instance.manual_only.unwrap_or(false);
    let sky_access = instance.sky_access.unwrap_or(false);

    Ok(quote! {
        TestData {
            environment: #environment,
            dimension: #dimension,
            structure: #structure,
            max_ticks: #max_ticks,
            setup_ticks: #setup_ticks,
            required: #required,
            rotation: #rotation,
            manual_only: #manual_only,
            max_attempts: #max_attempts,
            required_successes: #required_successes,
            sky_access: #sky_access,
            padding: #padding,
        }
    })
}

fn generate_instance(instance: &TestInstanceJson) -> Result<TokenStream, String> {
    let info = quote_test_data(instance)?;
    match instance.instance_type.as_str() {
        "minecraft:function" => {
            let function = instance
                .function
                .as_deref()
                .ok_or_else(|| "missing `function` field".to_string())?;
            let function = quote_identifier(function);
            Ok(quote! {
                TestInstance::Function(GameTestFunctionInstance {
                    function: #function,
                    info: #info,
                })
            })
        }
        "minecraft:block_based" => Ok(quote! {
            TestInstance::BlockBased(GameTestBlockInstance { info: #info })
        }),
        other => Err(format!("unknown test_instance type {other:?}")),
    }
}

pub(crate) fn build() -> TokenStream {
    let test_instance_dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/test_instance";
    println!("cargo:rerun-if-changed={test_instance_dir}");

    let mut instances = Vec::new();

    // Read all test_instance JSON files
    let mut paths: Vec<_> = fs::read_dir(test_instance_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();

    for path in paths {
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let key = path.file_stem().unwrap().to_str().unwrap().to_string();
            let content = fs::read_to_string(&path).unwrap();
            let instance: TestInstanceJson = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("Failed to parse test_instance {key}: {e}"));

            let const_ident = Ident::new(&key.to_shouty_snake_case(), Span::call_site());
            let generated = match generate_instance(&instance) {
                Ok(generated) => generated,
                Err(message) => panic!("Failed to generate test_instance {key}: {message}"),
            };

            instances.push(TestInstanceData {
                key,
                const_ident,
                instance: generated,
            });
        }
    }

    let mut stream = TokenStream::new();

    stream.extend(quote! {
        use crate::test_instance::{
            GameTestBlockInstance, GameTestFunctionInstance, TestData, TestInstance,
            TestInstanceEntry, TestInstanceRegistry,
        };
        use steel_utils::{Identifier, Rotation};
    });

    let mut register_stream = TokenStream::new();
    for instance in &instances {
        let const_ident = &instance.const_ident;
        let key = &instance.key;
        let generated = &instance.instance;

        stream.extend(quote! {
            pub static #const_ident: TestInstanceEntry = TestInstanceEntry {
                key: Identifier::vanilla_static(#key),
                instance: #generated,
            };
        });

        register_stream.extend(quote! {
            registry.register(&#const_ident);
        });
    }

    stream.extend(quote! {
        pub fn register_test_instances(registry: &mut TestInstanceRegistry) {
            #register_stream
        }
    });

    stream
}
