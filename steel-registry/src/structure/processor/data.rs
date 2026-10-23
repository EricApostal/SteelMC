//! Typed structure processor-list codec data.

use serde::{Deserialize, Deserializer, de::Error as _};
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_utils::{Identifier, value_providers::IntProvider};

use crate::shared_structs::BlockStateData;

/// Codec payload for a structure processor list.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructureProcessorListData {
    /// Ordered processors.
    pub processors: Vec<StructureProcessorKind>,
}

/// A typed vanilla structure processor.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "processor_type")]
pub enum StructureProcessorKind {
    /// Randomly drops input blocks.
    #[serde(rename = "minecraft:block_rot")]
    BlockRot {
        /// Optional block set restricting which blocks may be dropped.
        #[serde(default)]
        rottable_blocks: Option<StructureProcessorBlockSet>,
        /// Keep probability.
        integrity: f32,
    },
    /// Drops blocks from the structure when their block is a member of `blocks`.
    ///
    /// Mirrors vanilla `BlockIgnoreProcessor`: vanilla decodes each entry with
    /// `BlockState.CODEC.xmap(getBlock, defaultBlockState)`, so Steel keeps the
    /// block-state payloads and compares by block at placement time.
    #[serde(rename = "minecraft:block_ignore")]
    BlockIgnore {
        /// Block-state payloads; member blocks are ignored.
        blocks: Vec<BlockStateData>,
    },
    /// Shifts each placed block onto a heightmap column.
    ///
    /// Mirrors vanilla `GravityProcessor`: the block keeps its world X/Z and its
    /// template-relative Y offset, added to the column height plus `offset`.
    #[serde(rename = "minecraft:gravity")]
    Gravity {
        /// Heightmap column to rest on.
        #[serde(default = "default_gravity_heightmap")]
        heightmap: StructureProcessorHeightmap,
        /// Vertical offset added to the column height.
        #[serde(default)]
        offset: i32,
    },
    /// Identity processor.
    #[serde(rename = "minecraft:nop")]
    Nop,
    /// Replaces jigsaw blocks with the states their `final_state` tag names.
    #[serde(rename = "minecraft:jigsaw_replacement")]
    JigsawReplacement,
    /// Prevents replacement of protected world blocks.
    #[serde(rename = "minecraft:protected_blocks")]
    ProtectedBlocks {
        /// Vanilla field name is `value`; it stores the cannot-replace block set.
        #[serde(rename = "value")]
        cannot_replace: StructureProcessorBlockSet,
    },
    /// Applies the first matching rule.
    #[serde(rename = "minecraft:rule")]
    Rule { rules: Vec<ProcessorRuleData> },
    /// Ages stone/obsidian structure blocks, used by ruined portals.
    #[serde(rename = "minecraft:block_age")]
    BlockAge { mossiness: f32 },
    /// Keeps non-full structure blocks submerged in existing lava.
    #[serde(rename = "minecraft:lava_submerged_block")]
    LavaSubmergedBlock,
    /// Replaces stone ruin blocks with blackstone variants.
    #[serde(rename = "minecraft:blackstone_replace")]
    BlackstoneReplace,
    /// Delegates to another processor but caps successful modifications.
    #[serde(rename = "minecraft:capped")]
    Capped {
        delegate: Box<StructureProcessorKind>,
        limit: IntProvider,
    },
}

/// One rule inside vanilla's `RuleProcessor`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessorRuleData {
    pub input_predicate: StructureRuleTestData,
    pub location_predicate: StructureRuleTestData,
    #[serde(default)]
    pub position_predicate: PosRuleTestData,
    pub output_state: BlockStateData,
    #[serde(default)]
    pub block_entity_modifier: RuleBlockEntityModifierData,
}

/// Block-state rule tests used by `RuleProcessor`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "predicate_type")]
pub enum StructureRuleTestData {
    #[serde(rename = "minecraft:always_true")]
    AlwaysTrue,
    #[serde(rename = "minecraft:block_match")]
    BlockMatch { block: Identifier },
    #[serde(rename = "minecraft:random_block_match")]
    RandomBlockMatch { block: Identifier, probability: f32 },
    #[serde(rename = "minecraft:tag_match")]
    TagMatch { tag: Identifier },
    #[serde(rename = "minecraft:blockstate_match")]
    BlockStateMatch { block_state: BlockStateData },
    #[serde(rename = "minecraft:random_blockstate_match")]
    RandomBlockStateMatch {
        block_state: BlockStateData,
        probability: f32,
    },
    #[serde(rename = "minecraft:all_of")]
    AllOf { rules: Vec<StructureRuleTestData> },
    #[serde(rename = "minecraft:any_of")]
    AnyOf { rules: Vec<StructureRuleTestData> },
    #[serde(rename = "minecraft:not")]
    Not { rule: Box<StructureRuleTestData> },
    #[serde(rename = "minecraft:height_match")]
    HeightMatch {
        min_inclusive: i32,
        max_inclusive: i32,
    },
}

