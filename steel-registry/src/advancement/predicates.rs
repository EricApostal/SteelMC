//! Advancement criterion predicates.
//!
//! Mirrors the vanilla `net.minecraft.advancements.predicates` package: the
//! entity/location/item/damage predicates and the `LootCondition` subset used by
//! advancement criteria. Vanilla shares these classes with the loot system; Steel
//! keeps a separate set here because the loot runtime predates the advancement
//! criteria and is being unified with it incrementally.

use crate::data_components::components::ItemEnchantments;
use crate::equipment::EquipmentSlot;
pub use crate::item_predicate::{DoubleBounds, IntBounds};
use crate::loot_table::{EntityEquipmentSlots, ItemSetPredicate, PropertyCheck, ToolPredicate};
use crate::{DyeColor, REGISTRY, RegistryExt, TaggedRegistryExt, item_stack::ItemStack};
use steel_utils::{BlockPos, BlockStateId, Identifier};
use text_components::TextComponent;

/// Bounds alias matching vanilla `MinMaxBounds.Ints`.
pub type IntMinMax = IntBounds;
/// Bounds alias matching vanilla `MinMaxBounds.Doubles`.
pub type DoubleMinMax = DoubleBounds;

impl IntBounds {
    /// Vanilla `MinMaxBounds.Ints.matches`.
    #[must_use]
    pub fn matches(&self, value: i32) -> bool {
        self.min().is_none_or(|min| value >= min) && self.max().is_none_or(|max| value <= max)
    }
}

impl DoubleBounds {
    /// Vanilla `MinMaxBounds.Doubles.matches`.
    #[must_use]
    pub fn matches(&self, value: f64) -> bool {
        self.min().is_none_or(|min| value >= min) && self.max().is_none_or(|max| value <= max)
    }

    /// Vanilla `MinMaxBounds.Doubles.matchesSqr`: compares the *squared* value
    /// against the *squared* bounds (vanilla `boundsSqr` holds `min^2`/`max^2`).
    #[must_use]
    pub fn matches_sqr(&self, value_sqr: f64) -> bool {
        self.min().is_none_or(|min| min * min <= value_sqr)
            && self.max().is_none_or(|max| max * max >= value_sqr)
    }
}

/// A holder set of registry keys as it appears in predicate JSON.
///
/// Advancement predicates accept a single key, a list of keys or a `#tag`;
/// vanilla `HolderSet` first drawn from JSON uses the same set semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HolderSet {
    /// A `#tag` reference.
    Tag(Identifier),
    /// Direct registry keys.
    Keys(&'static [&'static str]),
}

impl Default for HolderSet {
    fn default() -> Self {
        Self::Keys(&[])
    }
}

/// Vanilla `StatePropertiesPredicate`: per-property exact or ranged matches.
#[derive(Debug, Clone, Default)]
pub struct StatePropertiesPredicate {
    pub properties: &'static [PropertyCheck],
}

impl StatePropertiesPredicate {
    /// Vanilla `StatePropertiesPredicate.matches(BlockState)`.
    #[must_use]
    pub fn matches(&self, state: BlockStateId) -> bool {
        use crate::blocks::block_state_ext::BlockStateExt;
        self.properties.iter().all(|property| {
            state
                .get_property_str(property.name)
                .is_some_and(|value| value == property.value)
        })
    }
}

/// Vanilla `BlockPredicate`: a required block holder set plus state properties.
#[derive(Debug, Clone, Default)]
pub struct BlockPredicate {
    /// Required blocks (vanilla `blocks`).
    pub blocks: Option<HolderSet>,
    /// Required block state properties (vanilla `state`).
    pub state: Option<StatePropertiesPredicate>,
}

impl BlockPredicate {
    /// Vanilla `BlockPredicate.matches(BlockState)`.
    #[must_use]
    pub fn matches(&self, state: BlockStateId) -> bool {
        use crate::blocks::block_state_ext::BlockStateExt;
        self.blocks
            .as_ref()
            .is_none_or(|blocks| blocks.contains_block(&state.get_block().key))
            && self
                .state
                .as_ref()
                .is_none_or(|expected| expected.matches(state))
    }
}

/// A vanilla `DataComponentExactPredicate` entry.
///
/// Vanilla's exact predicate holds an arbitrary component value; Steel models
/// the components that appear in vanilla advancement data.
#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "variants mirror vanilla DataComponentExactPredicate payloads and are built once per criterion, not moved in hot paths"
)]
pub enum ExactComponent {
    /// `minecraft:damage` must equal this value.
    Damage(i32),
    /// `minecraft:banner_patterns` must equal this exact layer list.
    BannerPatterns(&'static [(&'static str, DyeColor)]),
    /// `minecraft:item_name` must equal this component.
    ItemName(TextComponent),
}

impl ExactComponent {
    /// Vanilla `DataComponentExactPredicate.test(DataComponentGetter)`: the
    /// component must be present and equal to the expected value.
    #[must_use]
    pub fn matches(&self, stack: &ItemStack) -> bool {
        use crate::data_components::vanilla_components::{BANNER_PATTERNS, DAMAGE, ITEM_NAME};
        match self {
            Self::Damage(expected) => stack.get(DAMAGE) == Some(expected),
            Self::BannerPatterns(expected) => stack.get(BANNER_PATTERNS).is_some_and(|layers| {
                layers.layers().len() == expected.len()
                    && layers.layers().iter().zip(expected.iter()).all(
                        |(layer, (pattern, color))| {
                            layer.pattern().as_reference().is_some_and(|pattern_ref| {
                                pattern_ref.key.path.as_ref() == *pattern
                            }) && layer.color() == *color
                        },
                    )
            }),
            Self::ItemName(expected) => stack.get(ITEM_NAME) == Some(expected),
        }
    }
}

