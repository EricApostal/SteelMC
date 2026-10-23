#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted test environment data"
)]

use std::fs;

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;

/// The raw shape of a `test_environment` data pack entry: a vanilla type id plus
/// the fields of the matching `TestEnvironmentDefinition` record.
#[derive(Deserialize, Debug)]
struct TestEnvironmentJson {
    #[serde(rename = "type")]
    environment_type: String,
    /// `all_of` record field.
    #[serde(default)]
    definitions: Option<Vec<TestEnvironmentDefinitionJson>>,
    /// `clock_time` record fields.
    #[serde(default)]
    clock: Option<String>,
    #[serde(default)]
    time: Option<i32>,
    /// `difficulty` record field.
    #[serde(default)]
    difficulty: Option<String>,
    /// `function` record fields.
    #[serde(default)]
    setup: Option<String>,
    #[serde(default)]
    teardown: Option<String>,
    /// `game_rules` record field: vanilla's `GameRuleMap` serializes as a map
    /// from game rule id to the rule's value.
    #[serde(default)]
    rules: Option<serde_json::Map<String, serde_json::Value>>,
    /// `timeline_attributes` record field.
    #[serde(default)]
    timelines: Option<Vec<String>>,
    /// `weather` record field.
    #[serde(default)]
    weather: Option<String>,
}

/// The `definitions` list of an `all_of` entry: a `test_environment` key or an
/// inline definition (vanilla's `TestEnvironmentDefinition.CODEC`).
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum TestEnvironmentDefinitionJson {
    Reference(String),
    Inline(TestEnvironmentJson),
}

/// A `test_environment` key with its generated static identifier.
struct TestEnvironmentData {
    key: String,
    const_ident: Ident,
    definition: TokenStream,
}

fn strip_minecraft(value: &str) -> &str {
    value.strip_prefix("minecraft:").unwrap_or(value)
}