/// Position rule tests used by `RuleProcessor`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(tag = "predicate_type")]
pub enum PosRuleTestData {
    #[default]
    #[serde(rename = "minecraft:always_true")]
    AlwaysTrue,
    #[serde(rename = "minecraft:axis_aligned_linear_pos")]
    AxisAlignedLinearPos {
        #[serde(
            default = "default_structure_processor_axis",
            deserialize_with = "deserialize_processor_axis"
        )]
        axis: StructureProcessorAxis,
        #[serde(default)]
        min_chance: f32,
        #[serde(default)]
        max_chance: f32,
        #[serde(default)]
        min_dist: i32,
        #[serde(default)]
        max_dist: i32,
    },
    /// Distance-from-reference chance test.
    ///
    /// Mirrors vanilla `LinearPosTest`: `dist` is `worldPos.distManhattan(worldReference)`
    /// and the rule applies when a uniform sample is at most
    /// `clampedLerp(inverseLerp(dist, min_dist, max_dist), min_chance, max_chance)`.
    #[serde(rename = "minecraft:linear_pos")]
    LinearPos {
        /// Success chance at or below `min_dist`.
        #[serde(default)]
        min_chance: f32,
        /// Success chance at or above `max_dist`.
        #[serde(default)]
        max_chance: f32,
        /// Distance at or below which `min_chance` applies.
        #[serde(default)]
        min_dist: i32,
        /// Distance at or above which `max_chance` applies.
        #[serde(default)]
        max_dist: i32,
    },
}

/// Heightmap selector for `minecraft:gravity`.
///
/// Vanilla serializes the full `Heightmap.Types` enum by name; Steel only needs
/// the six worldgen heightmaps, mirrored here by serialization key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum StructureProcessorHeightmap {
    /// Tracks the highest non-air block (worldgen usage).
    #[serde(rename = "WORLD_SURFACE_WG")]
    WorldSurfaceWg,
    /// Tracks the highest non-air block (client usage).
    #[serde(rename = "WORLD_SURFACE")]
    WorldSurface,
    /// Tracks the highest solid block (worldgen usage).
    #[serde(rename = "OCEAN_FLOOR_WG")]
    OceanFloorWg,
    /// Tracks the highest solid block (live-world usage).
    #[serde(rename = "OCEAN_FLOOR")]
    OceanFloor,
    /// Tracks the highest motion-blocking block or fluid.
    #[serde(rename = "MOTION_BLOCKING")]
    MotionBlocking,
    /// Tracks the highest motion-blocking block or fluid that is not leaves.
    #[serde(rename = "MOTION_BLOCKING_NO_LEAVES")]
    MotionBlockingNoLeaves,
}

const fn default_gravity_heightmap() -> StructureProcessorHeightmap {
    StructureProcessorHeightmap::WorldSurfaceWg
}

/// Vanilla block holder set used by `minecraft:block_rot` and
/// `minecraft:protected_blocks`. Vanilla's `RegistryCodecs.holderSet` accepts a
/// `#tag`, a bare block id such as `minecraft:air`, or a list of block ids;
/// entries are resolved against the block registry at placement time.
#[derive(Debug, Clone)]
pub enum StructureProcessorBlockSet {
    /// A named block tag (`#namespace:path`).
    Tag(Identifier),
    /// Direct block ids.
    Entries(Vec<Identifier>),
}

impl<'de> Deserialize<'de> for StructureProcessorBlockSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Single(String),
            Many(Vec<Identifier>),
        }

        match Raw::deserialize(deserializer)? {
            Raw::Single(value) => match value.strip_prefix('#') {
                Some(tag) => tag
                    .parse::<Identifier>()
                    .map(Self::Tag)
                    .map_err(D::Error::custom),
                None => value
                    .parse::<Identifier>()
                    .map(|entry| Self::Entries(vec![entry]))
                    .map_err(D::Error::custom),
            },
            Raw::Many(values) => Ok(Self::Entries(values)),
        }
    }
}