/// Vanilla `EnchantmentPredicate`: one required enchantment (or any) with a level range.
#[derive(Debug, Clone)]
pub struct EnchantmentPredicate {
    /// Required enchantments (vanilla `enchantments`); `None` means "any".
    pub enchantments: Option<HolderSet>,
    /// Required levels (vanilla `levels`).
    pub min_level: Option<i32>,
    pub max_level: Option<i32>,
}

impl EnchantmentPredicate {
    /// Vanilla `EnchantmentPredicate.containedIn(ItemEnchantments)`.
    #[must_use]
    pub fn contained_in(&self, enchantments: &ItemEnchantments) -> bool {
        if let Some(required) = &self.enchantments {
            return enchantments.levels.iter().any(|(key, level)| {
                required.contains_enchantment(key)
                    && self
                        .min_level
                        .is_none_or(|min| i32::try_from(*level).unwrap_or(i32::MAX) >= min)
                    && self
                        .max_level
                        .is_none_or(|max| i32::try_from(*level).unwrap_or(i32::MAX) <= max)
            });
        }
        if self.min_level.is_some() || self.max_level.is_some() {
            return enchantments.levels.values().any(|level| {
                let level = i32::try_from(*level).unwrap_or(i32::MAX);
                self.min_level.is_none_or(|min| level >= min)
                    && self.max_level.is_none_or(|max| level <= max)
            });
        }
        !enchantments.levels.is_empty()
    }
}

/// The vanilla `ItemPredicate` used by advancement criteria.
///
/// Mirrors `net.minecraft.advancements.predicates.ItemPredicate`: the item holder
/// set, count bounds and the `DataComponentMatchers` subset advancement data uses.
#[derive(Debug, Clone, Default)]
pub struct ItemPredicate {
    /// Required items (vanilla `items`).
    pub items: Option<ItemSetPredicate>,
    /// Required stack size (vanilla `count`).
    pub count: Option<IntBounds>,
    /// Required components by presence only (vanilla `predicates` any-value entries).
    pub has_components: &'static [Identifier],
    /// Required exact component values (vanilla `components`).
    pub exact_components: &'static [ExactComponent],
    /// `minecraft:enchantments` component predicates.
    pub enchantments: &'static [EnchantmentPredicate],
    /// `minecraft:stored_enchantments` component predicates.
    pub stored_enchantments: &'static [EnchantmentPredicate],
}

impl ItemPredicate {
    /// Vanilla `ItemPredicate.test(ItemStack)`.
    #[must_use]
    pub fn test(&self, stack: &ItemStack) -> bool {
        use crate::data_components::vanilla_components::{ENCHANTMENTS, STORED_ENCHANTMENTS};
        if self.items.as_ref().is_some_and(|items| !items.test(stack)) {
            return false;
        }
        if let Some(count) = &self.count
            && !count.matches(stack.count())
        {
            return false;
        }
        if !self
            .exact_components
            .iter()
            .all(|exact| exact.matches(stack))
        {
            return false;
        }
        if !self
            .has_components
            .iter()
            .all(|key| stack.has_component(key))
        {
            return false;
        }
        if !self.enchantments.is_empty() {
            let Some(values) = stack.get(ENCHANTMENTS) else {
                return false;
            };
            if !self
                .enchantments
                .iter()
                .all(|predicate| predicate.contained_in(values))
            {
                return false;
            }
        }
        if !self.stored_enchantments.is_empty() {
            let Some(values) = stack.get(STORED_ENCHANTMENTS) else {
                return false;
            };
            if !self
                .stored_enchantments
                .iter()
                .all(|predicate| predicate.contained_in(values))
            {
                return false;
            }
        }
        true
    }
}

/// Vanilla `LocationPredicate`.
#[derive(Debug, Clone, Default)]
pub struct LocationPredicate {
    /// Exact block position (`position`).
    pub position: Option<PositionPredicate>,
    /// Required biome (holder set).
    pub biomes: Option<HolderSet>,
    /// Required structure (holder set).
    pub structures: Option<HolderSet>,
    /// Required dimension key.
    pub dimension: Option<Identifier>,
    /// Light level range.
    pub light: Option<IntBounds>,
    /// Required block.
    pub block: Option<BlockPredicate>,
    /// Required fluid key.
    pub fluid: Option<Identifier>,
    /// `smokey` flag.
    pub smokey: Option<bool>,
    /// `can_see_sky` flag.
    pub can_see_sky: Option<bool>,
}

/// Vanilla `LocationPredicate.PositionPredicate`: absolute or relative bounds.
#[derive(Debug, Clone, Default)]
pub struct PositionPredicate {
    pub x: Option<DoubleBounds>,
    pub y: Option<DoubleBounds>,
    pub z: Option<DoubleBounds>,
}

impl PositionPredicate {
    /// Vanilla `PositionPredicate.matches(x, y, z)`.
    #[must_use]
    pub fn matches(&self, x: f64, y: f64, z: f64) -> bool {
        self.x.as_ref().is_none_or(|bounds| bounds.matches(x))
            && self.y.as_ref().is_none_or(|bounds| bounds.matches(y))
            && self.z.as_ref().is_none_or(|bounds| bounds.matches(z))
    }
}

