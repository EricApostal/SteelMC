//! `enchantment_provider` registry entries, ported from vanilla
//! `net.minecraft.world.item.enchantment.providers`.
//!
//! These are the providers raiders, mob equipment and enderman loot use to roll
//! their enchantments (`EnchantmentHelper.enchantItemFromProvider`).
//!
//! **Deviation from vanilla:** vanilla registers these seven providers in code
//! (`VanillaEnchantmentProviders.bootstrap`) rather than from datapack JSON, and
//! Steel mirrors that with a static table instead of a registry mount, so
//! `Identifier`s are matched by key. Callers pass the difficulty special
//! multiplier (`DifficultyInstance.getSpecialMultiplier()`) explicitly to keep
//! this crate free of world types.

use std::sync::LazyLock;

use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use super::EnchantmentRef;
use super::table::{EnchantmentRandom, RandRngAdapter, enchant_item_from_provider_selection};
use crate::REGISTRY;
use crate::item_stack::ItemStack;
use crate::{TaggedRegistryExt, vanilla_enchantments};

/// Vanilla `IntProvider` subset used by vanilla provider data.
///
/// Vanilla registers `ConstantInt`, `UniformInt`, `BiasedToBottomInt`,
/// `ClampedInt`, `WeightedListInt` and `SumInt`; only the constant and uniform
/// forms appear in vanilla provider data, so only those are modelled.
#[derive(Debug, Clone, Copy)]
pub enum IntProvider {
    Constant(i32),
    Uniform { min: i32, max: i32 },
}

impl IntProvider {
    /// Vanilla `IntProvider.sample`.
    #[must_use]
    pub fn sample(&self, random: &mut impl EnchantmentRandom) -> i32 {
        match *self {
            Self::Constant(value) => value,
            Self::Uniform { min, max } => {
                // Vanilla `Mth.randomBetweenInclusive`.
                random.next_int(max - min + 1) + min
            }
        }
    }
}

