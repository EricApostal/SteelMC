use super::{
    BlockStateExt, DamageSourceInfo, DyeColor, EntityEquipmentRef, EntityRef, EntityRefFlags,
    Identifier, ItemStack, LootContext, LootContextEntity, NumberProvider, NumberProviderRange,
    REGISTRY, RegistryExt, RngExt, ScoreboardTarget, TaggedRegistryExt, score_of,
};
use crate::data_components::vanilla_components::{ENCHANTMENTS, STORED_ENCHANTMENTS};
use crate::equipment::EquipmentSlot;

/// A property check for block state conditions.
#[derive(Debug, Clone)]
pub struct PropertyCheck {
    pub name: &'static str,
    pub value: &'static str,
}

/// A condition that must be met for a loot entry or pool to apply.
#[derive(Debug, Clone)]
#[expect(clippy::large_enum_variant)]
pub enum LootCondition {
    /// The loot survives explosion damage (random chance based on explosion radius).
    /// Vanilla: 1/radius chance to pass. If no explosion, always passes.
    SurvivesExplosion,
    /// Check the block (and its state properties) matches (vanilla `match_block`).
    MatchBlock {
        block: Identifier,
        properties: &'static [PropertyCheck],
    },
    /// Simple random chance (0.0 to 1.0).
    RandomChance(f32),
    /// Random chance affected by an enchantment (e.g., looting).
    /// Vanilla 1.21+: uses `enchanted_chance` which can be constant or linear.
    RandomChanceWithEnchantedBonus {
        enchantment: Identifier,
        unenchanted_chance: f32,
        /// For linear formula: chance = base + `per_level` * (level - 1)
        enchanted_chance: EnchantedChance,
    },
    /// Match tool predicate - checks if the tool matches certain criteria.
    MatchTool(ToolPredicate),
    /// Table bonus condition - chance based on enchantment level from a table.
    /// The chances array is indexed by enchantment level (0 = no enchant, 1 = level 1, etc.)
    TableBonus {
        enchantment: Identifier,
        chances: &'static [f32],
    },
    /// Inverted condition - passes if the inner condition fails.
    Inverted(&'static LootCondition),
    /// Any of the conditions pass (OR logic).
    AnyOf(&'static [LootCondition]),
    /// All of the conditions pass (AND logic).
    AllOf(&'static [LootCondition]),
    /// Killed by player condition.
    KilledByPlayer,
    /// Entity properties condition - checks entity predicates.
    EntityProperties {
        entity: LootContextEntity,
        predicate: EntityPredicate,
    },
    /// Damage source properties condition - checks how the entity was damaged.
    DamageSourceProperties { predicate: DamageSourcePredicate },
    /// Location check condition - checks the location predicate.
    LocationCheck {
        offset_x: i32,
        offset_y: i32,
        offset_z: i32,
        predicate: LocationPredicate,
    },
    /// Weather check condition - checks current weather.
    WeatherCheck {
        raining: Option<bool>,
        thundering: Option<bool>,
    },
    /// Time check condition - checks game time.
    TimeCheck {
        value: NumberProviderRange,
        period: Option<i64>,
    },
    /// Value check condition - compares a number provider value against a range.
    ValueCheck {
        value: NumberProvider,
        range: NumberProviderRange,
    },
    /// Check if a specific enchantment is active.
    EnchantmentActiveCheck {
        enchantment: Identifier,
        active: bool,
    },
    /// Check entity scoreboard scores.
    EntityScores {
        entity: LootContextEntity,
        scores: &'static [(&'static str, NumberProviderRange)],
    },
    /// Check the value of an environment attribute (vanilla
    /// `minecraft:environment_attribute_check`).
    ///
    /// **Deviation from vanilla:** vanilla compares the attribute's typed value
    /// (`EnvironmentAttributeCheck.value` is decoded by the attribute's own
    /// `AttributeType`, so colours, particles, moon phases, … compare as their own
    /// type). Steel's [`EnvironmentAttributeLookup`] exposes only the
    /// `AttributeType.toFloat` projection, so the comparison happens on that
    /// projection; an attribute whose value is not carried by the hook fails.
    EnvironmentAttributeCheck {
        /// Environment attribute id (vanilla `attribute`).
        attribute: Identifier,
        /// Expected value, as its `AttributeType.toFloat` projection.
        value: f32,
    },
    /// Reference to a named condition in the registry.
    Reference(Identifier),
}

/// Enchanted chance calculation method.
#[derive(Debug, Clone, Copy)]
pub enum EnchantedChance {
    /// Constant chance regardless of enchantment level.
    Constant(f32),
    /// Linear formula: base + `per_level_above_first` * (level - 1)
    Linear {
        base: f32,
        per_level_above_first: f32,
    },
}

/// Predicate for matching tools.
#[derive(Debug, Clone)]
pub enum ToolPredicate {
    /// Match a specific item.
    Item(Identifier),
    /// Match items with a specific enchantment at minimum level.
    HasEnchantment {
        enchantment: Identifier,
        min_level: i32,
    },
    /// Match items in a tag.
    Tag(Identifier),
    /// Always matches (no predicate specified).
    Any,
}

/// Predicate for matching items (vanilla `net.minecraft.advancements.predicates.ItemPredicate`),
/// used by the `minecraft:filtered` loot function's `item_filter`.
#[derive(Debug, Clone)]
pub struct ItemPredicate {
    pub items: Option<ItemSetPredicate>,
    pub count_min: Option<i32>,
    pub count_max: Option<i32>,
    /// Component presence checks (vanilla's `AnyValueType` predicate shape).
    pub has_components: &'static [Identifier],
    /// Vanilla `EnchantmentsPredicate` component predicates.
    pub enchantment_matches: &'static [EnchantmentMatch],
}

/// The `items` holder set of an `ItemPredicate`.
///
/// Item statics are lazily initialized, so a `RegistryHolderSet<Item>` cannot be
/// built in the constant initialisers of the generated loot functions. This
/// mirrors the same two shapes with keys that are resolved at test time.
#[derive(Debug, Clone)]
pub enum ItemSetPredicate {
    /// A `#tag` reference.
    Tag(Identifier),
    /// Direct item keys.
    Items(&'static [&'static str]),
}