impl LocationPredicate {
    /// Vanilla `LocationPredicate.matches(Level, x, y, z)`.
    ///
    /// The coordinates select the position inside `level` that every other
    /// sub-predicate is resolved at; pass [`LevelView::position`] to test the
    /// position the view itself is bound to.
    #[must_use]
    pub fn matches_at(&self, level: &dyn LevelView, position: (f64, f64, f64)) -> bool {
        if let Some(expected) = &self.position
            && !expected.matches(position.0, position.1, position.2)
        {
            return false;
        }
        if let Some(biomes) = &self.biomes
            && !level
                .biome_at(position)
                .is_some_and(|biome| biomes.contains_biome(biome))
        {
            return false;
        }
        if let Some(structures) = &self.structures
            && !level
                .structure_at(position)
                .is_some_and(|structure| structures.contains_structure(structure))
        {
            return false;
        }
        if let Some(dimension) = &self.dimension
            && level.dimension() != dimension
        {
            return false;
        }
        if let Some(light) = &self.light
            && !light.matches(level.light_at(position))
        {
            return false;
        }
        if let Some(block) = &self.block {
            let Some(state) = level.block_state_at(position) else {
                return false;
            };
            if !block.matches(state) {
                return false;
            }
        }
        if let Some(fluid) = &self.fluid
            && level.fluid_at(position) != Some(fluid)
        {
            return false;
        }
        if let Some(smokey) = self.smokey
            && level.is_smokey_at(position) != smokey
        {
            return false;
        }
        if let Some(can_see_sky) = self.can_see_sky
            && level.can_see_sky_at(position) != can_see_sky
        {
            return false;
        }
        true
    }

    /// Vanilla `LocationPredicate.matches(Level, x, y, z)` at the view's position.
    #[must_use]
    pub fn matches(&self, level: &dyn LevelView) -> bool {
        self.matches_at(level, level.position())
    }
}

impl HolderSet {
    #[must_use]
    pub fn contains_biome(&self, biome: &Identifier) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY
                .biomes
                .by_key(biome)
                .is_some_and(|biome| REGISTRY.biomes.is_in_tag(biome, tag)),
            Self::Keys(keys) => keys.iter().any(|key| key == &biome.path.as_ref()),
        }
    }

    /// Tests an entity-type key against this set.
    #[must_use]
    pub fn contains_entity_type(&self, key: &Identifier) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY
                .entity_types
                .by_key(key)
                .is_some_and(|entity_type| REGISTRY.entity_types.is_in_tag(entity_type, tag)),
            Self::Keys(keys) => keys.iter().any(|candidate| candidate == &key.path.as_ref()),
        }
    }

    #[must_use]
    pub fn contains_structure(&self, key: &Identifier) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY
                .structures
                .by_key(key)
                .is_some_and(|structure| REGISTRY.structures.is_in_tag(structure, tag)),
            Self::Keys(keys) => keys.iter().any(|candidate| candidate == &key.path.as_ref()),
        }
    }

    #[must_use]
    pub fn contains_item(&self, item: &Identifier) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY
                .items
                .by_key(item)
                .is_some_and(|item| REGISTRY.items.is_in_tag(item, tag)),
            Self::Keys(keys) => keys
                .iter()
                .any(|candidate| candidate == &item.path.as_ref()),
        }
    }

    #[must_use]
    pub fn contains_effect(&self, effect: &Identifier) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY
                .mob_effects
                .by_key(effect)
                .is_some_and(|effect| REGISTRY.mob_effects.is_in_tag(effect, tag)),
            Self::Keys(keys) => keys
                .iter()
                .any(|candidate| candidate == &effect.path.as_ref()),
        }
    }

    /// Tests an enchantment key against this set.
    #[must_use]
    pub fn contains_enchantment(&self, enchantment: &Identifier) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY
                .enchantments
                .by_key(enchantment)
                .is_some_and(|definition| REGISTRY.enchantments.is_in_tag(definition, tag)),
            Self::Keys(keys) => keys
                .iter()
                .any(|candidate| candidate == &enchantment.path.as_ref()),
        }
    }

    #[must_use]
    pub fn contains_loot_table(&self, key: &Identifier) -> bool {
        match self {
            // Vanilla has no loot-table tags; `LootTable.LIST_CODEC` only carries ids.
            Self::Tag(_) => false,
            Self::Keys(keys) => keys.iter().any(|candidate| candidate == &key.path.as_ref()),
        }
    }

    #[must_use]
    pub fn contains_block(&self, key: &Identifier) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY
                .blocks
                .by_key(key)
                .is_some_and(|block| REGISTRY.blocks.is_in_tag(block, tag)),
            Self::Keys(keys) => keys.iter().any(|candidate| candidate == &key.path.as_ref()),
        }
    }

    #[must_use]
    pub fn contains_recipe(&self, key: &Identifier) -> bool {
        match self {
            // Vanilla has no recipe tags; `RecipeHolder` sets only carry ids.
            Self::Tag(_) => false,
            Self::Keys(keys) => keys.iter().any(|candidate| candidate == &key.path.as_ref()),
        }
    }
}

/// Vanilla `DistancePredicate`: bounds on the three axes, horizontal and absolute distance.
#[derive(Debug, Clone, Default)]
pub struct DistancePredicate {
    pub x: Option<DoubleBounds>,
    pub y: Option<DoubleBounds>,
    pub z: Option<DoubleBounds>,
    pub horizontal: Option<DoubleBounds>,
    pub absolute: Option<DoubleBounds>,
}

