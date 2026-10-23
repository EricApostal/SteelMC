//! Build script for generating vanilla `trial_spawner` config definitions.

#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted trial spawner data"
)]

use std::{
    fs,
    path::{Path, PathBuf},
};

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use steel_utils::Identifier;

/// A weighted entry as stored in vanilla `WeightedList` JSON.
#[derive(Deserialize, Debug)]
struct WeightedJson<T> {
    data: T,
    weight: i32,
}

/// Vanilla `TrialSpawnerConfig` JSON. Field defaults mirror `TrialSpawnerConfig.Builder`.
#[derive(Deserialize, Debug)]
struct TrialSpawnerJson {
    #[serde(default = "default_spawn_range")]
    spawn_range: i32,
    #[serde(default = "default_total_mobs")]
    total_mobs: f32,
    #[serde(default = "default_simultaneous_mobs")]
    simultaneous_mobs: f32,
    #[serde(default = "default_total_mobs_added_per_player")]
    total_mobs_added_per_player: f32,
    #[serde(default = "default_simultaneous_mobs_added_per_player")]
    simultaneous_mobs_added_per_player: f32,
    #[serde(default = "default_ticks_between_spawn")]
    ticks_between_spawn: i32,
    #[serde(default)]
    spawn_potentials: Vec<WeightedJson<SpawnDataJson>>,
    #[serde(default = "default_loot_tables_to_eject")]
    loot_tables_to_eject: Vec<WeightedJson<Identifier>>,
    #[serde(default)]
    items_to_drop_when_ominous: Option<Identifier>,
}

/// Vanilla `SpawnData` JSON.
#[derive(Deserialize, Debug)]
struct SpawnDataJson {
    entity: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    custom_spawn_rules: Option<CustomSpawnRulesJson>,
    #[serde(default)]
    equipment: Option<EquipmentJson>,
}

/// Vanilla `SpawnData.CustomSpawnRules` JSON.
#[derive(Deserialize, Debug)]
struct CustomSpawnRulesJson {
    #[serde(default = "default_light_limit")]
    block_light_limit: LightLimitJson,
    #[serde(default = "default_light_limit")]
    sky_light_limit: LightLimitJson,
}

/// An `InclusiveRange<Integer>` accepts a single int, a `[min, max]` array, or an
/// object with explicit bounds.
#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(untagged)]
enum LightLimitJson {
    Value(i32),
    Range([i32; 2]),
    Explicit {
        min_inclusive: i32,
        max_inclusive: i32,
    },
}

/// Vanilla `EquipmentTable` JSON.
#[derive(Deserialize, Debug)]
struct EquipmentJson {
    loot_table: Identifier,
    #[serde(default)]
    slot_drop_chances: Option<SlotDropChancesJson>,
}

/// `slot_drop_chances` is either a uniform float or a slot-name map.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum SlotDropChancesJson {
    Uniform(f32),
    PerSlot(FxHashMap<String, f32>),
}

const fn default_spawn_range() -> i32 {
    4
}

const fn default_total_mobs() -> f32 {
    6.0
}

const fn default_simultaneous_mobs() -> f32 {
    2.0
}

const fn default_total_mobs_added_per_player() -> f32 {
    2.0
}

const fn default_simultaneous_mobs_added_per_player() -> f32 {
    1.0
}

const fn default_ticks_between_spawn() -> i32 {
    40
}

const fn default_light_limit() -> LightLimitJson {
    // Vanilla `CustomSpawnRules.LIGHT_RANGE`.
    LightLimitJson::Explicit {
        min_inclusive: 0,
        max_inclusive: 15,
    }
}

fn default_loot_tables_to_eject() -> Vec<WeightedJson<Identifier>> {
    // Vanilla `TrialSpawnerConfig.Builder` default weighted list.
    vec![
        WeightedJson {
            data: "minecraft:spawners/trial_chamber/consumables"
                .parse()
                .unwrap(),
            weight: 1,
        },
        WeightedJson {
            data: "minecraft:spawners/trial_chamber/key".parse().unwrap(),
            weight: 1,
        },
    ]
}

