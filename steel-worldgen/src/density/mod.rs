//! Density function types and transpiler for world generation.
//!
//! Density functions form a tree structure parsed from JSON at build time.
//! The transpiler compiles these trees into native Rust code — runtime evaluation
//! is done by the transpiled output, not by interpreting this tree.
//!
//! # Key Types
//!
//! - [`DensityFunction`] - The density function enum with all operation types
//! - [`NoiseRouter`] - Collection of all density functions for world generation
//! - [`CubicSpline`] - Cubic spline interpolation for smooth terrain transitions
//! - [`RarityValueMapper`] - Used at runtime by transpiled cave generation code
//! - [`DimensionNoises`] - Trait for dimension-specific noise generators
//! - [`NoiseSettings`] - Trait for dimension-specific settings from datapack

use crate::noise::NormalNoise;
use crate::random::RandomSplitter;
use steel_utils::Identifier;

pub mod spline_eval;
pub mod traits;

pub use traits::{ColumnCache, DimensionNoises, NoiseSettings};

/// Vanilla `noise_settings` registry entries.
///
/// Each key corresponds to one generated module under
/// [`crate::density_functions`], e.g. [`NoiseSettingsKey::LargeBiomes`] provides
/// `density_functions::large_biomes::{LargeBiomesNoises, LargeBiomesNoiseSettings}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NoiseSettingsKey {
    /// `minecraft:overworld` — the default overworld generator.
    Overworld,
    /// `minecraft:nether` — the nether generator.
    Nether,
    /// `minecraft:end` — the end generator.
    End,
    /// `minecraft:large_biomes` — overworld with large biomes.
    LargeBiomes,
    /// `minecraft:amplified` — overworld with amplified terrain.
    Amplified,
    /// `minecraft:caves` — cave generation test dimension.
    Caves,
    /// `minecraft:floating_islands` — floating islands test dimension.
    FloatingIslands,
}

impl NoiseSettingsKey {
    /// All keys, in vanilla registry order.
    pub const ALL: [Self; 7] = [
        Self::Overworld,
        Self::Nether,
        Self::End,
        Self::LargeBiomes,
        Self::Amplified,
        Self::Caves,
        Self::FloatingIslands,
    ];

    /// Resolves a `noise_settings` registry identifier to its key.
    ///
    /// Returns `None` for identifiers outside the `minecraft` namespace or for
    /// vanilla settings Steel does not transpile.
    #[must_use]
    pub fn from_identifier(key: &Identifier) -> Option<Self> {
        if key.namespace.as_ref() != Identifier::VANILLA_NAMESPACE {
            return None;
        }
        match key.path.as_ref() {
            "overworld" => Some(Self::Overworld),
            "nether" => Some(Self::Nether),
            "end" => Some(Self::End),
            "large_biomes" => Some(Self::LargeBiomes),
            "amplified" => Some(Self::Amplified),
            "caves" => Some(Self::Caves),
            "floating_islands" => Some(Self::FloatingIslands),
            _ => None,
        }
    }

    /// The `noise_settings` registry identifier for this key.
    #[must_use]
    pub const fn identifier(self) -> Identifier {
        Identifier::vanilla_static(self.path())
    }

    /// The datapack path of this key (`overworld`, `large_biomes`, ...).
    const fn path(self) -> &'static str {
        match self {
            Self::Overworld => "overworld",
            Self::Nether => "nether",
            Self::End => "end",
            Self::LargeBiomes => "large_biomes",
            Self::Amplified => "amplified",
            Self::Caves => "caves",
            Self::FloatingIslands => "floating_islands",
        }
    }
}

/// Parameters for creating a noise generator.
///
/// Mirrors vanilla's `NormalNoise.Parameters` codec, used directly by datapack
/// `worldgen/noise/*.json` entries.
#[derive(Debug, Clone)]
pub struct NoiseParameters {
    /// Amplitude at the base octave, before persistence falloff.
    pub base_amplitude: f64,
    /// The first (lowest-frequency) octave level.
    pub base_octave: i32,
    /// Number of octaves.
    pub octave_count: i32,
    /// Whether to apply persistence normalization (`Normalization.ENABLED`).
    pub normalize: bool,
    /// Per-octave amplitude multipliers. Empty means "all `1.0`".
    pub amplitude_modifiers: Vec<f64>,
}

impl NoiseParameters {
    /// Create new noise parameters.
    #[must_use]
    pub const fn new(
        base_amplitude: f64,
        base_octave: i32,
        octave_count: i32,
        normalize: bool,
        amplitude_modifiers: Vec<f64>,
    ) -> Self {
        Self {
            base_amplitude,
            base_octave,
            octave_count,
            normalize,
            amplitude_modifiers,
        }
    }

    /// Create a [`NormalNoise`] generator from these parameters, seeded from `splitter`
    /// under the given noise `id`.
    #[must_use]
    pub fn create(&self, splitter: &RandomSplitter, id: &str) -> NormalNoise {
        NormalNoise::create_with_params(
            splitter,
            id,
            self.base_octave,
            self.base_amplitude,
            self.octave_count,
            self.normalize,
            &self.amplitude_modifiers,
        )
    }
}

/// Rarity value mapper for cave generation.
///
/// Used at runtime by transpiled `WeirdScaledSampler` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RarityValueMapper {
    /// Mapper type `"type_1"` for tunnels.
    Tunnels,
    /// Mapper type `"type_2"` for caves.
    Caves,
}

impl RarityValueMapper {
    /// Get the scaling factor for this mapper based on rarity value.
    ///
    /// From vanilla `NoiseRouterData.QuantizedSpaghettiRarity`.
    #[must_use]
    pub fn get_values(self, rarity: f32) -> f32 {
        match self {
            Self::Tunnels => {
                if rarity < -0.5 {
                    0.75
                } else if rarity < 0.0 {
                    1.0
                } else if rarity < 0.5 {
                    1.5
                } else {
                    2.0
                }
            }
            Self::Caves => {
                if rarity < -0.75 {
                    0.5
                } else if rarity < -0.5 {
                    0.75
                } else if rarity < 0.5 {
                    1.0
                } else if rarity < 0.75 {
                    2.0
                } else {
                    3.0
                }
            }
        }
    }
}
