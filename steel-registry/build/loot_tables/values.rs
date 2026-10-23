use super::{
    BlockPredicateJson, DamageSourcePredicateJson, DoubleBoundsJson, EnchantmentOptionsJson,
    EntityEquipmentJson, EntityFlagsJson, EntityPredicateJson, EquipmentSlotJson, FromStr,
    HolderSetJson, Ident, Identifier, LevelRangeJson, LocationPredicateJson, NumberProviderJson,
    NumberProviderObjectJson, PositionPredicateJson, PredicateJson, Span, ToShoutySnakeCase,
    TokenStream, generate_condition, quote,
};

pub(crate) fn generate_number_provider(value: &NumberProviderJson) -> TokenStream {
    match value {
        NumberProviderJson::Constant(value) => {
            quote! { NumberProvider::Constant(#value) }
        }
        NumberProviderJson::Reference(key) => {
            let key = generate_identifier_token(key);
            quote! { NumberProvider::Reference { key: #key } }
        }
        NumberProviderJson::Object(object) => match object.provider_type.as_str() {
            "minecraft:constant" => {
                let value = object.value.as_ref().and_then(constant_of).unwrap_or(0.0);
                quote! { NumberProvider::Constant(#value) }
            }
            "minecraft:uniform" => {
                let min = object.min.as_ref().and_then(constant_of);
                let max = object.max.as_ref().and_then(constant_of);
                if let (Some(min), Some(max)) = (min, max) {
                    quote! { NumberProvider::Uniform { min: #min, max: #max } }
                } else {
                    let min =
                        provider_or_default(&object.min, quote! { NumberProvider::Constant(0.0) });
                    let max =
                        provider_or_default(&object.max, quote! { NumberProvider::Constant(1.0) });
                    quote! {
                        NumberProvider::UniformProviders {
                            min: &#min,
                            max: &#max,
                        }
                    }
                }
            }
            "minecraft:binomial" => {
                let trials = object.n.as_ref().and_then(constant_of);
                let chance = object.p.as_ref().and_then(constant_of);
                if let (Some(trials), Some(chance)) = (trials, chance) {
                    let n = trials as i32;
                    quote! { NumberProvider::Binomial { n: #n, p: #chance } }
                } else {
                    let n =
                        provider_or_default(&object.n, quote! { NumberProvider::Constant(1.0) });
                    let p =
                        provider_or_default(&object.p, quote! { NumberProvider::Constant(0.5) });
                    quote! { NumberProvider::BinomialProviders { n: &#n, p: &#p } }
                }
            }
            "minecraft:add" | "add" => {
                let inputs = aggregate_inputs(object, "add");
                quote! { NumberProvider::Add { inputs: &[#(#inputs),*] } }
            }
            "minecraft:avg" | "avg" => {
                let inputs = aggregate_inputs(object, "avg");
                quote! { NumberProvider::Average { inputs: &[#(#inputs),*] } }
            }
            "minecraft:min" | "min" => {
                let inputs = aggregate_inputs(object, "min");
                quote! { NumberProvider::Minimum { inputs: &[#(#inputs),*] } }
            }
            "minecraft:max" | "max" => {
                let inputs = aggregate_inputs(object, "max");
                quote! { NumberProvider::Maximum { inputs: &[#(#inputs),*] } }
            }
            "minecraft:mul" | "mul" => {
                let inputs = aggregate_inputs(object, "mul");
                quote! { NumberProvider::Product { inputs: &[#(#inputs),*] } }
            }
            "minecraft:length" | "length" => {
                let inputs = aggregate_inputs(object, "length");
                quote! { NumberProvider::Length { inputs: &[#(#inputs),*] } }
            }
            "minecraft:sub" | "sub" => {
                let (left, right) = binary_inputs(object);
                quote! { NumberProvider::Difference { left: &#left, right: &#right } }
            }
            "minecraft:div" | "div" => {
                let (left, right) = binary_inputs(object);
                quote! { NumberProvider::Quotient { left: &#left, right: &#right } }
            }
            "minecraft:floor_div" | "floor_div" => {
                let (left, right) = binary_inputs(object);
                quote! {
                    NumberProvider::FloorQuotient {
                        left: &#left,
                        right: &#right,
                    }
                }
            }
            "minecraft:mod" | "mod" => {
                let (left, right) = binary_inputs(object);
                quote! { NumberProvider::Modulus { left: &#left, right: &#right } }
            }
            "minecraft:floor_mod" | "floor_mod" => {
                let (left, right) = binary_inputs(object);
                quote! {
                    NumberProvider::FloorModulus {
                        left: &#left,
                        right: &#right,
                    }
                }
            }
            "minecraft:pow" | "pow" => {
                let base = object
                    .base
                    .as_ref()
                    .unwrap_or_else(|| panic!("minecraft:pow is missing `base`"));
                let exponent = object
                    .exponent
                    .as_ref()
                    .unwrap_or_else(|| panic!("minecraft:pow is missing `exponent`"));
                let base = generate_number_provider(base);
                let exponent = generate_number_provider(exponent);
                quote! {
                    NumberProvider::Power {
                        base: &#base,
                        exponent: &#exponent,
                    }
                }
            }
            "minecraft:negate" | "negate" => {
                let input = unary_input(object, "negate");
                quote! { NumberProvider::Negate { input: &#input } }
            }
            "minecraft:abs" | "abs" => {
                let input = unary_input(object, "abs");
                quote! { NumberProvider::Absolute { input: &#input } }
            }
            "minecraft:ceil" | "ceil" => {
                let input = unary_input(object, "ceil");
                quote! { NumberProvider::Ceiling { input: &#input } }
            }
            "minecraft:floor" | "floor" => {
                let input = unary_input(object, "floor");
                quote! { NumberProvider::Floor { input: &#input } }
            }
            "minecraft:round" | "round" => {
                let input = unary_input(object, "round");
                quote! { NumberProvider::Round { input: &#input } }
            }
            "minecraft:truncate" | "truncate" => {
                let input = unary_input(object, "truncate");
                quote! { NumberProvider::Truncate { input: &#input } }
            }
            "minecraft:sqrt" | "sqrt" => {
                let input = unary_input(object, "sqrt");
                quote! { NumberProvider::SquareRoot { input: &#input } }
            }
            "minecraft:sin" | "sin" => {
                let input = unary_input(object, "sin");
                quote! { NumberProvider::Sine { input: &#input } }
            }
            "minecraft:cos" | "cos" => {
                let input = unary_input(object, "cos");
                quote! { NumberProvider::Cosine { input: &#input } }
            }
            "minecraft:from_float" | "from_float" => {
                let input = unary_input(object, "from_float");
                quote! { NumberProvider::FromFloat { input: &#input } }
            }
            "minecraft:from_int" | "from_int" => {
                let input = unary_input(object, "from_int");
                quote! { NumberProvider::FromInt { input: &#input } }
            }
            "minecraft:conditional" | "conditional" => {
                let condition = object.condition.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:conditional number provider is missing `condition`")
                });
                let on_true = object.on_true.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:conditional number provider is missing `on_true`")
                });
                let on_false = object.on_false.as_ref().map_or_else(
                    || quote! { &NumberProvider::Constant(0.0) },
                    generate_number_provider,
                );
                let condition = generate_condition(condition);
                let on_true = generate_number_provider(on_true);
                quote! {
                    NumberProvider::Conditional {
                        condition: &#condition,
                        on_true: &#on_true,
                        on_false: #on_false,
                    }
                }
            }
            "minecraft:number_dispatcher" | "number_dispatcher" => {
                let cases = object.cases.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:number_dispatcher number provider is missing `cases`")
                });
                let cases = cases.iter().map(|case| {
                    let condition = generate_condition(&case.condition);
                    let value = generate_number_provider(&case.value);
                    quote! { (&#condition, #value) }
                });
                let default = object.default.as_ref().map_or_else(
                    || quote! { &NumberProvider::Constant(0.0) },
                    generate_number_provider,
                );
                quote! {
                    NumberProvider::Dispatcher {
                        cases: &[#(#cases),*],
                        default: #default,
                    }
                }
            }
            "minecraft:weighted_list" | "weighted_list" => {
                let distribution = object.distribution.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:weighted_list number provider is missing `distribution`")
                });
                assert!(
                    !distribution.is_empty(),
                    "minecraft:weighted_list requires at least one entry"
                );
                let distribution = distribution.iter().map(|entry| {
                    let provider = generate_number_provider(&entry.data);
                    let weight = entry.weight;
                    assert!(
                        weight >= 0,
                        "minecraft:weighted_list weights must be non-negative"
                    );
                    quote! { (#provider, #weight) }
                });
                quote! { NumberProvider::WeightedList { distribution: &[#(#distribution),*] } }
            }
            "minecraft:score" | "score" => {
                let target = object.target.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:score number provider is missing `target`")
                });
                let target = generate_scoreboard_target(target);
                let score = object.score.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:score number provider is missing `score`")
                });
                let scale = object.scale.unwrap_or(1.0);
                let fallback = object.fallback.as_ref().map_or_else(
                    || quote! { &NumberProvider::Constant(0.0) },
                    generate_number_provider,
                );
                quote! {
                    NumberProvider::Score {
                        target: #target,
                        score: #score,
                        scale: #scale,
                        fallback: #fallback,
                    }
                }
            }
            "minecraft:storage" | "storage" => {
                let storage = object.storage.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:storage number provider is missing `storage`")
                });
                let path = object.path.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:storage number provider is missing `path`")
                });
                let storage = generate_identifier_token(storage);
                let fallback = object.fallback.as_ref().map_or_else(
                    || quote! { &NumberProvider::Constant(0.0) },
                    generate_number_provider,
                );
                quote! {
                    NumberProvider::Storage {
                        storage: #storage,
                        path: #path,
                        fallback: #fallback,
                    }
                }
            }
            "minecraft:enchantment_level" | "enchantment_level" => {
                let amount = object.amount.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:enchantment_level number provider is missing `amount`")
                });
                let amount = generate_level_based_value_ref_token(amount);
                quote! {
                    NumberProvider::EnchantmentLevel {
                        amount: #amount,
                    }
                }
            }
            "minecraft:environment_attribute" | "environment_attribute" => {
                let attribute = object.attribute.as_ref().unwrap_or_else(|| {
                    panic!("minecraft:environment_attribute number provider is missing `attribute`")
                });
                let attribute = generate_identifier_token(attribute);
                quote! {
                    NumberProvider::EnvironmentAttribute {
                        attribute: #attribute,
                    }
                }
            }
            other => panic!("unsupported number provider type {other:?}"),
        },
    }
}

/// The token for a nested provider field, or `fallback` when the JSON omitted it.
fn provider_or_default(
    provider: &Option<NumberProviderJson>,
    fallback: TokenStream,
) -> TokenStream {
    match provider {
        Some(provider) => generate_number_provider(provider),
        None => fallback,
    }
}

/// The numeric value of a JSON number provider when it is a compile-time constant
/// (a bare number or `minecraft:constant`), used to fold `uniform`/`binomial`
/// bounds into their constant forms.
fn constant_of(value: &NumberProviderJson) -> Option<f32> {
    match value {
        NumberProviderJson::Constant(value) => Some(*value),
        NumberProviderJson::Reference(_) => None,
        NumberProviderJson::Object(object) => match object.provider_type.as_str() {
            "minecraft:constant" => object.value.as_ref().and_then(constant_of),
            _ => None,
        },
    }
}

/// The required inputs of an aggregate provider (`inputs`, e.g. `minecraft:add`).
fn aggregate_inputs(object: &NumberProviderObjectJson, provider_type: &str) -> Vec<TokenStream> {
    let inputs = object
        .inputs
        .as_ref()
        .unwrap_or_else(|| panic!("minecraft:{provider_type} number provider is missing `inputs`"));
    assert!(
        !inputs.is_empty(),
        "minecraft:{provider_type} number provider requires at least one input"
    );
    inputs.iter().map(generate_number_provider).collect()
}

/// The required operands of a binary provider (`left`/`right`, e.g. `minecraft:div`).
fn binary_inputs(object: &NumberProviderObjectJson) -> (TokenStream, TokenStream) {
    let left = object.left.as_ref().map_or_else(
        || panic!("number provider is missing `left`"),
        generate_number_provider,
    );
    let right = object.right.as_ref().map_or_else(
        || panic!("number provider is missing `right`"),
        generate_number_provider,
    );
    (left, right)
}

/// The required operand of a unary provider (`input`, e.g. `minecraft:abs`).
fn unary_input(object: &NumberProviderObjectJson, provider_type: &str) -> TokenStream {
    object.input.as_ref().map_or_else(
        || panic!("minecraft:{provider_type} number provider is missing `input`"),
        generate_number_provider,
    )
}

/// The score holder of a `minecraft:score` provider: either a bare holder-name
/// string (vanilla `ContextScoreboardNameProvider.INLINE_CODEC`) or a
/// `minecraft:fixed`/`minecraft:context` object.
fn generate_scoreboard_target(target: &serde_json::Value) -> TokenStream {
    if let Some(name) = target.as_str() {
        return match name {
            "this" => {
                quote! { crate::loot_table::ScoreboardTarget::Context(LootContextEntity::This) }
            }
            "killer" | "attacker" => {
                quote! { crate::loot_table::ScoreboardTarget::Context(LootContextEntity::Killer) }
            }
            "direct_killer" | "direct_attacker" => {
                quote! { crate::loot_table::ScoreboardTarget::Context(LootContextEntity::DirectKiller) }
            }
            "killer_player" | "last_damage_player" => {
                quote! { crate::loot_table::ScoreboardTarget::Context(LootContextEntity::KillerPlayer) }
            }
            _ => quote! { crate::loot_table::ScoreboardTarget::Fixed(#name) },
        };
    }
    let object = target
        .as_object()
        .unwrap_or_else(|| panic!("minecraft:score `target` must be a string or object"));
    let provider_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("minecraft:score `target` object is missing `type`"));
    match provider_type {
        "minecraft:fixed" => {
            let name = object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("minecraft:fixed scoreboard target is missing `name`"));
            quote! { crate::loot_table::ScoreboardTarget::Fixed(#name) }
        }
        "minecraft:context" => {
            let target = object
                .get("target")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| {
                    panic!("minecraft:context scoreboard target is missing `target`")
                });
            generate_scoreboard_target(&serde_json::Value::String(target.to_owned()))
        }
        other => panic!("unsupported scoreboard target type {other:?}"),
    }
}

/// An identifier token (`Identifier::vanilla_static` for the `minecraft`
/// namespace, `Identifier::new_static` otherwise).
fn generate_identifier_token(id: &str) -> TokenStream {
    let id = Identifier::from_str(id)
        .unwrap_or_else(|error| panic!("invalid identifier {id:?}: {error}"));
    let namespace = id.namespace.as_ref();
    let path = id.path.as_ref();
    if namespace == "minecraft" {
        quote! { Identifier::vanilla_static(#path) }
    } else {
        quote! { Identifier::new_static(#namespace, #path) }
    }
}

/// A `&'static LevelBasedValue` token for an `minecraft:enchantment_level`
/// `amount`, handling the shapes the enchantment files use: a bare constant, a
/// `linear` formula, or any other level-based-value object mirrored by the
/// enchantment codegen's `LevelBasedValueJson`.
fn generate_level_based_value_ref_token(amount: &serde_json::Value) -> TokenStream {
    if let Some(constant) = amount.as_f64() {
        let constant = constant as f32;
        return quote! { &crate::enchantment::effect::LevelBasedValue::Constant(#constant) };
    }
    let object = amount.as_object().unwrap_or_else(|| {
        panic!("minecraft:enchantment_level `amount` must be a number or object")
    });
    assert!(
        object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        "minecraft:enchantment_level `amount` object is missing `type`"
    );
    /// Recursively generates a `LevelBasedValue` token for one `amount` shape.
    fn generate(value: &serde_json::Value) -> TokenStream {
        if let Some(constant) = value.as_f64() {
            let constant = constant as f32;
            return quote! { crate::enchantment::effect::LevelBasedValue::Constant(#constant) };
        }
        let object = value
            .as_object()
            .unwrap_or_else(|| panic!("level-based value must be a number or object"));
        let provider_type = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_else(|| panic!("level-based value object is missing `type`"));
        let field = |name: &str| {
            object.get(name).unwrap_or_else(|| {
                panic!("level-based value `{provider_type}` is missing `{name}`")
            })
        };
        match provider_type {
            "minecraft:clamped" => {
                let value = generate(field("value"));
                let min = field("min")
                    .as_f64()
                    .unwrap_or_else(|| panic!("level-based value `clamped` `min` must be a number"))
                    as f32;
                let max = field("max")
                    .as_f64()
                    .unwrap_or_else(|| panic!("level-based value `clamped` `max` must be a number"))
                    as f32;
                quote! {
                    crate::enchantment::effect::LevelBasedValue::Clamped {
                        value: &#value,
                        min: #min,
                        max: #max,
                    }
                }
            }
            "minecraft:exponent" => {
                let base = generate(field("base"));
                let power = generate(field("power"));
                quote! {
                    crate::enchantment::effect::LevelBasedValue::Exponent {
                        base: &#base,
                        power: &#power,
                    }
                }
            }
            "minecraft:fraction" => {
                let numerator = generate(field("numerator"));
                let denominator = generate(field("denominator"));
                quote! {
                    crate::enchantment::effect::LevelBasedValue::Fraction {
                        numerator: &#numerator,
                        denominator: &#denominator,
                    }
                }
            }
            "minecraft:levels_squared" => {
                let added = field("added").as_f64().unwrap_or_else(|| {
                    panic!("level-based value `levels_squared` `added` must be a number")
                }) as f32;
                quote! { crate::enchantment::effect::LevelBasedValue::LevelsSquared { added: #added } }
            }
            "minecraft:linear" => {
                let base = field("base")
                    .as_f64()
                    .unwrap_or_else(|| panic!("level-based value `linear` `base` must be a number"))
                    as f32;
                let per_level_above_first =
                    field("per_level_above_first").as_f64().unwrap_or_else(|| {
                        panic!(
                            "level-based value `linear` `per_level_above_first` must be a number"
                        )
                    }) as f32;
                quote! {
                    crate::enchantment::effect::LevelBasedValue::Linear {
                        base: #base,
                        per_level_above_first: #per_level_above_first,
                    }
                }
            }
            "minecraft:lookup" => {
                let values = field("values")
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("level-based value `lookup` `values` must be an array")
                    })
                    .iter()
                    .map(|value| {
                        value.as_f64().unwrap_or_else(|| {
                            panic!("level-based value `lookup` `values` must be numbers")
                        }) as f32
                    })
                    .collect::<Vec<_>>();
                let fallback = generate(field("fallback"));
                quote! {
                    crate::enchantment::effect::LevelBasedValue::Lookup {
                        values: &[#(#values),*],
                        fallback: &#fallback,
                    }
                }
            }
            other => panic!("unsupported level-based value type {other:?}"),
        }
    }

    let amount = generate(amount);
    quote! { &#amount }
}

/// Generate the `LootContextEntity` enum variant at build time.
pub(super) fn generate_loot_context_entity(entity: &str) -> TokenStream {
    match entity {
        "this" => quote! { LootContextEntity::This },
        "killer" | "attacker" => quote! { LootContextEntity::Killer },
        "direct_killer" | "direct_attacker" => quote! { LootContextEntity::DirectKiller },
        "killer_player" | "last_damage_player" => quote! { LootContextEntity::KillerPlayer },
        "interacting_entity" => quote! { LootContextEntity::Interacting },
        _ => quote! { LootContextEntity::This },
    }
}

/// Generate the `EquipmentSlotGroup` enum variant at build time.
#[expect(
    dead_code,
    reason = "loot table generator keeps slot helper for extracted predicate coverage"
)]
pub(super) fn generate_equipment_slot_group(slot: &str) -> TokenStream {
    match slot {
        "any" => quote! { EquipmentSlotGroup::Any },
        "mainhand" | "main_hand" => quote! { EquipmentSlotGroup::MainHand },
        "offhand" | "off_hand" => quote! { EquipmentSlotGroup::OffHand },
        "hand" => quote! { EquipmentSlotGroup::Hand },
        "head" => quote! { EquipmentSlotGroup::Head },
        "chest" => quote! { EquipmentSlotGroup::Chest },
        "legs" => quote! { EquipmentSlotGroup::Legs },
        "feet" => quote! { EquipmentSlotGroup::Feet },
        "armor" => quote! { EquipmentSlotGroup::Armor },
        "body" => quote! { EquipmentSlotGroup::Body },
        _ => quote! { EquipmentSlotGroup::Any },
    }
}

/// Generate the `DyeColor` enum variant at build time.
pub(super) fn generate_dye_color(color: &str) -> TokenStream {
    match color {
        "white" => quote! { DyeColor::White },
        "orange" => quote! { DyeColor::Orange },
        "magenta" => quote! { DyeColor::Magenta },
        "light_blue" => quote! { DyeColor::LightBlue },
        "yellow" => quote! { DyeColor::Yellow },
        "lime" => quote! { DyeColor::Lime },
        "pink" => quote! { DyeColor::Pink },
        "gray" => quote! { DyeColor::Gray },
        "light_gray" => quote! { DyeColor::LightGray },
        "cyan" => quote! { DyeColor::Cyan },
        "purple" => quote! { DyeColor::Purple },
        "blue" => quote! { DyeColor::Blue },
        "brown" => quote! { DyeColor::Brown },
        "green" => quote! { DyeColor::Green },
        "red" => quote! { DyeColor::Red },
        "black" => quote! { DyeColor::Black },
        _ => quote! { DyeColor::White },
    }
}

/// Generate the `LootType` enum variant at build time.
pub(super) fn generate_loot_type(loot_type: &str) -> TokenStream {
    match loot_type {
        "minecraft:block" => quote! { LootType::Block },
        "minecraft:entity" => quote! { LootType::Entity },
        "minecraft:chest" => quote! { LootType::Chest },
        "minecraft:fishing" => quote! { LootType::Fishing },
        "minecraft:gift" => quote! { LootType::Gift },
        "minecraft:archaeology" => quote! { LootType::Archaeology },
        "minecraft:vault" => quote! { LootType::Vault },
        "minecraft:shearing" => quote! { LootType::Shearing },
        "minecraft:equipment" => quote! { LootType::Equipment },
        "minecraft:selector" => quote! { LootType::Selector },
        "minecraft:entity_interact" => quote! { LootType::EntityInteract },
        "minecraft:block_interact" => quote! { LootType::BlockInteract },
        "minecraft:barter" => quote! { LootType::Barter },
        _ => quote! { LootType::Block }, // Default to Block
    }
}

pub(super) fn generate_tool_predicate(predicate: &Option<PredicateJson>) -> TokenStream {
    let Some(pred) = predicate else {
        return quote! { ToolPredicate::Any };
    };

    // Only handle tool predicates; location/entity/damage_source predicates return Any
    let pred = match pred {
        PredicateJson::Tool(p) => p,
        PredicateJson::Location(_) => return quote! { ToolPredicate::Any },
        PredicateJson::DamageSource(_) => return quote! { ToolPredicate::Any },
        PredicateJson::Entity(_) => return quote! { ToolPredicate::Any },
    };

    // Check for items field (can be a string or tag reference)
    if let Some(item_str) = &pred.items {
        if item_str.starts_with('#') {
            // Tag reference like "#minecraft:pickaxes"
            let tag = item_str
                .strip_prefix("#minecraft:")
                .unwrap_or(item_str.strip_prefix('#').unwrap_or(item_str));
            return quote! { ToolPredicate::Tag(Identifier::vanilla_static(#tag)) };
        } else {
            let item = item_str.strip_prefix("minecraft:").unwrap_or(item_str);
            return quote! { ToolPredicate::Item(Identifier::vanilla_static(#item)) };
        }
    }

    // Check for enchantment predicates
    if let Some(predicates) = &pred.predicates
        && let Some(enchants) = &predicates.enchantments
        && let Some(first) = enchants.first()
        && let Some(enchant_name) = &first.enchantments
    {
        let enchant_name = enchant_name.strip_prefix("#minecraft:").unwrap_or(
            enchant_name
                .strip_prefix("minecraft:")
                .unwrap_or(enchant_name),
        );
        let min_level = first
            .levels
            .as_ref()
            .and_then(LevelRangeJson::min_level)
            .unwrap_or(1);

        return quote! {
            ToolPredicate::HasEnchantment {
                enchantment: Identifier::vanilla_static(#enchant_name),
                min_level: #min_level,
            }
        };
    }

    quote! { ToolPredicate::Any }
}

pub(super) fn generate_enchantment_options(
    options: &Option<EnchantmentOptionsJson>,
) -> TokenStream {
    match options {
        Some(EnchantmentOptionsJson::Tag(s)) => {
            let tag = s
                .strip_prefix("#minecraft:")
                .unwrap_or(s.strip_prefix("minecraft:").unwrap_or(s));
            quote! { EnchantmentOptions::Tag(Identifier::vanilla_static(#tag)) }
        }
        Some(EnchantmentOptionsJson::List(arr)) => {
            let enchants: Vec<TokenStream> = arr
                .iter()
                .map(|s| {
                    let s = s.strip_prefix("minecraft:").unwrap_or(s);
                    quote! { Identifier::vanilla_static(#s) }
                })
                .collect();
            quote! { EnchantmentOptions::List(&[#(#enchants),*]) }
        }
        None => {
            quote! { EnchantmentOptions::Tag(Identifier::vanilla_static("on_random_loot")) }
        }
    }
}

pub(super) fn generate_instrument_ref(value: &str) -> TokenStream {
    let id = Identifier::from_str(value)
        .unwrap_or_else(|error| panic!("invalid instrument id {value:?}: {error}"));
    assert_eq!(
        id.namespace.as_ref(),
        "minecraft",
        "vanilla loot table references a non-vanilla instrument: {id}"
    );
    let ident = Ident::new(&id.path.to_shouty_snake_case(), Span::call_site());
    quote! { &crate::vanilla_instruments::#ident }
}

pub(super) fn generate_instrument_options(options: &Option<EnchantmentOptionsJson>) -> TokenStream {
    match options {
        Some(EnchantmentOptionsJson::Tag(value)) if value.starts_with('#') => {
            let tag = value.trim_start_matches('#');
            let id = Identifier::from_str(tag)
                .unwrap_or_else(|error| panic!("invalid instrument tag {value:?}: {error}"));
            let namespace = id.namespace.as_ref();
            let path = id.path.as_ref();
            quote! {
                InstrumentOptions::Tag(Identifier::new_static(#namespace, #path))
            }
        }
        Some(EnchantmentOptionsJson::Tag(value)) => {
            let instrument = generate_instrument_ref(value);
            quote! { InstrumentOptions::Direct(&[#instrument]) }
        }
        Some(EnchantmentOptionsJson::List(values)) => {
            let instruments = values.iter().map(|value| generate_instrument_ref(value));
            quote! { InstrumentOptions::Direct(&[#(#instruments),*]) }
        }
        None => panic!("set_instrument function is missing its options holder set"),
    }
}

/// Generates an `Option<PotionOptions>` for `minecraft:set_random_potion`.
pub(crate) fn generate_potion_options(options: &Option<EnchantmentOptionsJson>) -> TokenStream {
    match options {
        Some(EnchantmentOptionsJson::Tag(value)) if value.starts_with('#') => {
            let tag = Identifier::from_str(value.trim_start_matches('#'))
                .unwrap_or_else(|error| panic!("invalid potion tag {value:?}: {error}"));
            assert_eq!(
                tag.namespace.as_ref(),
                "minecraft",
                "set_random_potion tags must use the minecraft namespace: {tag}"
            );
            let path = tag.path.as_ref();
            quote! { Some(PotionOptions::Tag(Identifier::vanilla_static(#path))) }
        }
        Some(EnchantmentOptionsJson::Tag(value)) => {
            let potion = generate_potion_ref(value);
            quote! { Some(PotionOptions::Direct(&[#potion])) }
        }
        Some(EnchantmentOptionsJson::List(values)) => {
            let potions = values.iter().map(|value| generate_potion_ref(value));
            quote! { Some(PotionOptions::Direct(&[#(#potions),*])) }
        }
        // Vanilla falls back to a random potion from the whole registry.
        None => quote! { None },
    }
}

/// Generates a `&'static PotionRef` for a vanilla potion id.
fn generate_potion_ref(value: &str) -> TokenStream {
    let id = Identifier::from_str(value)
        .unwrap_or_else(|error| panic!("invalid potion id {value:?}: {error}"));
    assert_eq!(
        id.namespace.as_ref(),
        "minecraft",
        "vanilla loot table references a non-vanilla potion: {id}"
    );
    let ident = Ident::new(&id.path.to_shouty_snake_case(), Span::call_site());
    quote! { &crate::vanilla_potions::#ident }
}

pub(super) fn generate_entity_flags(flags: &Option<EntityFlagsJson>) -> TokenStream {
    if let Some(f) = flags {
        let is_on_fire = if let Some(v) = f.is_on_fire {
            quote! { Some(#v) }
        } else {
            quote! { None }
        };
        let is_sneaking = if let Some(v) = f.is_sneaking {
            quote! { Some(#v) }
        } else {
            quote! { None }
        };
        let is_sprinting = if let Some(v) = f.is_sprinting {
            quote! { Some(#v) }
        } else {
            quote! { None }
        };
        let is_swimming = if let Some(v) = f.is_swimming {
            quote! { Some(#v) }
        } else {
            quote! { None }
        };
        let is_baby = if let Some(v) = f.is_baby {
            quote! { Some(#v) }
        } else {
            quote! { None }
        };
        quote! {
            Some(EntityFlags {
                is_on_fire: #is_on_fire,
                is_sneaking: #is_sneaking,
                is_sprinting: #is_sprinting,
                is_swimming: #is_swimming,
                is_baby: #is_baby,
            })
        }
    } else {
        quote! { None }
    }
}

pub(super) fn generate_equipment_slot_predicate(slot: &Option<EquipmentSlotJson>) -> TokenStream {
    if let Some(s) = slot {
        if let Some(items) = &s.items {
            if items.starts_with('#') {
                let tag = items
                    .strip_prefix("#minecraft:")
                    .unwrap_or(items.strip_prefix('#').unwrap_or(items));
                return quote! { Some(ToolPredicate::Tag(Identifier::vanilla_static(#tag))) };
            } else {
                let item = items.strip_prefix("minecraft:").unwrap_or(items);
                return quote! { Some(ToolPredicate::Item(Identifier::vanilla_static(#item))) };
            }
        }

        if let Some(predicates) = &s.predicates
            && let Some(enchants) = &predicates.enchantments
            && let Some(first) = enchants.first()
            && let Some(enchant_name) = &first.enchantments
        {
            let enchant_name = enchant_name.strip_prefix("#minecraft:").unwrap_or(
                enchant_name
                    .strip_prefix("minecraft:")
                    .unwrap_or(enchant_name),
            );
            let min_level = first
                .levels
                .as_ref()
                .and_then(LevelRangeJson::min_level)
                .unwrap_or(1);
            return quote! {
                Some(ToolPredicate::HasEnchantment {
                    enchantment: Identifier::vanilla_static(#enchant_name),
                    min_level: #min_level,
                })
            };
        }

        quote! { Some(ToolPredicate::Any) }
    } else {
        quote! { None }
    }
}

pub(super) fn generate_entity_equipment(equipment: &Option<EntityEquipmentJson>) -> TokenStream {
    if let Some(e) = equipment {
        let mainhand = generate_equipment_slot_predicate(&e.mainhand);
        let offhand = generate_equipment_slot_predicate(&e.offhand);
        let head = generate_equipment_slot_predicate(&e.head);
        let chest = generate_equipment_slot_predicate(&e.chest);
        let legs = generate_equipment_slot_predicate(&e.legs);
        let feet = generate_equipment_slot_predicate(&e.feet);

        quote! {
            Some(EntityEquipment {
                mainhand: #mainhand,
                offhand: #offhand,
                head: #head,
                chest: #chest,
                legs: #legs,
                feet: #feet,
            })
        }
    } else {
        quote! { None }
    }
}

pub(super) fn generate_entity_predicate(predicate: &EntityPredicateJson) -> TokenStream {
    let entity_type = if let Some(t) = &predicate.entity_type {
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        quote! { Some(Identifier::vanilla_static(#t)) }
    } else {
        quote! { None }
    };

    let flags = generate_entity_flags(&predicate.flags);
    let equipment = generate_entity_equipment(&predicate.equipment);

    let sheep_color = predicate
        .components
        .as_ref()
        .and_then(|components| components.sheep_color.as_deref())
        .map_or_else(
            || quote! { None },
            |color| {
                let color = generate_dye_color(color);
                quote! { Some(#color) }
            },
        );
    let chicken_variant = predicate
        .components
        .as_ref()
        .and_then(|components| components.chicken_variant.as_deref())
        .map_or_else(
            || quote! { None },
            |variant| {
                let variant = variant.strip_prefix("minecraft:").unwrap_or(variant);
                quote! { Some(Identifier::vanilla_static(#variant)) }
            },
        );
    let sheep_sheared = predicate
        .sheep_type_specific
        .as_ref()
        .and_then(|sheep| sheep.sheared)
        .map_or_else(|| quote! { None }, |sheared| quote! { Some(#sheared) });

    quote! {
        EntityPredicate {
            entity_type: #entity_type,
            flags: #flags,
            equipment: #equipment,
            sheep_color: #sheep_color,
            sheep_sheared: #sheep_sheared,
            chicken_variant: #chicken_variant,
        }
    }
}

pub(super) fn generate_damage_source_predicate(
    predicate: &DamageSourcePredicateJson,
) -> TokenStream {
    let tags: Vec<TokenStream> = predicate.tags.as_ref().map_or_default(|t| {
        t.iter()
            .map(|tag| {
                let id = tag.id.strip_prefix("minecraft:").unwrap_or(&tag.id);
                let expected = tag.expected;
                quote! {
                    DamageTagPredicate {
                        id: Identifier::vanilla_static(#id),
                        expected: #expected,
                    }
                }
            })
            .collect()
    });

    let source_entity = if let Some(e) = &predicate.source_entity {
        let pred = generate_entity_predicate(e);
        quote! { Some(#pred) }
    } else {
        quote! { None }
    };

    let direct_entity = if let Some(e) = &predicate.direct_entity {
        let pred = generate_entity_predicate(e);
        quote! { Some(#pred) }
    } else {
        quote! { None }
    };

    let is_direct = if let Some(v) = predicate.is_direct {
        quote! { Some(#v) }
    } else {
        quote! { None }
    };

    quote! {
        DamageSourcePredicate {
            tags: &[#(#tags),*],
            source_entity: #source_entity,
            direct_entity: #direct_entity,
            is_direct: #is_direct,
        }
    }
}

/// A vanilla `HolderSet` token: `#tag`, one key, or a key list.
fn generate_holder_set(set: &HolderSetJson) -> TokenStream {
    let key = |value: &str| {
        let key = value.strip_prefix("minecraft:").unwrap_or(value);
        quote! { #key }
    };

    match set {
        HolderSetJson::Single(value) => {
            if let Some(tag) = value.strip_prefix('#') {
                let tag = generate_identifier_token(tag);
                quote! { HolderSet::Tag(#tag) }
            } else {
                let key = key(value);
                quote! { HolderSet::Keys(&[#key]) }
            }
        }
        HolderSetJson::List(values) => {
            let keys: Vec<TokenStream> = values
                .iter()
                .map(|value| {
                    assert!(
                        !value.starts_with('#'),
                        "vanilla holder sets cannot mix tags into a key list: {value}"
                    );
                    key(value)
                })
                .collect();
            quote! { HolderSet::Keys(&[#(#keys),*]) }
        }
    }
}

fn generate_block_predicate(predicate: &BlockPredicateJson) -> TokenStream {
    let blocks = predicate.blocks.as_ref().map_or_else(
        || quote! { None },
        |set| {
            let set = generate_holder_set(set);
            quote! { Some(#set) }
        },
    );

    let state = predicate.state.as_ref().map_or_else(
        || quote! { None },
        |props| {
            let properties: Vec<TokenStream> = props
                .iter()
                .map(|(name, value)| {
                    quote! { PropertyCheck { name: #name, value: #value } }
                })
                .collect();
            quote! {
                Some(StatePropertiesPredicate {
                    properties: &[#(#properties),*],
                })
            }
        },
    );

    quote! {
        BlockPredicate {
            blocks: #blocks,
            state: #state,
        }
    }
}

/// Vanilla `MinMaxBounds.Doubles` token.
fn generate_double_bounds(bounds: &DoubleBoundsJson) -> TokenStream {
    match bounds {
        DoubleBoundsJson::Exact(value) => quote! { DoubleBounds::exactly(#value) },
        DoubleBoundsJson::Bounds { min, max } => match (min, max) {
            (Some(min), Some(max)) => quote! { DoubleBounds::range(#min, #max) },
            (Some(min), None) => quote! { DoubleBounds::at_least(#min) },
            (None, Some(max)) => quote! { DoubleBounds::at_most(#max) },
            (None, None) => quote! { DoubleBounds::ANY },
        },
    }
}

/// Vanilla `MinMaxBounds.Ints` token.
fn generate_int_bounds(bounds: &LevelRangeJson) -> TokenStream {
    match bounds {
        LevelRangeJson::Exact(value) => quote! { IntBounds::exactly(#value) },
        LevelRangeJson::Bounds { min, max } => match (min, max) {
            (Some(min), Some(max)) => quote! { IntBounds::range(#min, #max) },
            (Some(min), None) => quote! { IntBounds::at_least(#min) },
            (None, Some(max)) => quote! { IntBounds::at_most(#max) },
            (None, None) => quote! { IntBounds::ANY },
        },
    }
}

fn generate_position_predicate(predicate: &PositionPredicateJson) -> TokenStream {
    let axis = |bounds: &Option<DoubleBoundsJson>| {
        bounds.as_ref().map_or_else(
            || quote! { None },
            |bounds| {
                let bounds = generate_double_bounds(bounds);
                quote! { Some(#bounds) }
            },
        )
    };
    let x = axis(&predicate.x);
    let y = axis(&predicate.y);
    let z = axis(&predicate.z);

    quote! { PositionPredicate { x: #x, y: #y, z: #z } }
}

pub(super) fn generate_location_predicate(predicate: &LocationPredicateJson) -> TokenStream {
    let position = predicate.position.as_ref().map_or_else(
        || quote! { None },
        |position| {
            let position = generate_position_predicate(position);
            quote! { Some(#position) }
        },
    );
    let biomes = predicate.biomes.as_ref().map_or_else(
        || quote! { None },
        |set| {
            let set = generate_holder_set(set);
            quote! { Some(#set) }
        },
    );
    let structures = predicate.structures.as_ref().map_or_else(
        || quote! { None },
        |set| {
            let set = generate_holder_set(set);
            quote! { Some(#set) }
        },
    );
    let dimension = predicate.dimension.as_deref().map_or_else(
        || quote! { None },
        |dimension| {
            let dimension = generate_identifier_token(dimension);
            quote! { Some(#dimension) }
        },
    );
    let light = predicate.light.as_ref().map_or_else(
        || quote! { None },
        |bounds| {
            let bounds = generate_int_bounds(bounds);
            quote! { Some(#bounds) }
        },
    );
    let block = predicate.block.as_ref().map_or_else(
        || quote! { None },
        |block| {
            let block = generate_block_predicate(block);
            quote! { Some(#block) }
        },
    );
    let fluid = predicate.fluid.as_deref().map_or_else(
        || quote! { None },
        |fluid| {
            let fluid = generate_identifier_token(fluid);
            quote! { Some(#fluid) }
        },
    );
    let smokey = predicate.smokey.map_or_else(
        || quote! { None },
        |value| {
            quote! { Some(#value) }
        },
    );
    let can_see_sky = predicate
        .can_see_sky
        .map_or_else(|| quote! { None }, |value| quote! { Some(#value) });

    quote! {
        LocationPredicate {
            position: #position,
            biomes: #biomes,
            structures: #structures,
            dimension: #dimension,
            light: #light,
            block: #block,
            fluid: #fluid,
            smokey: #smokey,
            can_see_sky: #can_see_sky,
        }
    }
}