impl DistancePredicate {
    /// Vanilla `DistancePredicate.matches(x0, y0, z0, x1, y1, z1)`.
    #[must_use]
    pub fn matches(&self, from: (f64, f64, f64), to: (f64, f64, f64)) -> bool {
        // Vanilla casts each delta to `float` before `Mth.abs`, then compares the
        // axis bounds against the float deltas and the squared bounds.
        #[expect(clippy::cast_possible_truncation)]
        let (xd, yd, zd) = (
            (from.0 - to.0) as f32,
            (from.1 - to.1) as f32,
            (from.2 - to.2) as f32,
        );
        self.x
            .as_ref()
            .is_none_or(|bounds| bounds.matches(f64::from(xd.abs())))
            && self
                .y
                .as_ref()
                .is_none_or(|bounds| bounds.matches(f64::from(yd.abs())))
            && self
                .z
                .as_ref()
                .is_none_or(|bounds| bounds.matches(f64::from(zd.abs())))
            && self
                .horizontal
                .as_ref()
                .is_none_or(|bounds| bounds.matches_sqr(f64::from(xd * xd + zd * zd)))
            && self
                .absolute
                .as_ref()
                .is_none_or(|bounds| bounds.matches_sqr(f64::from(xd * xd + yd * yd + zd * zd)))
    }
}

/// Vanilla `MobEffectsPredicate`: required effect instances and their bounds.
#[derive(Debug, Clone, Default)]
pub struct MobEffectsPredicate {
    pub effects: &'static [(&'static str, MobEffectPredicateData)],
}

/// One effect requirement of a [`MobEffectsPredicate`].
#[derive(Debug, Clone, Default)]
pub struct MobEffectPredicateData {
    /// Vanilla `MobEffectInstancePredicate.amplifier`.
    pub amplifier: Option<IntBounds>,
    /// Vanilla `MobEffectInstancePredicate.duration`.
    pub duration: Option<IntBounds>,
    /// Vanilla `MobEffectInstancePredicate.ambient`.
    pub ambient: Option<bool>,
    /// Vanilla `MobEffectInstancePredicate.visible`.
    pub visible: Option<bool>,
}

/// A live mob effect as seen by [`MobEffectsPredicate`].
#[derive(Debug, Clone, Copy)]
pub struct EffectInstanceView<'a> {
    pub effect: &'a Identifier,
    pub amplifier: i32,
    pub duration: i32,
    pub ambient: bool,
    pub visible: bool,
}

impl MobEffectsPredicate {
    /// Vanilla `MobEffectsPredicate.matches(entity)`.
    #[must_use]
    pub fn matches(&self, effects: &[EffectInstanceView<'_>]) -> bool {
        self.effects.iter().all(|(key, expected)| {
            effects
                .iter()
                .filter(|effect| effect.effect.path.as_ref() == *key)
                .any(|effect| {
                    expected
                        .amplifier
                        .as_ref()
                        .is_none_or(|bounds| bounds.matches(effect.amplifier))
                        && expected
                            .duration
                            .as_ref()
                            .is_none_or(|bounds| bounds.matches(effect.duration))
                        && expected
                            .ambient
                            .is_none_or(|ambient| ambient == effect.ambient)
                        && expected
                            .visible
                            .is_none_or(|visible| visible == effect.visible)
                })
        })
    }
}

/// Vanilla `EntityPredicate`: an AND of entity sub-predicates.
#[derive(Debug, Clone, Default)]
pub struct EntityPredicate {
    /// `minecraft:entity_type`.
    pub entity_type: Option<HolderSet>,
    /// `minecraft:location`.
    pub location: Option<LocationPredicate>,
    /// `minecraft:stepping_on`.
    pub stepping_on: Option<LocationPredicate>,
    /// `minecraft:distance`.
    pub distance: Option<DistancePredicate>,
    /// `minecraft:effects`.
    pub effects: Option<MobEffectsPredicate>,
    /// `minecraft:flags`.
    pub flags: Option<EntityFlagsPredicate>,
    /// `minecraft:equipment`.
    pub equipment: Option<EntityEquipmentPredicate>,
    /// `minecraft:components` (the data-component predicates present in vanilla data).
    pub components: Option<EntityComponentsPredicate>,
    /// `minecraft:vehicle`.
    pub vehicle: Option<Box<EntityPredicate>>,
    /// `minecraft:passenger`.
    pub passenger: Option<Box<EntityPredicate>>,
    /// `minecraft:type_specific/player`.
    pub player: Option<Box<PlayerPredicate>>,
    /// `minecraft:type_specific/lightning`.
    pub lightning: Option<Box<LightningPredicate>>,
}

/// Vanilla `EntityFlagsPredicate`.
#[derive(Debug, Clone, Default)]
pub struct EntityFlagsPredicate {
    pub is_on_ground: Option<bool>,
    pub is_on_fire: Option<bool>,
    pub is_sneaking: Option<bool>,
    pub is_sprinting: Option<bool>,
    pub is_swimming: Option<bool>,
    pub is_flying: Option<bool>,
    pub is_baby: Option<bool>,
}

/// Vanilla `EntityEquipmentPredicate`.
#[derive(Debug, Clone, Default)]
pub struct EntityEquipmentPredicate {
    pub head: Option<ItemPredicate>,
    pub chest: Option<ItemPredicate>,
    pub legs: Option<ItemPredicate>,
    pub feet: Option<ItemPredicate>,
    pub body: Option<ItemPredicate>,
    pub mainhand: Option<ItemPredicate>,
    pub offhand: Option<ItemPredicate>,
}

/// The entity data-component predicates actually referenced by vanilla data.
///
/// These mirror `EntityExactDataComponentsPredicate`/`EntityPartialComponentsPredicate`
/// for the variant components used by advancement criteria; more component
/// predicates are added here as their entity types land.
#[derive(Debug, Clone, Default)]
pub struct EntityComponentsPredicate {
    pub cat_variant: Option<Identifier>,
    pub frog_variant: Option<Identifier>,
    pub wolf_variant: Option<Identifier>,
    pub sheep_color: Option<crate::DyeColor>,
    pub sheep_sheared: Option<bool>,
    pub chicken_variant: Option<Identifier>,
}

/// Vanilla `PlayerPredicate` (the subset used by advancement criteria).
#[derive(Debug, Clone, Default)]
pub struct PlayerPredicate {
    /// `looking_at`.
    pub looking_at: Option<EntityPredicate>,
    /// `recipes` (unlocked-recipe prerequisites).
    pub recipes: Option<&'static [&'static str]>,
}