impl ItemSetPredicate {
    #[must_use]
    pub fn test(&self, item: &ItemStack) -> bool {
        match self {
            Self::Tag(tag) => REGISTRY.items.is_in_tag(item.item, tag),
            Self::Items(items) => items.iter().any(|key| item.item.key.path.as_ref() == *key),
        }
    }
}

/// One vanilla `EnchantmentPredicate` of an `EnchantmentsPredicate` component
/// predicate: the referenced component must exist and hold an enchantment whose
/// level lies within the bounds.
#[derive(Debug, Clone)]
pub struct EnchantmentMatch {
    pub component: EnchantmentSource,
    /// Vanilla `EnchantmentPredicate.levels` (`MinMaxBounds.Ints`).
    pub min_level: Option<i32>,
    pub max_level: Option<i32>,
}

/// The enchantment component an [`EnchantmentMatch`] inspects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnchantmentSource {
    Enchantments,
    StoredEnchantments,
}

impl ItemPredicate {
    #[must_use]
    pub fn test(&self, item: &ItemStack) -> bool {
        if let Some(items) = &self.items
            && !items.test(item)
        {
            return false;
        }
        if let Some(min) = self.count_min
            && item.count < min
        {
            return false;
        }
        if let Some(max) = self.count_max
            && item.count > max
        {
            return false;
        }
        for r#match in self.enchantment_matches {
            let values = match r#match.component {
                EnchantmentSource::Enchantments => item.get(ENCHANTMENTS),
                EnchantmentSource::StoredEnchantments => item.get(STORED_ENCHANTMENTS),
            };
            let Some(values) = values else {
                return false;
            };
            let matched = if r#match.min_level.is_none() && r#match.max_level.is_none() {
                // Vanilla `EnchantmentPredicate.containedIn` with `MinMaxBounds.Ints.ANY`
                // requires the enchantment component to be non-empty.
                !values.is_empty()
            } else {
                values.iter().any(|(_, level)| {
                    let level = *level as i32;
                    r#match.min_level.is_none_or(|min| level >= min)
                        && r#match.max_level.is_none_or(|max| level <= max)
                })
            };
            if !matched {
                return false;
            }
        }
        self.has_components
            .iter()
            .all(|component| item.has_component(component))
    }
}

// Vanilla shares `LocationPredicate` between the loot and advancement systems
// (`net.minecraft.advancements.predicates.LocationPredicate` is referenced by
// both `LocationCheck` and advancement criteria), so the loot runtime reuses the
// advancement type instead of keeping a second, narrower copy.
pub use crate::advancement::predicates::{
    BlockPredicate, DoubleBounds, HolderSet, IntBounds, LocationPredicate, PositionPredicate,
    StatePropertiesPredicate,
};