/// The equipment slots in vanilla `EquipmentSlot.values()` order.
const EQUIPMENT_SLOTS: [&str; 8] = [
    "mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle",
];

fn generate_equipment_slot(name: &str) -> TokenStream {
    match name {
        "mainhand" => quote! { EquipmentSlot::MainHand },
        "offhand" => quote! { EquipmentSlot::OffHand },
        "feet" => quote! { EquipmentSlot::Feet },
        "legs" => quote! { EquipmentSlot::Legs },
        "chest" => quote! { EquipmentSlot::Chest },
        "head" => quote! { EquipmentSlot::Head },
        "body" => quote! { EquipmentSlot::Body },
        "saddle" => quote! { EquipmentSlot::Saddle },
        other => panic!("Unknown equipment slot in trial spawner data: {other}"),
    }
}

fn generate_identifier(identifier: &Identifier) -> TokenStream {
    let namespace = identifier.namespace.as_ref();
    let path = identifier.path.as_ref();
    quote! { Identifier::new_static(#namespace, #path) }
}

fn generate_light_limit(limit: &LightLimitJson) -> TokenStream {
    let (min, max) = match limit {
        LightLimitJson::Value(value) => (*value, *value),
        LightLimitJson::Range([min, max]) => (*min, *max),
        LightLimitJson::Explicit {
            min_inclusive,
            max_inclusive,
        } => (*min_inclusive, *max_inclusive),
    };
    quote! {
        InclusiveRange {
            min_inclusive: #min,
            max_inclusive: #max,
        }
    }
}

fn generate_custom_spawn_rules(rules: &CustomSpawnRulesJson) -> TokenStream {
    let block = generate_light_limit(&rules.block_light_limit);
    let sky = generate_light_limit(&rules.sky_light_limit);
    quote! {
        CustomSpawnRules {
            block_light_limit: #block,
            sky_light_limit: #sky,
        }
    }
}

fn generate_slot_drop_chances(chances: &SlotDropChancesJson) -> Vec<TokenStream> {
    match chances {
        SlotDropChancesJson::Uniform(chance) => EQUIPMENT_SLOTS
            .iter()
            .map(|slot| {
                let slot = generate_equipment_slot(slot);
                quote! { (#slot, #chance) }
            })
            .collect(),
        SlotDropChancesJson::PerSlot(map) => {
            let mut entries: Vec<(&String, &f32)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            entries
                .into_iter()
                .map(|(slot, chance)| {
                    let slot = generate_equipment_slot(slot);
                    quote! { (#slot, #chance) }
                })
                .collect()
        }
    }
}

fn generate_equipment(equipment: &EquipmentJson) -> TokenStream {
    let loot_table = generate_identifier(&equipment.loot_table);
    let chances = equipment
        .slot_drop_chances
        .as_ref()
        .map_or_else(Vec::new, generate_slot_drop_chances);
    quote! {
        EquipmentTable {
            loot_table: #loot_table,
            slot_drop_chances: vec![#(#chances),*],
        }
    }
}

fn generate_spawn_data(data: &SpawnDataJson) -> TokenStream {
    let entity_nbt = serde_json::to_string(&data.entity).unwrap();
    let entity_type = data
        .entity
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || panic!("Spawn data entity is missing an `id` key: {entity_nbt}"),
            |id| id.parse::<Identifier>().unwrap(),
        );
    let entity_type = generate_identifier(&entity_type);

    let custom_spawn_rules = data.custom_spawn_rules.as_ref().map_or_else(
        || quote! { None },
        |rules| {
            let rules = generate_custom_spawn_rules(rules);
            quote! { Some(#rules) }
        },
    );
    let equipment = data.equipment.as_ref().map_or_else(
        || quote! { None },
        |equipment| {
            let equipment = generate_equipment(equipment);
            quote! { Some(#equipment) }
        },
    );

    quote! {
        SpawnData {
            entity_type: #entity_type,
            entity_nbt: #entity_nbt,
            custom_spawn_rules: #custom_spawn_rules,
            equipment: #equipment,
        }
    }
}

/// Recursively collects `(key, path)` pairs for every JSON file under `dir`.
fn collect_json_files(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_json_files(&path, base, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
            let key = path
                .strip_prefix(base)
                .unwrap()
                .with_extension("")
                .to_str()
                .unwrap()
                .replace('\\', "/");
            out.push((key, path));
        }
    }
}

pub(crate) fn build() -> TokenStream {
    let trial_spawner_dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/trial_spawner";
    println!("cargo:rerun-if-changed={trial_spawner_dir}");

    let base = Path::new(trial_spawner_dir);
    let mut files = Vec::new();
    collect_json_files(base, base, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut stream = TokenStream::new();
    let mut register_stream = TokenStream::new();

    for (key, path) in &files {
        let content = fs::read_to_string(path).unwrap();
        let config: TrialSpawnerJson =
            serde_json::from_str(&content).unwrap_or_else(|e| panic!("Failed to parse {key}: {e}"));

        let ident = Ident::new(
            &key.replace('/', "_").to_shouty_snake_case(),
            Span::call_site(),
        );

        let spawn_range = config.spawn_range;
        let total_mobs = config.total_mobs;
        let simultaneous_mobs = config.simultaneous_mobs;
        let total_mobs_added_per_player = config.total_mobs_added_per_player;
        let simultaneous_mobs_added_per_player = config.simultaneous_mobs_added_per_player;
        let ticks_between_spawn = config.ticks_between_spawn;

        let spawn_potentials = config.spawn_potentials.iter().map(|entry| {
            let data = generate_spawn_data(&entry.data);
            let weight = entry.weight;
            quote! {
                Weighted {
                    data: #data,
                    weight: #weight,
                }
            }
        });
        let loot_tables_to_eject = config.loot_tables_to_eject.iter().map(|entry| {
            let data = generate_identifier(&entry.data);
            let weight = entry.weight;
            quote! {
                Weighted {
                    data: #data,
                    weight: #weight,
                }
            }
        });
        let items_to_drop_when_ominous = config.items_to_drop_when_ominous.as_ref().map_or_else(
            || quote! { None },
            |identifier| {
                let identifier = generate_identifier(identifier);
                quote! { Some(#identifier) }
            },
        );

        stream.extend(quote! {
            pub static #ident: LazyLock<TrialSpawnerConfig> = LazyLock::new(|| TrialSpawnerConfig {
                key: Identifier::vanilla_static(#key),
                spawn_range: #spawn_range,
                total_mobs: #total_mobs,
                simultaneous_mobs: #simultaneous_mobs,
                total_mobs_added_per_player: #total_mobs_added_per_player,
                simultaneous_mobs_added_per_player: #simultaneous_mobs_added_per_player,
                ticks_between_spawn: #ticks_between_spawn,
                spawn_potentials: vec![#(#spawn_potentials),*],
                loot_tables_to_eject: vec![#(#loot_tables_to_eject),*],
                items_to_drop_when_ominous: #items_to_drop_when_ominous,
            });
        });

        register_stream.extend(quote! {
            registry.register(&#ident);
        });
    }

    let mut out = TokenStream::new();
    out.extend(quote! {
        use std::sync::LazyLock;

        use crate::equipment::EquipmentSlot;
        use crate::trial_spawner::{
            CustomSpawnRules, EquipmentTable, InclusiveRange, SpawnData, TrialSpawnerConfig,
            TrialSpawnerConfigRegistry, Weighted,
        };
        use steel_utils::Identifier;

        #stream

        pub fn register_trial_spawner_configs(registry: &mut TrialSpawnerConfigRegistry) {
            #register_stream
        }
    });
    out
}