/// Axis enum for position predicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureProcessorAxis {
    X,
    Y,
    Z,
}

const fn default_structure_processor_axis() -> StructureProcessorAxis {
    StructureProcessorAxis::Y
}

fn deserialize_processor_axis<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<StructureProcessorAxis, D::Error> {
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "x" => Ok(StructureProcessorAxis::X),
        "y" => Ok(StructureProcessorAxis::Y),
        "z" => Ok(StructureProcessorAxis::Z),
        _ => Err(D::Error::custom("invalid structure processor axis")),
    }
}

/// Rule block-entity NBT modifiers.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(tag = "type")]
pub enum RuleBlockEntityModifierData {
    /// Vanilla passthrough when the field is absent.
    #[default]
    #[serde(rename = "minecraft:passthrough")]
    Passthrough,
    /// Replaces any existing tag with an empty compound.
    #[serde(rename = "minecraft:clear")]
    Clear,
    /// Merges a static compound tag over the existing block-entity tag.
    #[serde(rename = "minecraft:append_static")]
    AppendStatic {
        /// Compound tag merged into the output block entity.
        #[serde(deserialize_with = "deserialize_compound_tag")]
        data: NbtCompound,
    },
    /// Appends loot table metadata to the output block entity.
    #[serde(rename = "minecraft:append_loot")]
    AppendLoot { loot_table: Identifier },
}

/// Decodes a vanilla `CompoundTag.CODEC` payload.
///
/// Vanilla round-trips the JSON payload through `NbtOps`; `json_to_nbt_tag`
/// reproduces `JsonOps`' conversion so nested numeric tags keep vanilla's types.
fn deserialize_compound_tag<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<NbtCompound, D::Error> {
    match json_to_nbt_tag(&serde_json::Value::deserialize(deserializer)?) {
        Ok(NbtTag::Compound(compound)) => Ok(compound),
        Ok(_) => Err(D::Error::custom("expected a compound tag")),
        Err(error) => Err(D::Error::custom(error)),
    }
}

fn json_to_nbt_tag(value: &serde_json::Value) -> Result<NbtTag, String> {
    match value {
        serde_json::Value::Null => Err("null is not a valid NBT tag".to_owned()),
        serde_json::Value::Bool(value) => Ok(NbtTag::Byte(i8::from(*value))),
        serde_json::Value::Number(number) => Ok(json_number_to_nbt_tag(number)),
        serde_json::Value::String(value) => Ok(NbtTag::String(value.as_str().into())),
        serde_json::Value::Array(entries) => {
            let mut tags = Vec::with_capacity(entries.len());
            for entry in entries {
                tags.push(json_to_nbt_tag(entry)?);
            }
            Ok(NbtTag::List(tags.into()))
        }
        serde_json::Value::Object(entries) => {
            let mut compound = NbtCompound::new();
            for (key, entry) in entries {
                // JSON object keys are always strings, matching vanilla's
                // `JsonObject` key set exactly.
                compound.insert(key.as_str(), json_to_nbt_tag(entry)?);
            }
            Ok(NbtTag::Compound(compound))
        }
    }
}

/// Mirrors `JsonOps`' numeric narrowing: whole numbers become the narrowest of
/// byte/short/int/long, fractional numbers the narrowest of float/double.
fn json_number_to_nbt_tag(number: &serde_json::Number) -> NbtTag {
    if let Some(value) = number.as_i64() {
        return narrow_integer(value);
    }
    if let Some(value) = number.as_u64()
        && let Ok(value) = i64::try_from(value)
    {
        return narrow_integer(value);
    }
    let value = number.as_f64().unwrap_or_default();
    let single = value as f32;
    if f64::from(single) == value {
        NbtTag::Float(single)
    } else {
        NbtTag::Double(value)
    }
}

fn narrow_integer(value: i64) -> NbtTag {
    if let Ok(value) = i8::try_from(value) {
        NbtTag::Byte(value)
    } else if let Ok(value) = i16::try_from(value) {
        NbtTag::Short(value)
    } else if let Ok(value) = i32::try_from(value) {
        NbtTag::Int(value)
    } else {
        NbtTag::Long(value)
    }
}