/// Vanilla `L lightningPredicate` (`minecraft:type_specific/lightning`).
#[derive(Debug, Clone, Default)]
pub struct LightningPredicate {
    pub blocks_set_on_fire: Option<IntBounds>,
    pub entity_struck: Option<EntityPredicate>,
}

/// Vanilla `DamageSourcePredicate`.
#[derive(Debug, Clone, Default)]
pub struct DamageSourcePredicate {
    /// `tags`: required tag presence.
    pub tags: &'static [(&'static str, bool)],
    /// `source_entity`.
    pub source_entity: Option<EntityPredicate>,
    /// `direct_entity`.
    pub direct_entity: Option<EntityPredicate>,
    /// `is_direct`.
    pub is_direct: Option<bool>,
}

/// Vanilla `DamagePredicate`.
#[derive(Debug, Clone, Default)]
pub struct DamagePredicate {
    /// Bounds on the original damage amount.
    pub dealt: Option<DoubleBounds>,
    /// Bounds on the damage actually taken.
    pub taken: Option<DoubleBounds>,
    /// `source_entity` predicate.
    pub source_entity: Option<EntityPredicate>,
    /// `blocked` flag.
    pub blocked: Option<bool>,
    /// Nested damage-source predicate.
    pub damage_type: Option<DamageSourcePredicate>,
}

/// The live damage facts tested by a [`DamagePredicate`].
#[derive(Debug, Clone, Copy)]
pub struct DamageView<'a> {
    pub original: f32,
    pub actual: f32,
    pub blocked: bool,
    pub source_entity: Option<&'a EntityView<'a>>,
    pub direct_entity: Option<&'a EntityView<'a>>,
    pub type_key: Option<&'a Identifier>,
    pub is_direct: bool,
}

impl DamageSourcePredicate {
    /// Vanilla `DamageSourcePredicate.matches`.
    #[must_use]
    pub fn matches(&self, damage: &DamageView<'_>) -> bool {
        for (tag, expected) in self.tags {
            let tag = Identifier::vanilla_static(tag);
            let has_tag = damage.type_key.is_some_and(|key| {
                REGISTRY
                    .damage_types
                    .by_key(key)
                    .is_some_and(|damage_type| REGISTRY.damage_types.is_in_tag(damage_type, &tag))
            });
            if has_tag != *expected {
                return false;
            }
        }
        if let Some(is_direct) = self.is_direct
            && is_direct != damage.is_direct
        {
            return false;
        }
        if let Some(predicate) = &self.source_entity {
            let Some(entity) = damage.source_entity else {
                return false;
            };
            if !predicate.matches(entity) {
                return false;
            }
        }
        if let Some(predicate) = &self.direct_entity {
            let Some(entity) = damage.direct_entity else {
                return false;
            };
            if !predicate.matches(entity) {
                return false;
            }
        }
        true
    }
}

impl DamagePredicate {
    /// Vanilla `DamagePredicate.matches(player, source, original, actual, blocked)`.
    #[must_use]
    pub fn matches(&self, damage: &DamageView<'_>) -> bool {
        if let Some(dealt) = &self.dealt
            && !dealt.matches(f64::from(damage.original))
        {
            return false;
        }
        if let Some(taken) = &self.taken
            && !taken.matches(f64::from(damage.actual))
        {
            return false;
        }
        if let Some(blocked) = self.blocked
            && blocked != damage.blocked
        {
            return false;
        }
        if let Some(source_entity) = &self.source_entity {
            let Some(entity) = damage.source_entity else {
                return false;
            };
            if !source_entity.matches(entity) {
                return false;
            }
        }
        if let Some(damage_type) = &self.damage_type
            && !damage_type.matches(damage)
        {
            return false;
        }
        true
    }
}

/// The number of nested entity-graph levels an [`EntityView`] snapshot keeps.
///
/// Steel builds the whole vehicle/passenger/`lookingAt` graph eagerly while
/// vanilla evaluates it lazily per predicate, so a cap (plus a per-path cycle
/// cut in the builder) keeps a cyclic graph like rider <-> boat finite.
pub const MAX_ENTITY_VIEW_DEPTH: u8 = 8;

/// Vanilla `PlayerPredicate.LOOKING_AT_RANGE`: the view-vector reach of the
/// `looking_at` entity pick, in blocks.
pub const LOOKING_AT_RANGE: f64 = 100.0;