/// Vanilla `HolderSet<Enchantment>`: an enchantment tag or a direct list.
#[derive(Debug, Clone)]
pub enum EnchantmentSet {
    Tag(Identifier),
    Direct(&'static [EnchantmentRef]),
}

impl EnchantmentSet {
    /// Vanilla `HolderSet::stream`.
    #[must_use]
    pub fn resolve(&self) -> Vec<EnchantmentRef> {
        match self {
            Self::Tag(tag) => REGISTRY.enchantments.get_tag(tag).unwrap_or_default(),
            Self::Direct(enchantments) => enchantments.to_vec(),
        }
    }
}

/// Vanilla `EnchantmentProvider`.
#[derive(Debug, Clone)]
pub enum EnchantmentProvider {
    /// Vanilla `EnchantmentsByCost`.
    ByCost {
        enchantments: EnchantmentSet,
        cost: IntProvider,
    },
    /// Vanilla `EnchantmentsByCostWithDifficulty`.
    ByCostWithDifficulty {
        enchantments: EnchantmentSet,
        min_cost: i32,
        max_cost_span: i32,
    },
    /// Vanilla `SingleEnchantment`.
    Single {
        enchantment: EnchantmentRef,
        level: IntProvider,
    },
}

impl EnchantmentProvider {
    /// Vanilla `EnchantmentProvider.enchant`: rolls this provider's
    /// enchantments and upgrades them onto `item`.
    ///
    /// `special_difficulty` is `DifficultyInstance.getSpecialMultiplier()`.
    pub fn enchant(
        &self,
        item: &mut ItemStack,
        special_difficulty: f32,
        random: &mut impl EnchantmentRandom,
    ) {
        match self {
            Self::ByCost { enchantments, cost } => {
                let cost = cost.sample(random);
                enchant_item_from_provider_selection(item, cost, &enchantments.resolve(), random);
            }
            Self::ByCostWithDifficulty {
                enchantments,
                min_cost,
                max_cost_span,
            } => {
                let span = (special_difficulty * *max_cost_span as f32) as i32;
                let cost = IntProvider::Uniform {
                    min: *min_cost,
                    max: *min_cost + span,
                }
                .sample(random);
                enchant_item_from_provider_selection(item, cost, &enchantments.resolve(), random);
            }
            Self::Single { enchantment, level } => {
                let level = level
                    .sample(random)
                    .clamp(enchantment.get_min_level(), enchantment.get_max_level());
                item.upgrade_enchantment((*enchantment).key.clone(), level as u32);
            }
        }
    }
}

/// Vanilla `VanillaEnchantmentProviders` bootstrap.
///
/// Keyed by provider `Identifier` instead of a registry id.
static VANILLA_ENCHANTMENT_PROVIDERS: LazyLock<FxHashMap<Identifier, EnchantmentProvider>> =
    LazyLock::new(|| {
        let mut providers = FxHashMap::default();
        providers.insert(
            Identifier::vanilla_static("mob_spawn_equipment"),
            EnchantmentProvider::ByCostWithDifficulty {
                enchantments: EnchantmentSet::Tag(Identifier::vanilla_static(
                    "on_mob_spawn_equipment",
                )),
                min_cost: 5,
                max_cost_span: 17,
            },
        );
        providers.insert(
            Identifier::vanilla_static("pillager_spawn_crossbow"),
            EnchantmentProvider::Single {
                enchantment: &vanilla_enchantments::PIERCING,
                level: IntProvider::Constant(1),
            },
        );
        providers.insert(
            Identifier::vanilla_static("raid/pillager_post_wave_3"),
            EnchantmentProvider::Single {
                enchantment: &vanilla_enchantments::QUICK_CHARGE,
                level: IntProvider::Constant(1),
            },
        );
        providers.insert(
            Identifier::vanilla_static("raid/pillager_post_wave_5"),
            EnchantmentProvider::Single {
                enchantment: &vanilla_enchantments::QUICK_CHARGE,
                level: IntProvider::Constant(2),
            },
        );
        providers.insert(
            Identifier::vanilla_static("raid/vindicator"),
            EnchantmentProvider::Single {
                enchantment: &vanilla_enchantments::SHARPNESS,
                level: IntProvider::Constant(1),
            },
        );
        providers.insert(
            Identifier::vanilla_static("raid/vindicator_post_wave_5"),
            EnchantmentProvider::Single {
                enchantment: &vanilla_enchantments::SHARPNESS,
                level: IntProvider::Constant(2),
            },
        );
        providers.insert(
            Identifier::vanilla_static("enderman_loot_drop"),
            EnchantmentProvider::Single {
                enchantment: &vanilla_enchantments::SILK_TOUCH,
                level: IntProvider::Constant(1),
            },
        );
        providers
    });

/// Vanilla provider lookup by key (`RegistryAccess.lookupOrThrow(ENCHANTMENT_PROVIDER)`).
#[must_use]
pub fn by_key(key: &Identifier) -> Option<&'static EnchantmentProvider> {
    VANILLA_ENCHANTMENT_PROVIDERS.get(key)
}

/// Vanilla `VanillaEnchantmentProviders.MOB_SPAWN_EQUIPMENT`.
pub fn mob_spawn_equipment() -> &'static EnchantmentProvider {
    by_key(&Identifier::vanilla_static("mob_spawn_equipment"))
        .expect("mob_spawn_equipment provider is registered")
}

/// Vanilla `VanillaEnchantmentProviders.PILLAGER_SPAWN_CROSSBOW`.
pub fn pillager_spawn_crossbow() -> &'static EnchantmentProvider {
    by_key(&Identifier::vanilla_static("pillager_spawn_crossbow"))
        .expect("pillager_spawn_crossbow provider is registered")
}