/// Predicate for checking entity properties.
#[derive(Debug, Clone)]
pub struct EntityPredicate {
    pub entity_type: Option<Identifier>,
    pub flags: Option<EntityFlags>,
    pub equipment: Option<EntityEquipment>,
    /// Vanilla `minecraft:components.sheep/color` entity data component check.
    pub sheep_color: Option<DyeColor>,
    /// Vanilla `minecraft:type_specific/sheep.sheared` check.
    pub sheep_sheared: Option<bool>,
    /// Vanilla `minecraft:components.chicken/variant` entity data component check.
    pub chicken_variant: Option<Identifier>,
}

/// Entity flags (`is_on_fire`, `is_sneaking`, etc.)
#[derive(Debug, Clone)]
pub struct EntityFlags {
    pub is_on_fire: Option<bool>,
    pub is_sneaking: Option<bool>,
    pub is_sprinting: Option<bool>,
    pub is_swimming: Option<bool>,
    pub is_baby: Option<bool>,
}

/// Entity equipment predicate
#[derive(Debug, Clone)]
pub struct EntityEquipment {
    pub mainhand: Option<ToolPredicate>,
    pub offhand: Option<ToolPredicate>,
    pub head: Option<ToolPredicate>,
    pub chest: Option<ToolPredicate>,
    pub legs: Option<ToolPredicate>,
    pub feet: Option<ToolPredicate>,
}

/// Predicate for checking damage source properties.
#[derive(Debug, Clone)]
pub struct DamageSourcePredicate {
    /// Tags that must be present on the damage source.
    pub tags: &'static [DamageTagPredicate],
    /// Source entity predicate (e.g., the player/mob that caused damage).
    pub source_entity: Option<EntityPredicate>,
    /// Direct entity predicate (e.g., the arrow/fireball).
    pub direct_entity: Option<EntityPredicate>,
    /// Whether damage bypasses armor.
    pub is_direct: Option<bool>,
}

/// A tag check for damage source.
#[derive(Debug, Clone)]
pub struct DamageTagPredicate {
    pub id: Identifier,
    pub expected: bool,
}