/// The live entity facts tested by an [`EntityPredicate`].
#[derive(Clone)]
pub struct EntityView<'a> {
    pub entity_type: Option<&'a Identifier>,
    pub position: (f64, f64, f64),
    pub level: &'a dyn LevelView,
    pub stepping_on: Option<&'a dyn LevelView>,
    pub flags: EntityViewFlags,
    pub equipment: Option<EntityEquipmentSlots>,
    pub effects: Vec<EffectInstanceView<'a>>,
    pub components: EntityComponentsPredicate,
    /// The vehicle this entity is riding (vanilla `minecraft:vehicle`).
    pub vehicle: Option<Box<EntityView<'a>>>,
    /// This entity's direct passengers (vanilla `minecraft:passenger`
    /// matches when *any* of them matches).
    pub passengers: Vec<EntityView<'a>>,
    pub player: Option<PlayerView<'a>>,
    pub lightning: Option<LightningView<'a>>,
    /// The stack held in the entity's main hand, used by `fishing_rod_hooked`'s
    /// hooked-item check.
    pub held_item: Option<ItemStack>,
}

impl EntityView<'_> {
    /// Whether the entity's held item satisfies an item predicate.
    #[must_use]
    pub fn held_item_matches(&self, predicate: &ItemPredicate) -> bool {
        self.held_item
            .as_ref()
            .is_some_and(|stack| item_predicate_matches(predicate, stack))
    }
}

impl std::fmt::Debug for EntityView<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EntityView")
            .field("entity_type", &self.entity_type)
            .field("position", &self.position)
            .field("flags", &self.flags)
            .finish_non_exhaustive()
    }
}

/// Flag values of an [`EntityView`].
#[derive(Debug, Clone, Copy, Default)]
pub struct EntityViewFlags {
    pub is_on_ground: bool,
    pub is_on_fire: bool,
    pub is_sneaking: bool,
    pub is_sprinting: bool,
    pub is_swimming: bool,
    pub is_flying: bool,
    pub is_baby: bool,
}

/// Player-only facts of an [`EntityView`].
#[derive(Debug, Clone, Default)]
pub struct PlayerView<'a> {
    /// The entity the player's view vector resolves to, mirroring vanilla
    /// `PlayerPredicate`'s `lookingAt`: the nearest non-spectator entity the
    /// view vector reaches within [`LOOKING_AT_RANGE`] blocks that the player
    /// also has line of sight to.
    pub looking_at: Option<Box<EntityView<'a>>>,
    pub unlocked_recipes: &'a [Identifier],
}

/// Lightning-only facts of an [`EntityView`].
#[derive(Debug, Clone, Default, Copy)]
pub struct LightningView<'a> {
    pub blocks_set_on_fire: i32,
    pub entity_struck: Option<&'a EntityView<'a>>,
}

/// World facts a [`LocationPredicate`] tests.
///
/// Vanilla resolves these through `Level`; Steel exposes a narrow view so the
/// registry crate stays free of world access.
pub trait LevelView {
    /// Absolute coordinates of the position this view is bound to.
    fn position(&self) -> (f64, f64, f64);
    /// Dimension key.
    fn dimension(&self) -> &Identifier;
    /// Biome at `position`.
    fn biome_at(&self, position: (f64, f64, f64)) -> Option<&Identifier>;
    /// Structure at `position`.
    fn structure_at(&self, position: (f64, f64, f64)) -> Option<&Identifier>;
    /// Block state at `position`.
    fn block_state_at(&self, position: (f64, f64, f64)) -> Option<BlockStateId>;
    /// Fluid key at `position`.
    fn fluid_at(&self, position: (f64, f64, f64)) -> Option<&Identifier>;
    /// Combined light level at `position`.
    fn light_at(&self, position: (f64, f64, f64)) -> i32;
    /// Vanilla `LocationPredicate.smokey`: a campfire below the position.
    fn is_smokey_at(&self, _position: (f64, f64, f64)) -> bool {
        false
    }
    /// Whether the sky is visible from `position`.
    fn can_see_sky_at(&self, _position: (f64, f64, f64)) -> bool {
        false
    }

    /// Biome at the view's own position.
    fn biome(&self) -> Option<&Identifier> {
        self.biome_at(self.position())
    }
    /// Structure at the view's own position.
    fn structure(&self) -> Option<&Identifier> {
        self.structure_at(self.position())
    }
    /// Block state at the view's own position.
    fn block_state(&self) -> Option<BlockStateId> {
        self.block_state_at(self.position())
    }
    /// Fluid at the view's own position.
    fn fluid(&self) -> Option<&Identifier> {
        self.fluid_at(self.position())
    }
    /// Light level at the view's own position.
    fn light(&self) -> i32 {
        self.light_at(self.position())
    }
}

impl EntityFlagsPredicate {
    /// Vanilla `EntityFlagsPredicate.matches(entity)`.
    #[must_use]
    pub fn matches(&self, flags: EntityViewFlags) -> bool {
        self.is_on_ground
            .is_none_or(|expected| expected == flags.is_on_ground)
            && self
                .is_on_fire
                .is_none_or(|expected| expected == flags.is_on_fire)
            && self
                .is_sneaking
                .is_none_or(|expected| expected == flags.is_sneaking)
            && self
                .is_sprinting
                .is_none_or(|expected| expected == flags.is_sprinting)
            && self
                .is_swimming
                .is_none_or(|expected| expected == flags.is_swimming)
            && self
                .is_flying
                .is_none_or(|expected| expected == flags.is_flying)
            && self
                .is_baby
                .is_none_or(|expected| expected == flags.is_baby)
    }
}

