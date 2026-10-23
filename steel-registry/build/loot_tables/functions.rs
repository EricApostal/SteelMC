use super::{
    EnchantmentPredicateJson, EnchantmentSourceJson, ItemFilterJson, ItemSetJson, LimitJson,
    LootFunctionJson, TokenStream, generate_condition, generate_enchantment_options,
    generate_instrument_options, generate_number_provider, generate_potion_options, quote,
};
use std::str::FromStr;
use steel_utils::Identifier;

pub(crate) fn generate_function(function: &LootFunctionJson) -> TokenStream {
    let func_body = match function.function.as_str() {
        "minecraft:set_count" => {
            let count = function.count.as_ref().map_or_else(
                || quote! { NumberProvider::Constant(1.0) },
                generate_number_provider,
            );
            let add = function.add;
            quote! { LootFunction::SetCount { count: #count, add: #add } }
        }
        "minecraft:explosion_decay" => {
            quote! { LootFunction::ExplosionDecay }
        }
        "minecraft:apply_bonus" => {
            let enchantment = function
                .enchantment
                .as_deref()
                .unwrap_or("minecraft:fortune");
            let enchantment = enchantment
                .strip_prefix("minecraft:")
                .unwrap_or(enchantment);

            let formula = match function.formula.as_deref() {
                Some("minecraft:ore_drops") => {
                    quote! { BonusFormula::OreDrops }
                }
                Some("minecraft:uniform_bonus_count") => {
                    let multiplier = function
                        .parameters
                        .as_ref()
                        .and_then(|p| p.bonus_multiplier)
                        .unwrap_or(1);
                    quote! { BonusFormula::UniformBonusCount { bonus_multiplier: #multiplier } }
                }
                Some("minecraft:binomial_with_bonus_count") => {
                    let extra = function
                        .parameters
                        .as_ref()
                        .and_then(|p| p.extra)
                        .unwrap_or(0);
                    let probability = function
                        .parameters
                        .as_ref()
                        .and_then(|p| p.probability)
                        .unwrap_or(0.5);
                    quote! { BonusFormula::BinomialWithBonusCount { extra: #extra, probability: #probability } }
                }
                _ => {
                    quote! { BonusFormula::OreDrops }
                }
            };

            quote! {
                LootFunction::ApplyBonus {
                    enchantment: Identifier::vanilla_static(#enchantment),
                    formula: #formula,
                }
            }
        }
        "minecraft:enchanted_count_increase" => {
            let enchantment = function
                .enchantment
                .as_deref()
                .unwrap_or("minecraft:looting");
            let enchantment = enchantment
                .strip_prefix("minecraft:")
                .unwrap_or(enchantment);

            let count = function.count.as_ref().map_or_else(
                || quote! { NumberProvider::Uniform { min: 0.0, max: 1.0 } },
                generate_number_provider,
            );

            let limit = match &function.limit {
                Some(LimitJson::Integer(v)) => *v,
                Some(LimitJson::Object { max, .. }) => max.map_or(0, |v| v as i32),
                None => 0,
            };

            quote! {
                LootFunction::EnchantedCountIncrease {
                    enchantment: Identifier::vanilla_static(#enchantment),
                    count: #count,
                    limit: #limit,
                }
            }
        }
        "minecraft:limit_count" => {
            let (min, max) = match &function.limit {
                Some(LimitJson::Integer(v)) => (Some(*v), Some(*v)),
                Some(LimitJson::Object { min, max }) => {
                    (min.map(|v| v as i32), max.map(|v| v as i32))
                }
                None => (None, None),
            };

            let min_tokens = if let Some(v) = min {
                quote! { Some(#v) }
            } else {
                quote! { None }
            };
            let max_tokens = if let Some(v) = max {
                quote! { Some(#v) }
            } else {
                quote! { None }
            };

            quote! { LootFunction::LimitCount { min: #min_tokens, max: #max_tokens } }
        }
        "minecraft:set_damage" => {
            let damage = function.damage.as_ref().map_or_else(
                || quote! { NumberProvider::Constant(1.0) },
                generate_number_provider,
            );
            let add = function.add;
            quote! { LootFunction::SetDamage { damage: #damage, add: #add } }
        }
        "minecraft:enchant_randomly" => {
            let options = generate_enchantment_options(&function.options);
            let only_compatible = function.only_compatible.unwrap_or(true);
            let include_additional_cost_component =
                function.include_additional_cost_component.unwrap_or(false);
            quote! {
                LootFunction::EnchantRandomly {
                    options: #options,
                    only_compatible: #only_compatible,
                    include_additional_cost_component: #include_additional_cost_component,
                }
            }
        }
        "minecraft:enchant_with_levels" => {
            let levels = function.levels.as_ref().map_or_else(
                || quote! { NumberProvider::Constant(30.0) },
                generate_number_provider,
            );
            let options = generate_enchantment_options(&function.options);
            quote! {
                LootFunction::EnchantWithLevels {
                    levels: #levels,
                    options: #options,
                }
            }
        }
        "minecraft:copy_components" => {
            let source = match function.source.as_deref() {
                Some("block_entity") => quote! { CopySource::BlockEntity },
                Some("this") => quote! { CopySource::This },
                Some("attacker") => quote! { CopySource::Attacker },
                Some("direct_attacker") => quote! { CopySource::DirectAttacker },
                _ => quote! { CopySource::BlockEntity },
            };

            let include: Vec<TokenStream> = function.include.as_ref().map_or_default(|inc| {
                inc.iter()
                    .map(|s| {
                        let s = s.strip_prefix("minecraft:").unwrap_or(s);
                        quote! { Identifier::vanilla_static(#s) }
                    })
                    .collect()
            });

            quote! {
                LootFunction::CopyComponents {
                    source: #source,
                    include: &[#(#include),*],
                }
            }
        }
        "minecraft:copy_state" => {
            let block = function.block.as_deref().unwrap_or("minecraft:air");
            let block = block.strip_prefix("minecraft:").unwrap_or(block);

            let properties: Vec<TokenStream> = function
                .properties
                .as_ref()
                .map_or_default(|props| props.iter().map(|p| quote! { #p }).collect());

            quote! {
                LootFunction::CopyState {
                    block: Identifier::vanilla_static(#block),
                    properties: &[#(#properties),*],
                }
            }
        }
        "minecraft:set_components" => {
            let components_str = function
                .components
                .as_ref()
                .map_or_else(|| "{}".to_string(), std::string::ToString::to_string);
            quote! { LootFunction::SetComponents { components: #components_str } }
        }
        "minecraft:furnace_smelt" => {
            let use_input_count = function.use_input_count.unwrap_or(true);
            quote! { LootFunction::FurnaceSmelt { use_input_count: #use_input_count } }
        }
        "minecraft:exploration_map" => {
            let destination = function
                .destination
                .as_deref()
                .unwrap_or("minecraft:buried_treasure");
            let destination = destination
                .strip_prefix("minecraft:")
                .unwrap_or(destination);

            let decoration = function.decoration.as_deref().unwrap_or("minecraft:red_x");
            let decoration = decoration.strip_prefix("minecraft:").unwrap_or(decoration);

            let zoom = function.zoom.unwrap_or(2);
            let skip_existing_chunks = function.skip_existing_chunks.unwrap_or(true);
            let search_radius = function.search_radius.unwrap_or(50);

            quote! {
                LootFunction::ExplorationMap {
                    destination: Identifier::vanilla_static(#destination),
                    decoration: Identifier::vanilla_static(#decoration),
                    zoom: #zoom,
                    skip_existing_chunks: #skip_existing_chunks,
                    search_radius: #search_radius,
                }
            }
        }
        "minecraft:set_name" => {
            let name_str = function
                .name
                .as_ref()
                .map_or_else(|| "\"\"".to_string(), std::string::ToString::to_string);

            let target = match function.target.as_deref() {
                Some("custom_name") => quote! { NameTarget::CustomName },
                Some("item_name") => quote! { NameTarget::ItemName },
                _ => quote! { NameTarget::CustomName },
            };

            quote! {
                LootFunction::SetName {
                    name: #name_str,
                    target: #target,
                }
            }
        }
        "minecraft:set_ominous_bottle_amplifier" => {
            let amplifier = function.amplifier.as_ref().map_or_else(
                || quote! { NumberProvider::Constant(0.0) },
                generate_number_provider,
            );
            quote! { LootFunction::SetOminousBottleAmplifier { amplifier: #amplifier } }
        }
        "minecraft:set_potion" => {
            let id = function.id.as_deref().unwrap_or("minecraft:water");
            let id = id.strip_prefix("minecraft:").unwrap_or(id);
            quote! { LootFunction::SetPotion { id: Identifier::vanilla_static(#id) } }
        }
        "minecraft:set_random_potion" => {
            let options = generate_potion_options(&function.options);
            quote! { LootFunction::SetRandomPotion { options: #options } }
        }
        "minecraft:set_random_dyes" => {
            let number_of_dyes = function.number_of_dyes.as_ref().map_or_else(
                || panic!("minecraft:set_random_dyes is missing number_of_dyes"),
                generate_number_provider,
            );
            quote! { LootFunction::SetRandomDyes { number_of_dyes: #number_of_dyes } }
        }
        "minecraft:set_stew_effect" => {
            let effects: Vec<TokenStream> = function.effects.as_ref().map_or_default(|effs| {
                effs.iter()
                    .map(|e| {
                        let effect_type = e
                            .effect_type
                            .strip_prefix("minecraft:")
                            .unwrap_or(&e.effect_type);
                        let duration = generate_number_provider(&e.duration);

                        quote! {
                            StewEffect {
                                effect_type: Identifier::vanilla_static(#effect_type),
                                duration: #duration,
                            }
                        }
                    })
                    .collect()
            });

            quote! { LootFunction::SetStewEffect { effects: &[#(#effects),*] } }
        }
        "minecraft:set_instrument" => {
            let options = generate_instrument_options(&function.options);
            quote! { LootFunction::SetInstrument { options: #options } }
        }
        "minecraft:set_enchantments" => {
            let enchantments: Vec<TokenStream> =
                function.enchantments.as_ref().map_or_default(|enc| {
                    enc.iter()
                        .map(|(name, level)| {
                            let name = name.strip_prefix("minecraft:").unwrap_or(name);
                            let level = generate_number_provider(level);
                            quote! { (Identifier::vanilla_static(#name), #level) }
                        })
                        .collect()
                });
            let add = function.add;
            quote! {
                LootFunction::SetEnchantments {
                    enchantments: &[#(#enchantments),*],
                    add: #add,
                }
            }
        }
        "minecraft:discard" => {
            quote! { LootFunction::Discard }
        }
        "minecraft:filtered" => {
            let item_filter = function
                .item_filter
                .as_ref()
                .unwrap_or_else(|| panic!("minecraft:filtered function is missing item_filter"));
            let item_filter = generate_item_predicate(item_filter);
            let on_pass = generate_optional_nested_function(function.on_pass.as_deref());
            let on_fail = generate_optional_nested_function(function.on_fail.as_deref());
            quote! {
                LootFunction::Filtered {
                    item_filter: #item_filter,
                    on_pass: #on_pass,
                    on_fail: #on_fail,
                }
            }
        }
        other => {
            panic!("Unknown loot function type: {other}");
        }
    };

    // Wrap the function with its own condition
    let conditions: Vec<TokenStream> = function.condition.iter().map(generate_condition).collect();

    quote! {
        ConditionalLootFunction {
            function: #func_body,
            conditions: &[#(#conditions),*],
        }
    }
}

/// Generates an `ItemPredicate` (vanilla `net.minecraft.advancements.predicates.ItemPredicate`)
/// for `minecraft:filtered`'s `item_filter`.
fn generate_item_predicate(filter: &ItemFilterJson) -> TokenStream {
    let items = match &filter.items {
        Some(ItemSetJson::One(value)) => generate_item_holder_set(std::slice::from_ref(value)),
        Some(ItemSetJson::Many(values)) => generate_item_holder_set(values),
        None => quote! { None },
    };

    let (count_min, count_max) = match &filter.count {
        Some(LimitJson::Integer(v)) => (Some(*v), Some(*v)),
        Some(LimitJson::Object { min, max }) => (min.map(|v| v as i32), max.map(|v| v as i32)),
        None => (None, None),
    };
    let count_min = count_min.map_or_else(|| quote! { None }, |v| quote! { Some(#v) });
    let count_max = count_max.map_or_else(|| quote! { None }, |v| quote! { Some(#v) });

    let has_components: Vec<TokenStream> = filter
        .predicates
        .iter()
        .filter(|(key, _)| enchantment_source(key).is_none())
        .map(|(key, value)| {
            assert!(
                is_presence_check(value),
                "unsupported item_filter predicate shape for {key}: {value} \
                 (only the presence-check `{{}}`/`[{{}}]` shapes are modeled)"
            );
            let id = Identifier::from_str(key).unwrap_or_else(|error| {
                panic!("invalid item_filter predicate key {key:?}: {error}")
            });
            assert_eq!(
                id.namespace.as_ref(),
                "minecraft",
                "item_filter predicate keys must use the minecraft namespace: {id}"
            );
            let path = id.path.as_ref();
            quote! { Identifier::vanilla_static(#path) }
        })
        .collect();

    let enchantment_matches: Vec<TokenStream> = filter
        .predicates
        .iter()
        .filter(|(key, _)| enchantment_source(key).is_some())
        .flat_map(|(key, value)| {
            let source = match enchantment_source(key) {
                Some(EnchantmentSourceJson::Enchantments) => {
                    quote! { EnchantmentSource::Enchantments }
                }
                Some(EnchantmentSourceJson::StoredEnchantments) => {
                    quote! { EnchantmentSource::StoredEnchantments }
                }
                None => unreachable!("filtered by enchantment_source"),
            };
            let entries: Vec<EnchantmentPredicateJson> = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| {
                    panic!("invalid item_filter enchantment predicate {key}: {error}")
                });
            assert!(
                !entries.is_empty(),
                "item_filter enchantment predicate {key} must list at least one entry"
            );
            entries.into_iter().map(move |entry| {
                assert!(
                    entry.enchantments.is_none(),
                    "item_filter enchantment holder-set restrictions are not modeled ({key})"
                );
                let (min_level, max_level) = entry.level_bounds();
                let min_level = min_level.map_or_else(|| quote! { None }, |v| quote! { Some(#v) });
                let max_level = max_level.map_or_else(|| quote! { None }, |v| quote! { Some(#v) });
                let source = source.clone();
                quote! {
                    EnchantmentMatch {
                        component: #source,
                        min_level: #min_level,
                        max_level: #max_level,
                    }
                }
            })
        })
        .collect();

    quote! {
        ItemPredicate {
            items: #items,
            count_min: #count_min,
            count_max: #count_max,
            has_components: &[#(#has_components),*],
            enchantment_matches: &[#(#enchantment_matches),*],
        }
    }
}

/// Vanilla enchantment component predicates, keyed by their predicate id.
fn enchantment_source(key: &str) -> Option<EnchantmentSourceJson> {
    match key {
        "minecraft:enchantments" => Some(EnchantmentSourceJson::Enchantments),
        "minecraft:stored_enchantments" => Some(EnchantmentSourceJson::StoredEnchantments),
        _ => None,
    }
}

/// Vanilla's `AnyValueType` predicate shape: an empty object, or a list of empty
/// objects (e.g. `stored_enchantments: [{}]`), both meaning "component present".
fn is_presence_check(value: &serde_json::Value) -> bool {
    if let Some(object) = value.as_object() {
        return object.is_empty();
    }
    value.as_array().is_some_and(|entries| {
        !entries.is_empty()
            && entries
                .iter()
                .all(|entry| entry.as_object().is_some_and(serde_json::Map::is_empty))
    })
}

/// Generates an `Option<ItemSetPredicate>` for `item_filter.items`.
fn generate_item_holder_set(values: &[String]) -> TokenStream {
    if let [single] = values
        && let Some(tag) = single.strip_prefix('#')
    {
        let tag = tag.strip_prefix("minecraft:").unwrap_or(tag);
        let tag = Identifier::from_str(tag)
            .unwrap_or_else(|error| panic!("invalid item tag {single:?}: {error}"));
        let path = tag.path.as_ref();
        assert_eq!(
            tag.namespace.as_ref(),
            "minecraft",
            "item_filter tags must use the minecraft namespace: {tag}"
        );
        return quote! { Some(ItemSetPredicate::Tag(Identifier::vanilla_static(#path))) };
    }

    let items: Vec<TokenStream> = values
        .iter()
        .map(|value| {
            let id = Identifier::from_str(value)
                .unwrap_or_else(|error| panic!("invalid item id {value:?}: {error}"));
            assert_eq!(
                id.namespace.as_ref(),
                "minecraft",
                "item_filter items must use the minecraft namespace: {id}"
            );
            let path = id.path.as_ref();
            quote! { #path }
        })
        .collect();
    quote! { Some(ItemSetPredicate::Items(&[#(#items),*])) }
}

/// Generates an `Option<&'static ConditionalLootFunction>` for `on_pass`/`on_fail`.
fn generate_optional_nested_function(function: Option<&LootFunctionJson>) -> TokenStream {
    function.map_or_else(
        || quote! { None },
        |f| {
            let inner = generate_function(f);
            quote! { Some(&#inner) }
        },
    )
}
