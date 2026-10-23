use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::equipment::EquipmentSlot;

/// A weighted entry in a vanilla `WeightedList`.
///
/// Mirrors `net.minecraft.util.random.WeightedList.Weighted`; the vanilla list is
/// stored as a plain `Vec` since Steel selects entries by summing weights on demand.
#[derive(Debug)]
pub struct Weighted<T> {
    pub data: T,
    pub weight: i32,
}

/// Vanilla `TrialSpawnerConfig`, loaded from `trial_spawner/<key>.json`.
#[derive(Debug)]
pub struct TrialSpawnerConfig {
    pub key: Identifier,
    pub spawn_range: i32,
    pub total_mobs: f32,
    pub simultaneous_mobs: f32,
    pub total_mobs_added_per_player: f32,
    pub simultaneous_mobs_added_per_player: f32,
    pub ticks_between_spawn: i32,
    pub spawn_potentials: Vec<Weighted<SpawnData>>,
    pub loot_tables_to_eject: Vec<Weighted<Identifier>>,
    /// Vanilla defaults this to the `spawners/trial_chamber/items_to_drop_when_ominous`
    /// loot table when the field is absent; no vendored entry sets it, so it is `None`
    /// until the field is actually present in the JSON.
    pub items_to_drop_when_ominous: Option<Identifier>,
}

/// One `spawn_potentials` entry: the entity to spawn plus optional spawn rules and
/// equipment.
#[derive(Debug)]
pub struct SpawnData {
    /// Entity type taken from the `id` key of `entity_nbt`.
    pub entity_type: Identifier,
    /// The vanilla `entity` compound, stored as compact JSON.
    ///
    /// Steel has no build-time NBT literal emitter, so the extracted compound is kept
    /// as a JSON string and replayed through the entity-load path at runtime.
    pub entity_nbt: &'static str,
    pub custom_spawn_rules: Option<CustomSpawnRules>,
    pub equipment: Option<EquipmentTable>,
}

/// Vanilla `SpawnData.CustomSpawnRules`: inclusive light limits controlling a valid
/// spawn position.
#[derive(Debug)]
pub struct CustomSpawnRules {
    pub block_light_limit: InclusiveRange,
    pub sky_light_limit: InclusiveRange,
}

/// Vanilla `net.minecraft.util.InclusiveRange<Integer>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InclusiveRange {
    pub min_inclusive: i32,
    pub max_inclusive: i32,
}

/// Vanilla `EquipmentTable`: a loot table rolled for the spawned entity's equipment,
/// plus the per-slot drop chance (expanded from the JSON's uniform-float shorthand).
#[derive(Debug)]
pub struct EquipmentTable {
    pub loot_table: Identifier,
    pub slot_drop_chances: Vec<(EquipmentSlot, f32)>,
}

pub type TrialSpawnerConfigRef = &'static TrialSpawnerConfig;

pub struct TrialSpawnerConfigRegistry {
    trial_spawner_configs_by_id: Vec<TrialSpawnerConfigRef>,
    trial_spawner_configs_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl TrialSpawnerConfigRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            trial_spawner_configs_by_id: Vec::new(),
            trial_spawner_configs_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    TrialSpawnerConfigRegistry,
    TrialSpawnerConfigRef,
    trial_spawner_configs_by_id,
    trial_spawner_configs_by_key,
    allows_registering
);

crate::impl_registry!(
    TrialSpawnerConfigRegistry,
    TrialSpawnerConfig,
    trial_spawner_configs_by_id,
    trial_spawner_configs_by_key,
    trial_spawner_configs
);