impl EntityEquipmentPredicate {
    /// Vanilla `EntityEquipmentPredicate.matches(entity)`.
    #[must_use]
    pub fn matches(&self, equipment: Option<&crate::loot_table::EntityEquipmentSlots>) -> bool {
        let slots = [
            (EquipmentSlot::Head, self.head.as_ref()),
            (EquipmentSlot::Chest, self.chest.as_ref()),
            (EquipmentSlot::Legs, self.legs.as_ref()),
            (EquipmentSlot::Feet, self.feet.as_ref()),
            (EquipmentSlot::Body, self.body.as_ref()),
            (EquipmentSlot::MainHand, self.mainhand.as_ref()),
            (EquipmentSlot::OffHand, self.offhand.as_ref()),
        ];
        if slots.iter().all(|(_, predicate)| predicate.is_none()) {
            return true;
        }
        let Some(equipment) = equipment else {
            return false;
        };
        slots.into_iter().all(|(slot, predicate)| {
            predicate.is_none_or(|predicate| {
                let stack = &equipment[slot.index()];
                item_predicate_matches(predicate, stack)
            })
        })
    }
}

/// Vanilla `ItemPredicate.test(ItemStack)`.
#[must_use]
pub fn item_predicate_matches(predicate: &ItemPredicate, stack: &ItemStack) -> bool {
    predicate.test(stack)
}

/// Vanilla `ItemPredicate.test(stack)` for the `ToolPredicate` shape used by equipment.
#[must_use]
pub fn tool_predicate_matches(predicate: &ToolPredicate, stack: &ItemStack) -> bool {
    match predicate {
        ToolPredicate::Item(item) => stack.item.key == *item,
        ToolPredicate::Tag(tag) => REGISTRY.items.is_in_tag(stack.item, tag),
        ToolPredicate::HasEnchantment {
            enchantment,
            min_level,
        } => {
            if stack.get_enchantment_level(enchantment) >= *min_level {
                return true;
            }
            stack.get_enchantments().is_some_and(|enchantments| {
                enchantments.iter().any(|(key, level)| {
                    *level >= *min_level as u32
                        && REGISTRY.enchantments.by_key(key).is_some_and(|definition| {
                            REGISTRY.enchantments.is_in_tag(definition, enchantment)
                        })
                })
            })
        }
        ToolPredicate::Any => true,
    }
}

impl EntityPredicate {
    /// Vanilla `EntityPredicate.matches(level, position, entity)`.
    #[must_use]
    pub fn matches(&self, entity: &EntityView<'_>) -> bool {
        if let Some(entity_type) = &self.entity_type
            && !entity
                .entity_type
                .is_some_and(|key| entity_type.contains_entity_type(key))
        {
            return false;
        }
        if let Some(location) = &self.location
            && !location.matches(entity.level)
        {
            return false;
        }
        if let Some(stepping_on) = &self.stepping_on {
            let Some(level) = entity.stepping_on else {
                return false;
            };
            if !stepping_on.matches(level) {
                return false;
            }
        }
        if let Some(distance) = &self.distance {
            let (x, y, z) = entity.level.position();
            if !distance.matches((x, y, z), entity.position) {
                return false;
            }
        }
        if let Some(effects) = &self.effects
            && !effects.matches(&entity.effects)
        {
            return false;
        }
        if let Some(flags) = &self.flags
            && !flags.matches(entity.flags)
        {
            return false;
        }
        if let Some(equipment) = &self.equipment
            && !equipment.matches(entity.equipment.as_ref())
        {
            return false;
        }
        if let Some(components) = &self.components
            && !components.matches(&entity.components)
        {
            return false;
        }
        if let Some(vehicle) = &self.vehicle {
            let Some(actual) = entity.vehicle.as_deref() else {
                return false;
            };
            if !vehicle.matches(actual) {
                return false;
            }
        }
        if let Some(passenger) = &self.passenger
            && !entity
                .passengers
                .iter()
                .any(|actual| passenger.matches(actual))
        {
            return false;
        }
        if let Some(player) = &self.player
            && !player.matches(entity.player.as_ref())
        {
            return false;
        }
        if let Some(lightning) = &self.lightning
            && !lightning.matches(entity.lightning)
        {
            return false;
        }
        true
    }
}

impl EntityComponentsPredicate {
    /// Vanilla `components.test(entity)`.
    #[must_use]
    pub fn matches(&self, actual: &Self) -> bool {
        self.cat_variant
            .as_ref()
            .is_none_or(|expected| actual.cat_variant.as_ref() == Some(expected))
            && self
                .frog_variant
                .as_ref()
                .is_none_or(|expected| actual.frog_variant.as_ref() == Some(expected))
            && self
                .wolf_variant
                .as_ref()
                .is_none_or(|expected| actual.wolf_variant.as_ref() == Some(expected))
            && self
                .sheep_color
                .is_none_or(|expected| actual.sheep_color == Some(expected))
            && self
                .sheep_sheared
                .is_none_or(|expected| actual.sheep_sheared == Some(expected))
            && self
                .chicken_variant
                .as_ref()
                .is_none_or(|expected| actual.chicken_variant.as_ref() == Some(expected))
    }
}

impl PlayerPredicate {
    /// Vanilla `PlayerPredicate.matches(player)`.
    #[must_use]
    pub fn matches(&self, player: Option<&PlayerView<'_>>) -> bool {
        let Some(player) = player else {
            return false;
        };
        if let Some(looking_at) = &self.looking_at
            && !player
                .looking_at
                .as_deref()
                .is_some_and(|entity| looking_at.matches(entity))
        {
            return false;
        }
        if let Some(recipes) = self.recipes
            && !recipes.iter().all(|recipe| {
                let path = recipe.strip_prefix("minecraft:").unwrap_or(recipe);
                player
                    .unlocked_recipes
                    .iter()
                    .any(|unlocked| unlocked.path.as_ref() == path)
            })
        {
            return false;
        }
        true
    }
}