/// Quotes an identifier value, omitting the `minecraft:` namespace like every
/// other registry generator.
fn quote_identifier(value: &str) -> TokenStream {
    let path = strip_minecraft(value);
    quote! { Identifier::vanilla_static(#path) }
}

/// Quotes a recursive inline definition, emitting an error upstream if the
/// caller cannot embed the result in static data.
fn quote_inline_definition(definition: &TestEnvironmentJson) -> TokenStream {
    match generate_definition(definition) {
        Ok(tokens) => tokens,
        Err(message) => panic!("Unsupported inline test_environment definition: {message}"),
    }
}

fn generate_holder(definition: &TestEnvironmentDefinitionJson) -> TokenStream {
    match definition {
        TestEnvironmentDefinitionJson::Reference(name) => {
            let key = quote_identifier(name);
            quote! { TestEnvironmentHolder::Reference(#key) }
        }
        TestEnvironmentDefinitionJson::Inline(inline) => {
            let definition = quote_inline_definition(inline);
            quote! { TestEnvironmentHolder::Inline({ #definition }) }
        }
    }
}

fn generate_weather(weather: &Option<String>) -> Result<TokenStream, String> {
    match weather.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("clear") => Ok(quote! { WeatherType::Clear }),
        Some("rain") => Ok(quote! { WeatherType::Rain }),
        Some("thunder") => Ok(quote! { WeatherType::Thunder }),
        Some(other) => Err(format!("unknown weather type {other:?}")),
        None => Err("missing `weather` field".to_string()),
    }
}

fn generate_difficulty(difficulty: &Option<String>) -> Result<TokenStream, String> {
    match difficulty
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("peaceful") => Ok(quote! { Difficulty::Peaceful }),
        Some("easy") => Ok(quote! { Difficulty::Easy }),
        Some("normal") => Ok(quote! { Difficulty::Normal }),
        Some("hard") => Ok(quote! { Difficulty::Hard }),
        Some(other) => Err(format!("unknown difficulty {other:?}")),
        None => Err("missing `difficulty` field".to_string()),
    }
}

fn generate_rules(
    rules: &Option<serde_json::Map<String, serde_json::Value>>,
) -> Result<TokenStream, String> {
    let rules = rules
        .as_ref()
        .ok_or_else(|| "missing `rules` field".to_string())?;
    let mut settings = Vec::new();
    for (rule, value) in rules {
        let key = quote_identifier(rule);
        let setting = if let Some(flag) = value.as_bool() {
            quote! {
                GameRuleSetting {
                    rule: #key,
                    value: GameRuleSettingValue::Bool(#flag),
                }
            }
        } else if let Some(number) = value.as_i64() {
            let number = i32::try_from(number)
                .map_err(|_| format!("game rule {rule:?} value {number} does not fit in an i32"))?;
            quote! {
                GameRuleSetting {
                    rule: #key,
                    value: GameRuleSettingValue::Int(#number),
                }
            }
        } else {
            return Err(format!(
                "game rule {rule:?} has non-bool, non-int value {value}"
            ));
        };
        settings.push(setting);
    }
    Ok(quote! { &[#(#settings),*] })
}

fn generate_definition(definition: &TestEnvironmentJson) -> Result<TokenStream, String> {
    match definition.environment_type.as_str() {
        "minecraft:all_of" => {
            let definitions: Vec<TokenStream> = definition
                .definitions
                .as_ref()
                .ok_or_else(|| "missing `definitions` field".to_string())?
                .iter()
                .map(generate_holder)
                .collect();
            Ok(quote! {
                TestEnvironmentDefinition::AllOf(TestEnvironmentAllOf {
                    definitions: &[#(#definitions),*],
                })
            })
        }
        "minecraft:clock_time" => {
            let clock = definition
                .clock
                .as_deref()
                .ok_or_else(|| "missing `clock` field".to_string())?;
            let time = definition
                .time
                .ok_or_else(|| "missing `time` field".to_string())?;
            if time < 0 {
                return Err(format!("`time` must be non-negative, got {time}"));
            }
            let clock = quote_identifier(clock);
            Ok(quote! {
                TestEnvironmentDefinition::ClockTime(TestEnvironmentClockTime {
                    clock: #clock,
                    time: #time,
                })
            })
        }
        "minecraft:difficulty" => {
            let difficulty = generate_difficulty(&definition.difficulty)?;
            Ok(quote! {
                TestEnvironmentDefinition::Difficulty(TestEnvironmentDifficulty {
                    difficulty: #difficulty,
                })
            })
        }
        "minecraft:function" => {
            let setup = definition.setup.as_deref().map(quote_identifier);
            let setup =
                setup.map_or_else(|| quote! { None }, |function| quote! { Some(#function) });
            let teardown = definition.teardown.as_deref().map(quote_identifier);
            let teardown =
                teardown.map_or_else(|| quote! { None }, |function| quote! { Some(#function) });
            Ok(quote! {
                TestEnvironmentDefinition::Functions(TestEnvironmentFunctions {
                    setup_function: #setup,
                    teardown_function: #teardown,
                })
            })
        }
        "minecraft:game_rules" => {
            let rules = generate_rules(&definition.rules)?;
            Ok(quote! {
                TestEnvironmentDefinition::GameRules(TestEnvironmentGameRules {
                    rules: #rules,
                })
            })
        }
        "minecraft:timeline_attributes" => {
            let timelines: Vec<TokenStream> = definition
                .timelines
                .as_ref()
                .ok_or_else(|| "missing `timelines` field".to_string())?
                .iter()
                .map(|timeline| quote_identifier(timeline))
                .collect();
            Ok(quote! {
                TestEnvironmentDefinition::Timelines(TestEnvironmentTimelines {
                    timelines: &[#(#timelines),*],
                })
            })
        }
        "minecraft:weather" => {
            let weather = generate_weather(&definition.weather)?;
            Ok(quote! {
                TestEnvironmentDefinition::Weather(TestEnvironmentWeather {
                    weather: #weather,
                })
            })
        }
        other => Err(format!("unknown test_environment type {other:?}")),
    }
}

pub(crate) fn build() -> TokenStream {
    let test_environment_dir =
        "../steel-utils/build_assets/builtin_datapacks/minecraft/test_environment";
    println!("cargo:rerun-if-changed={test_environment_dir}");

    let mut environments = Vec::new();

    // Read all test_environment JSON files
    let mut paths: Vec<_> = fs::read_dir(test_environment_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();

    for path in paths {
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let key = path.file_stem().unwrap().to_str().unwrap().to_string();
            let content = fs::read_to_string(&path).unwrap();
            let environment: TestEnvironmentJson = serde_json::from_str(&content)
                .unwrap_or_else(|e| panic!("Failed to parse test_environment {key}: {e}"));

            let const_name = key.to_shouty_snake_case();
            let const_ident = Ident::new(&const_name, Span::call_site());
            let definition = match generate_definition(&environment) {
                Ok(definition) => definition,
                Err(message) => {
                    panic!("Failed to generate test_environment {key}: {message}");
                }
            };

            environments.push(TestEnvironmentData {
                key,
                const_ident,
                definition,
            });
        }
    }

    let mut stream = TokenStream::new();

    stream.extend(quote! {
        use crate::test_environment::{
            GameRuleSetting, GameRuleSettingValue, TestEnvironment, TestEnvironmentAllOf,
            TestEnvironmentClockTime, TestEnvironmentDefinition, TestEnvironmentDifficulty,
            TestEnvironmentFunctions, TestEnvironmentGameRules, TestEnvironmentHolder,
            TestEnvironmentRegistry, TestEnvironmentTimelines, TestEnvironmentWeather,
            WeatherType,
        };
        use steel_utils::Identifier;
        use steel_utils::types::Difficulty;
    });

    let mut register_stream = TokenStream::new();
    for environment in &environments {
        let const_ident = &environment.const_ident;
        let key = &environment.key;
        let definition = &environment.definition;

        stream.extend(quote! {
            pub static #const_ident: TestEnvironment = TestEnvironment {
                key: Identifier::vanilla_static(#key),
                definition: #definition,
            };
        });

        register_stream.extend(quote! {
            registry.register(&#const_ident);
        });
    }

    stream.extend(quote! {
        pub fn register_test_environments(registry: &mut TestEnvironmentRegistry) {
            #register_stream
        }
    });

    stream
}