impl LootCondition {
    /// Test if this condition passes given the loot context.
    pub fn test<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> bool {
        match self {
            LootCondition::SurvivesExplosion => {
                if let Some(radius) = ctx.explosion_radius {
                    // Vanilla: 1/radius chance to survive
                    ctx.rng.random::<f32>() <= (1.0 / radius)
                } else {
                    true // No explosion, always survives
                }
            }
            LootCondition::MatchBlock { block, properties } => {
                if let Some(state) = ctx.block_state {
                    let state_block = state.get_block();
                    // Check block matches
                    if state_block.key != *block {
                        return false;
                    }
                    // Check all properties match
                    for prop in *properties {
                        if let Some(value) = state.get_property_str(prop.name) {
                            if value != prop.value {
                                return false;
                            }
                        } else {
                            return false; // Property doesn't exist
                        }
                    }
                    true
                } else {
                    false // No block state in context
                }
            }
            LootCondition::RandomChance(chance) => ctx.rng.random::<f32>() < *chance,
            LootCondition::RandomChanceWithEnchantedBonus {
                enchantment,
                unenchanted_chance,
                enchanted_chance,
            } => {
                let level = ctx.get_attacking_entity_enchantment_level(enchantment);
                let effective_chance = if level > 0 {
                    match enchanted_chance {
                        EnchantedChance::Constant(c) => *c,
                        EnchantedChance::Linear {
                            base,
                            per_level_above_first,
                        } => base + per_level_above_first * (level - 1) as f32,
                    }
                } else {
                    *unenchanted_chance
                };
                ctx.rng.random::<f32>() < effective_chance
            }
            LootCondition::MatchTool(predicate) => {
                if let Some(tool) = ctx.tool {
                    predicate.test(tool, ctx)
                } else {
                    // No tool in context - only passes if predicate is Any
                    matches!(predicate, ToolPredicate::Any)
                }
            }
            LootCondition::TableBonus {
                enchantment,
                chances,
            } => {
                let level = ctx.get_enchantment_level_by_id(enchantment);
                let index = (level as usize).min(chances.len().saturating_sub(1));
                let chance = chances.get(index).copied().unwrap_or(0.0);
                ctx.rng.random::<f32>() < chance
            }
            LootCondition::Inverted(inner) => !inner.test(ctx),
            LootCondition::AnyOf(conditions) => conditions.iter().any(|c| c.test(ctx)),
            LootCondition::AllOf(conditions) => conditions.iter().all(|c| c.test(ctx)),
            LootCondition::KilledByPlayer => ctx.killed_by_player,
            LootCondition::EntityProperties { entity, predicate } => {
                let Some(entity) = ctx.get_entity(*entity) else {
                    return false;
                };
                predicate.test(entity, ctx)
            }
            LootCondition::DamageSourceProperties { predicate } => predicate.test(ctx),
            LootCondition::LocationCheck {
                offset_x,
                offset_y,
                offset_z,
                predicate,
            } => {
                // Vanilla `LocationCheck.test`: the origin is required, an absent
                // predicate passes, and the predicate is resolved at the origin
                // shifted by the offset.
                let Some((x, y, z)) = ctx.origin else {
                    return false;
                };
                if location_predicate_is_unconstrained(predicate) {
                    return true;
                }
                let position = (
                    x + f64::from(*offset_x),
                    y + f64::from(*offset_y),
                    z + f64::from(*offset_z),
                );
                ctx.location
                    .is_some_and(|level| predicate.matches_at(level, position))
            }
            LootCondition::WeatherCheck {
                raining,
                thundering,
            } => {
                let weather = ctx.weather.unwrap_or_default();
                raining.is_none_or(|r| r == weather.raining)
                    && thundering.is_none_or(|t| t == weather.thundering)
            }
            LootCondition::TimeCheck { value, period } => {
                let game_time = ctx.game_time.unwrap_or(0);
                let time = if let Some(p) = period {
                    game_time % p
                } else {
                    game_time
                };
                value.test(time as f32, ctx.rng)
            }
            LootCondition::ValueCheck { value, range } => {
                let v = value.get_simple(ctx.rng);
                range.test(v, ctx.rng)
            }
            LootCondition::EnchantmentActiveCheck {
                enchantment,
                active,
            } => {
                let level = ctx.get_enchantment_level_by_id(enchantment);
                let is_active = level > 0;
                is_active == *active
            }
            LootCondition::EntityScores { entity, scores } => {
                // Vanilla `EntityHasScoreCondition.test` / `hasScore`: the target
                // entity must be present, every listed objective must exist, and
                // the target must hold a score that lies inside the range.
                if ctx.get_entity(*entity).is_none() {
                    return false;
                }
                let target = ScoreboardTarget::Context(*entity);
                scores.iter().all(|(objective, range)| {
                    score_of(ctx, &target, objective)
                        .is_some_and(|score| range.test(score as f32, ctx.rng))
                })
            }
            LootCondition::EnvironmentAttributeCheck { attribute, value } => {
                // Vanilla `EnvironmentAttributeCheck.test`: the attribute's value
                // is compared against the configured one; with no attribute source
                // installed the value is unavailable, so the check fails.
                ctx.environment_attributes
                    .and_then(|attributes| attributes.get_attribute(attribute, ctx.origin))
                    .is_some_and(|actual| actual == *value)
            }
            LootCondition::Reference(_name) => {
                // Steel inlines `minecraft:<path>` condition references at build
                // time (the loot-table build script substitutes the referenced
                // `predicate/` registry entry), so generated tables never contain
                // this variant. An unresolved reference fails closed rather than
                // silently passing.
                false
            }
        }
    }
}

/// Whether `predicate` constrains nothing, matching vanilla's absent
/// `LocationCheck.predicate` (an empty predicate always passes when an origin is
/// present).
const fn location_predicate_is_unconstrained(predicate: &LocationPredicate) -> bool {
    predicate.position.is_none()
        && predicate.biomes.is_none()
        && predicate.structures.is_none()
        && predicate.dimension.is_none()
        && predicate.light.is_none()
        && predicate.block.is_none()
        && predicate.fluid.is_none()
        && predicate.smokey.is_none()
        && predicate.can_see_sky.is_none()
}

impl ToolPredicate {
    /// Test if the tool matches this predicate.
    #[must_use]
    pub fn test<R: rand::Rng>(&self, tool: &ItemStack, _ctx: &LootContext<'_, R>) -> bool {
        match self {
            ToolPredicate::Item(item_id) => tool.item.key == *item_id,
            ToolPredicate::HasEnchantment {
                enchantment,
                min_level,
            } => tool_enchantment_matches(tool, enchantment, *min_level),
            ToolPredicate::Tag(tag) => {
                // Check if the tool's item is in the specified tag
                REGISTRY.items.is_in_tag(tool.item, tag)
            }
            ToolPredicate::Any => true,
        }
    }
}