impl LightningPredicate {
    /// Vanilla `LightningBoltPredicate.matches`.
    #[must_use]
    pub fn matches(&self, lightning: Option<LightningView<'_>>) -> bool {
        let Some(lightning) = lightning else {
            return false;
        };
        if let Some(bounds) = &self.blocks_set_on_fire
            && !bounds.matches(lightning.blocks_set_on_fire)
        {
            return false;
        }
        if let Some(entity_struck) = &self.entity_struck
            && !lightning
                .entity_struck
                .is_some_and(|entity| entity_struck.matches(entity))
        {
            return false;
        }
        true
    }
}

/// The live facts a criterion condition is tested against.
///
/// Vanilla builds one `LootContext` per evaluated entity; advancement data only
/// ever selects `entity: "this"`, so [`Condition::EntityProperties`] resolves
/// against [`AdvancementContext::this`].
pub struct AdvancementContext<'a> {
    /// The entity the criterion's conditions are about (`THIS_ENTITY`).
    pub this: &'a EntityView<'a>,
    /// The player the criterion is evaluated for.
    pub player: &'a EntityView<'a>,
    /// The item held/used when the trigger fires, if any.
    pub tool: Option<&'a ItemStack>,
    /// World facts at the event position.
    pub level: &'a dyn LevelView,
    /// Block position of the interaction origin.
    pub origin: BlockPos,
}

impl<'a> AdvancementContext<'a> {
    /// A context whose `this` entity is the player.
    #[must_use]
    pub const fn for_player(
        player: &'a EntityView<'a>,
        level: &'a dyn LevelView,
        origin: BlockPos,
        tool: Option<&'a ItemStack>,
    ) -> Self {
        Self {
            this: player,
            player,
            tool,
            level,
            origin,
        }
    }

    /// A context whose `this` entity is a non-player entity.
    #[must_use]
    pub const fn for_entity(
        entity: &'a EntityView<'a>,
        player: &'a EntityView<'a>,
        level: &'a dyn LevelView,
        origin: BlockPos,
        tool: Option<&'a ItemStack>,
    ) -> Self {
        Self {
            this: entity,
            player,
            tool,
            level,
            origin,
        }
    }
}

/// The vanilla `LootItemCondition` subset usable in advancement criteria.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "variants mirror vanilla LootItemCondition payloads and are built once per criterion, not moved in hot paths"
)]
pub enum Condition {
    /// `minecraft:entity_properties`.
    EntityProperties {
        /// Which entity of the context to test.
        entity: crate::loot_table::LootContextEntity,
        /// The entity predicate.
        predicate: EntityPredicate,
    },
    /// `minecraft:location_check`.
    LocationCheck {
        offset_x: i32,
        offset_y: i32,
        offset_z: i32,
        predicate: LocationPredicate,
    },
    /// `minecraft:match_block`.
    MatchBlock {
        /// Required blocks (vanilla `blocks`, a holder set or absent for any).
        blocks: Option<HolderSet>,
        /// Required state properties.
        properties: &'static [PropertyCheck],
    },
    /// `minecraft:match_tool`.
    MatchTool(ItemPredicate),
    /// `minecraft:inverted`.
    Inverted(&'static Condition),
    /// `minecraft:any_of`.
    AnyOf(&'static [Condition]),
    /// `minecraft:all_of`.
    AllOf(&'static [Condition]),
}

impl Condition {
    /// Vanilla `LootItemCondition.test(LootContext)` for advancement contexts.
    #[must_use]
    pub fn test(&self, ctx: &AdvancementContext<'_>) -> bool {
        match self {
            Self::EntityProperties { entity, predicate } => match entity {
                crate::loot_table::LootContextEntity::This => predicate.matches(ctx.this),
                _ => false,
            },
            Self::LocationCheck {
                offset_x,
                offset_y,
                offset_z,
                predicate,
            } => predicate.matches_at(
                ctx.level,
                (
                    f64::from(ctx.origin.x() + offset_x),
                    f64::from(ctx.origin.y() + offset_y),
                    f64::from(ctx.origin.z() + offset_z),
                ),
            ),
            Self::MatchBlock { blocks, properties } => {
                use crate::blocks::block_state_ext::BlockStateExt;
                let Some(state) = ctx.level.block_state() else {
                    return false;
                };
                blocks
                    .as_ref()
                    .is_none_or(|blocks| blocks.contains_block(&state.get_block().key))
                    && properties.iter().all(|property| {
                        state
                            .get_property_str(property.name)
                            .is_some_and(|value| value == property.value)
                    })
            }
            Self::MatchTool(predicate) => ctx
                .tool
                .is_some_and(|tool| item_predicate_matches(predicate, tool)),
            Self::Inverted(inner) => !inner.test(ctx),
            Self::AnyOf(conditions) => conditions.iter().any(|condition| condition.test(ctx)),
            Self::AllOf(conditions) => conditions.iter().all(|condition| condition.test(ctx)),
        }
    }
}

/// Vanilla `SimpleCriterionTrigger.SimpleInstance.player` check.
#[must_use]
pub fn player_condition_matches(
    condition: Option<&Condition>,
    ctx: &AdvancementContext<'_>,
) -> bool {
    condition.is_none_or(|condition| condition.test(ctx))
}

/// Vanilla `ItemPredicate`-based trigger field test against an item stack.
#[must_use]
pub fn optional_item_matches(predicate: Option<&ItemPredicate>, stack: &ItemStack) -> bool {
    predicate.is_none_or(|predicate| item_predicate_matches(predicate, stack))
}