/// Vanilla `VanillaEnchantmentProviders.RAID_PILLAGER_POST_WAVE_3`.
pub fn raid_pillager_post_wave_3() -> &'static EnchantmentProvider {
    by_key(&Identifier::vanilla_static("raid/pillager_post_wave_3"))
        .expect("raid/pillager_post_wave_3 provider is registered")
}

/// Vanilla `VanillaEnchantmentProviders.RAID_PILLAGER_POST_WAVE_5`.
pub fn raid_pillager_post_wave_5() -> &'static EnchantmentProvider {
    by_key(&Identifier::vanilla_static("raid/pillager_post_wave_5"))
        .expect("raid/pillager_post_wave_5 provider is registered")
}

/// Vanilla `VanillaEnchantmentProviders.RAID_VINDICATOR`.
pub fn raid_vindicator() -> &'static EnchantmentProvider {
    by_key(&Identifier::vanilla_static("raid/vindicator"))
        .expect("raid/vindicator provider is registered")
}

/// Vanilla `VanillaEnchantmentProviders.RAID_VINDICATOR_POST_WAVE_5`.
pub fn raid_vindicator_post_wave_5() -> &'static EnchantmentProvider {
    by_key(&Identifier::vanilla_static("raid/vindicator_post_wave_5"))
        .expect("raid/vindicator_post_wave_5 provider is registered")
}

/// Vanilla `VanillaEnchantmentProviders.ENDERMAN_LOOT_DROP`.
pub fn enderman_loot_drop() -> &'static EnchantmentProvider {
    by_key(&Identifier::vanilla_static("enderman_loot_drop"))
        .expect("enderman_loot_drop provider is registered")
}

/// Vanilla `EnchantmentHelper.enchantItemFromProvider`: applies the provider to
/// the stack's enchantments in place.
pub fn enchant_item_from_provider(
    item: &mut ItemStack,
    provider_key: &Identifier,
    special_difficulty: f32,
    random: &mut impl EnchantmentRandom,
) {
    if let Some(provider) = by_key(provider_key) {
        provider.enchant(item, special_difficulty, random);
    }
}

/// Vanilla `EnchantmentHelper.enchantItemFromProvider` with a `rand::Rng` source,
/// for callers that thread Steel's loot/raid RNG.
pub fn enchant_item_from_provider_with_rng<R: rand::Rng>(
    item: &mut ItemStack,
    provider_key: &Identifier,
    special_difficulty: f32,
    random: &mut R,
) {
    enchant_item_from_provider(
        item,
        provider_key,
        special_difficulty,
        &mut RandRngAdapter(random),
    );
}

#[cfg(test)]
mod tests {
    use super::{EnchantmentProvider, IntProvider, by_key};
    use crate::item_stack::ItemStack;
    use crate::vanilla_items;
    use steel_utils::Identifier;

    #[test]
    fn single_provider_clamps_to_enchantment_bounds() {
        let mut item = ItemStack::new(&vanilla_items::IRON_SWORD);
        let mut random = rand::rng();

        EnchantmentProvider::Single {
            enchantment: &crate::vanilla_enchantments::SHARPNESS,
            level: IntProvider::Constant(99),
        }
        .enchant(
            &mut item,
            0.0,
            &mut crate::enchantment::table::RandRngAdapter(&mut random),
        );

        assert_eq!(
            item.get_enchantment_level(&crate::vanilla_enchantments::SHARPNESS.key),
            crate::vanilla_enchantments::SHARPNESS.max_level as i32
        );
    }

    #[test]
    fn vanilla_providers_are_keyed_by_identifier() {
        assert!(by_key(&Identifier::vanilla_static("mob_spawn_equipment")).is_some());
        assert!(by_key(&Identifier::vanilla_static("raid/pillager_post_wave_5")).is_some());
        assert!(by_key(&Identifier::vanilla_static("enderman_loot_drop")).is_some());
        assert!(by_key(&Identifier::vanilla_static("unknown_provider")).is_none());
    }
}
