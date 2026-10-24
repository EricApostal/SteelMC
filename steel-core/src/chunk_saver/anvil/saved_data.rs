//! Vanilla SavedData files (`data/<id>.dat`) ⇄ Steel saved-data shapes.
//!
//! Vanilla writes every SavedData as gzip NBT `{ data: <codec payload>, DataVersion: <int> }`
//! (`LevelStorageSource.writeSavedData`). This module implements the wrapper plus the payload
//! conversions for the files whose field names already match Steel's:
//!
//! * `raids` — `Raids.CODEC` (field-for-field identical to Steel's `RaidsData`).
//! * `weather` — `WeatherData.CODEC` (identical to Steel's `WeatherState`).
//! * `wandering_trader` — `WanderingTraderData.CODEC`.
//! * `game_rules` — the world-level game-rule map.
//! * `world_gen_settings` — only the `seed` field is read; the rest is vanilla world-gen data
//!   Steel does not model.

use std::io;
use std::path::Path;

use rustc_hash::FxHashMap;
use serde_json::Value as JsonValue;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::uuid_ext::UuidExt as _;
use steel_utils::version::DATA_VERSION;
use uuid::Uuid;

use crate::entity::raid::{RaidRecord, RaidsData};
use crate::level_data::WeatherState;

use super::nbt::{read_compressed_nbt, write_gzip_nbt};

/// Reads a vanilla `data/*.dat` file and returns its inner `data` compound.
///
/// Returns `Ok(None)` when the file does not exist.
///
/// # Errors
/// Returns `InvalidData` when the file is not gzip NBT or has no `data` compound.
pub(crate) fn read_saved_data_file(path: &Path) -> io::Result<Option<NbtCompound>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let root = read_compressed_nbt(&bytes)?;
    let Some(NbtTag::Compound(data)) = root.get("data") else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} has no `data` compound", path.display()),
        ));
    };
    // The `DataVersion` tag lives next to `data` on the wrapper root, so the gate reads the
    // root, not the inner payload.
    crate::datafixer::ensure_current(
        crate::datafixer::version_of(&root),
        crate::datafixer::DataFixType::SavedData,
        &format!("saved data `{}`", path.display()),
    )?;
    Ok(Some(data.clone()))
}

/// Writes a vanilla `data/*.dat` file, creating parent directories.
///
/// # Errors
/// Propagates filesystem errors.
pub(crate) fn write_saved_data_file(path: &Path, data: &NbtCompound) -> io::Result<()> {
    let mut root = NbtCompound::new();
    root.insert("data", NbtTag::Compound(data.clone()));
    root.insert("DataVersion", DATA_VERSION);
    let bytes = write_gzip_nbt(&root)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("dat.tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path)
}

/// Serializes Steel's raid saved data with vanilla's `Raids.CODEC` field names.
#[must_use]
pub(crate) fn raids_to_nbt(data: &RaidsData) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.insert(
        "raids",
        NbtList::Compound(data.raids.iter().map(raid_to_nbt).collect()),
    );
    nbt.insert("next_id", data.next_id);
    nbt.insert("tick", data.tick);
    nbt
}

/// Parses vanilla's `Raids.CODEC` payload.
#[must_use]
pub(crate) fn raids_from_nbt(nbt: &NbtCompound) -> RaidsData {
    let raids = nbt
        .list("raids")
        .and_then(NbtList::compounds)
        .map(|raids| raids.iter().map(raid_from_nbt).collect())
        .unwrap_or_default();
    RaidsData {
        raids,
        next_id: nbt.int("next_id").unwrap_or(0),
        tick: nbt.int("tick").unwrap_or(0),
    }
}

fn raid_to_nbt(raid: &RaidRecord) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.insert("id", raid.id);
    nbt.insert("started", NbtTag::Byte(i8::from(raid.started)));
    nbt.insert("active", NbtTag::Byte(i8::from(raid.active)));
    nbt.insert("ticks_active", raid.ticks_active);
    nbt.insert("raid_omen_level", raid.raid_omen_level);
    nbt.insert("groups_spawned", raid.groups_spawned);
    nbt.insert("cooldown_ticks", raid.raid_cooldown_ticks);
    nbt.insert("post_raid_ticks", raid.post_raid_ticks);
    nbt.insert("total_health", raid.total_health);
    nbt.insert("group_count", raid.group_count);
    nbt.insert("status", raid.status.clone());
    nbt.insert("center", NbtTag::IntArray(raid.center.to_vec()));
    nbt.insert(
        "heroes_of_the_village",
        NbtList::IntArray(
            raid.heroes_of_the_village
                .iter()
                .map(|uuid| uuid.to_int_array().to_vec())
                .collect(),
        ),
    );
    nbt
}