fn tool_enchantment_matches(
    tool: &ItemStack,
    enchantment_or_tag: &Identifier,
    min_level: i32,
) -> bool {
    if tool.get_enchantment_level(enchantment_or_tag) >= min_level {
        return true;
    }

    let Some(enchantments) = tool.get_enchantments() else {
        return false;
    };

    for (key, level) in enchantments.iter() {
        if *level < min_level as u32 {
            continue;
        }
        let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
            continue;
        };
        if REGISTRY
            .enchantments
            .is_in_tag(enchantment, enchantment_or_tag)
        {
            return true;
        }
    }

    false
}

impl EntityPredicate {
    fn test<R: rand::Rng>(&self, entity: EntityRef<'_>, ctx: &LootContext<'_, R>) -> bool {
        if let Some(entity_type) = &self.entity_type
            && entity.entity_type != Some(entity_type)
        {
            return false;
        }

        if let Some(flags) = &self.flags
            && !flags.test(entity.flags)
        {
            return false;
        }

        if let Some(equipment) = &self.equipment
            && !equipment.test(entity.equipment, ctx)
        {
            return false;
        }

        if let Some(expected_color) = &self.sheep_color
            && entity.sheep_color != Some(*expected_color)
        {
            return false;
        }

        if let Some(expected_sheared) = &self.sheep_sheared
            && entity.sheep_sheared != Some(*expected_sheared)
        {
            return false;
        }

        if let Some(expected_variant) = &self.chicken_variant
            && entity.chicken_variant != Some(expected_variant)
        {
            return false;
        }

        true
    }
}

impl EntityFlags {
    fn test(&self, flags: EntityRefFlags) -> bool {
        self.is_on_fire
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
                .is_baby
                .is_none_or(|expected| expected == flags.is_baby)
    }
}

impl EntityEquipment {
    fn test<R: rand::Rng>(
        &self,
        equipment: Option<EntityEquipmentRef<'_>>,
        ctx: &LootContext<'_, R>,
    ) -> bool {
        let slot_predicates = self.slot_predicates();
        if slot_predicates
            .iter()
            .all(|(_, predicate)| predicate.is_none())
        {
            return true;
        }

        let Some(equipment) = equipment else {
            return false;
        };

        slot_predicates.into_iter().all(|(slot, predicate)| {
            predicate.is_none_or(|predicate| predicate.test(equipment.get(slot), ctx))
        })
    }

    const fn slot_predicates(&self) -> [(EquipmentSlot, Option<&ToolPredicate>); 6] {
        [
            (EquipmentSlot::MainHand, self.mainhand.as_ref()),
            (EquipmentSlot::OffHand, self.offhand.as_ref()),
            (EquipmentSlot::Head, self.head.as_ref()),
            (EquipmentSlot::Chest, self.chest.as_ref()),
            (EquipmentSlot::Legs, self.legs.as_ref()),
            (EquipmentSlot::Feet, self.feet.as_ref()),
        ]
    }
}

impl DamageSourcePredicate {
    fn test<R: rand::Rng>(&self, ctx: &LootContext<'_, R>) -> bool {
        let Some(damage_source) = ctx.damage_source else {
            return false;
        };

        for tag in self.tags {
            if damage_source_has_tag(damage_source, &tag.id) != tag.expected {
                return false;
            }
        }

        if let Some(expected) = self.is_direct
            && damage_source.is_direct != expected
        {
            return false;
        }

        if let Some(predicate) = &self.source_entity {
            let Some(entity) = ctx.killer_entity else {
                return false;
            };
            if !predicate.test(entity, ctx) {
                return false;
            }
        }

        if let Some(predicate) = &self.direct_entity {
            let Some(entity) = ctx.direct_killer_entity else {
                return false;
            };
            if !predicate.test(entity, ctx) {
                return false;
            }
        }

        true
    }
}

fn damage_source_has_tag(damage_source: DamageSourceInfo<'_>, tag: &Identifier) -> bool {
    if let Some(damage_type) = damage_source.damage_type
        && let Some(damage_type) = REGISTRY.damage_types.by_key(damage_type)
    {
        return REGISTRY.damage_types.is_in_tag(damage_type, tag);
    }

    damage_source.tags.iter().any(|candidate| candidate == tag)
}