fn raid_from_nbt(nbt: &NbtCompound) -> RaidRecord {
    let center = nbt
        .int_array("center")
        .filter(|center| center.len() >= 3)
        .map_or([0; 3], |center| [center[0], center[1], center[2]]);
    RaidRecord {
        id: nbt.int("id").unwrap_or(0),
        started: nbt.byte("started").is_some_and(|value| value != 0),
        active: nbt.byte("active").is_some_and(|value| value != 0),
        ticks_active: nbt.long("ticks_active").unwrap_or(0),
        raid_omen_level: nbt.int("raid_omen_level").unwrap_or(0),
        groups_spawned: nbt.int("groups_spawned").unwrap_or(0),
        raid_cooldown_ticks: nbt.int("cooldown_ticks").unwrap_or(0),
        post_raid_ticks: nbt.int("post_raid_ticks").unwrap_or(0),
        total_health: nbt.float("total_health").unwrap_or(0.0),
        group_count: nbt.int("group_count").unwrap_or(0),
        status: nbt
            .string("status")
            .map_or_else(|| "ongoing".to_owned(), |value| value.to_str().into_owned()),
        center,
        heroes_of_the_village: match nbt.get("heroes_of_the_village") {
            Some(NbtTag::List(NbtList::IntArray(arrays))) => arrays
                .iter()
                .filter_map(|array| Uuid::from_int_array(array))
                .collect(),
            _ => Vec::new(),
        },
    }
}

/// Serializes Steel's weather state with vanilla's `WeatherData.CODEC` field names.
#[must_use]
pub(crate) fn weather_to_nbt(weather: &WeatherState) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.insert("clear_weather_time", weather.clear_weather_time);
    nbt.insert("rain_time", weather.rain_time);
    nbt.insert("thunder_time", weather.thunder_time);
    nbt.insert("raining", NbtTag::Byte(i8::from(weather.raining)));
    nbt.insert("thundering", NbtTag::Byte(i8::from(weather.thundering)));
    nbt
}

/// Parses vanilla's `WeatherData.CODEC` payload.
#[must_use]
pub(crate) fn weather_from_nbt(nbt: &NbtCompound) -> WeatherState {
    WeatherState {
        raining: nbt.byte("raining").is_some_and(|value| value != 0),
        rain_time: nbt.int("rain_time").unwrap_or(0),
        thundering: nbt.byte("thundering").is_some_and(|value| value != 0),
        thunder_time: nbt.int("thunder_time").unwrap_or(0),
        clear_weather_time: nbt.int("clear_weather_time").unwrap_or(0),
    }
}

/// Serializes the wandering-trader saved data.
#[must_use]
pub(crate) fn wandering_trader_to_nbt(spawn_delay: i32, spawn_chance: i32) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    nbt.insert("spawn_delay", spawn_delay);
    nbt.insert("spawn_chance", spawn_chance);
    nbt
}

/// Parses the wandering-trader saved data, returning `(spawn_delay, spawn_chance)`.
#[must_use]
pub(crate) fn wandering_trader_from_nbt(nbt: &NbtCompound) -> (i32, i32) {
    (
        nbt.int("spawn_delay").unwrap_or(24_000),
        nbt.int("spawn_chance").unwrap_or(25),
    )
}

/// Serializes a game-rule map. Vanilla's `GameRuleMap` stores every value as a string.
#[must_use]
pub(crate) fn game_rules_to_nbt(rules: &FxHashMap<String, JsonValue>) -> NbtCompound {
    let mut nbt = NbtCompound::new();
    for (name, value) in rules {
        let text = match value {
            JsonValue::String(text) => text.clone(),
            other => other.to_string(),
        };
        nbt.insert(name.as_str(), text);
    }
    nbt
}

/// Parses a game-rule map, accepting string, boolean, and numeric values.
#[must_use]
pub(crate) fn game_rules_from_nbt(nbt: &NbtCompound) -> FxHashMap<String, JsonValue> {
    let mut rules = FxHashMap::default();
    for (name, tag) in nbt.iter() {
        let value = match tag {
            NbtTag::String(text) => {
                let text = text.to_string();
                if text == "true" || text == "false" {
                    JsonValue::Bool(text == "true")
                } else if let Ok(number) = text.parse::<i64>() {
                    JsonValue::from(number)
                } else {
                    JsonValue::String(text)
                }
            }
            NbtTag::Byte(value) => JsonValue::Bool(*value != 0),
            NbtTag::Int(value) => JsonValue::from(*value),
            NbtTag::Long(value) => JsonValue::from(*value),
            NbtTag::Float(value) => JsonValue::from(f64::from(*value)),
            NbtTag::Double(value) => JsonValue::from(*value),
            _ => continue,
        };
        rules.insert(name.to_str().into_owned(), value);
    }
    rules
}

/// Reads the generation seed out of a vanilla `world_gen_settings` payload.
///
/// # Errors
/// Returns `InvalidData` when the payload is not a valid NBT compound.
pub(crate) fn read_world_gen_seed(path: &Path) -> io::Result<Option<i64>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let root = read_compressed_nbt(&bytes)?;
    let data = match root.get("data") {
        Some(NbtTag::Compound(data)) => data,
        _ => &root,
    };
    if let Some(seed) = data.long("seed") {
        return Ok(Some(seed));
    }
    Ok(None)
}
