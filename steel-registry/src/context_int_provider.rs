//! `minecraft:context_int_provider` registry entries.
//!
//! Mirrors vanilla `net.minecraft.world.level.storage.loot.providers.number.ints`:
//! every provider type registered by `ContextIntProviderTypes` is one variant of
//! [`ContextIntProvider`], and a registry entry pairs a provider tree with the
//! datapack id it was defined under.
//!
//! Unlike [`crate::loot_table::NumberProvider`], which unifies both number
//! domains in one enum for loot-table evaluation, these enums keep vanilla's two
//! domains apart: an int provider only ever nests int providers (the sole bridge
//! is [`ContextIntProvider::FromFloat`]) and vice versa.
//!
//! Nested operands are `&'static` (and lists `&'static [..]`) because the trees
//! are compiled into the binary by the build script, following the same shape as
//! `crate::loot_table::NumberProvider`.

use rand::RngExt as _;
use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::context_float_provider::ContextFloatProvider;
use crate::loot_table::{LootCondition, LootContext, LootContextRef, ScoreboardTarget};

/// One entry of a `minecraft:number_dispatcher` provider: a condition and the
/// provider evaluated when it passes (vanilla `DispatcherProvider.Case`).
///
/// The value is a reference so that a case can share another registry entry's
/// provider tree instead of copying it.
#[derive(Debug)]
pub struct DispatcherCase<T: 'static> {
    /// Condition that selects this case.
    pub condition: &'static LootCondition,
    /// Value produced when the condition passes.
    pub value: &'static T,
}

/// One entry of a `minecraft:weighted_list` provider (vanilla
/// `WeightedList<Holder<..>>` entry).
#[derive(Debug)]
pub struct WeightedValue<T: 'static> {
    /// The provider (vanilla `data`).
    pub data: &'static T,
    /// Non-negative selection weight.
    pub weight: i32,
}

/// A vanilla `ContextIntProvider`: a provider tree that produces an `i32` from a
/// loot context.
///
/// Each variant is one type registered in `ContextIntProviderTypes`.
#[derive(Debug)]
pub enum ContextIntProvider {
    /// `minecraft:constant` (also the shape of a bare number in JSON).
    Constant(i32),
    /// `minecraft:uniform`: `Mth.nextInt` over the provider bounds (inclusive).
    Uniform {
        /// Lower bound.
        min: &'static ContextIntProvider,
        /// Upper bound.
        max: &'static ContextIntProvider,
    },
    /// `minecraft:binomial` with a constant trial count and success chance.
    Binomial {
        /// Number of trials.
        n: i32,
        /// Success chance of a single trial.
        p: f32,
    },
    /// `minecraft:binomial` whose trial count (int provider) and success chance
    /// (float provider) are computed by nested providers.
    BinomialProviders {
        /// Number of trials.
        n: &'static ContextIntProvider,
        /// Success chance of a single trial.
        p: &'static ContextFloatProvider,
    },
    /// `minecraft:add`: the sum of the nested providers.
    Sum {
        /// Summands.
        inputs: &'static [&'static ContextIntProvider],
    },
    /// `minecraft:avg`: the arithmetic mean of the nested providers.
    Average {
        /// Operands.
        inputs: &'static [&'static ContextIntProvider],
    },
    /// `minecraft:min`: the smallest nested provider value.
    Minimum {
        /// Operands.
        inputs: &'static [&'static ContextIntProvider],
    },
    /// `minecraft:max`: the largest nested provider value.
    Maximum {
        /// Operands.
        inputs: &'static [&'static ContextIntProvider],
    },
    /// `minecraft:mul`: the product of the nested providers.
    Product {
        /// Factors.
        inputs: &'static [&'static ContextIntProvider],
    },
    /// `minecraft:sub`: `left` minus `right` (`Math.subtractExact`).
    Difference {
        /// Minuend.
        left: &'static ContextIntProvider,
        /// Subtrahend.
        right: &'static ContextIntProvider,
    },
    /// `minecraft:div`: `left` divided by `right`.
    Quotient {
        /// Dividend.
        left: &'static ContextIntProvider,
        /// Divisor.
        right: &'static ContextIntProvider,
    },
    /// `minecraft:floor_div`: floored division (`Math.floorDivExact`).
    FloorQuotient {
        /// Dividend.
        left: &'static ContextIntProvider,
        /// Divisor.
        right: &'static ContextIntProvider,
    },
    /// `minecraft:mod`: the remainder of `left` divided by `right`.
    Modulus {
        /// Dividend.
        left: &'static ContextIntProvider,
        /// Divisor.
        right: &'static ContextIntProvider,
    },
    /// `minecraft:floor_mod`: the floored remainder (`Math.floorMod`).
    FloorModulus {
        /// Dividend.
        left: &'static ContextIntProvider,
        /// Divisor.
        right: &'static ContextIntProvider,
    },
    /// `minecraft:pow`: `base` raised to the power of `exponent`.
    Power {
        /// Base.
        base: &'static ContextIntProvider,
        /// Exponent.
        exponent: &'static ContextIntProvider,
    },
    /// `minecraft:negate`: the negated input (`Math.negateExact`).
    Negate {
        /// Operand.
        input: &'static ContextIntProvider,
    },
    /// `minecraft:abs`: the absolute value of the input (`Math.absExact`).
    Absolute {
        /// Operand.
        input: &'static ContextIntProvider,
    },
    /// `minecraft:from_float`: the input float provider converted by vanilla's
    /// `floatToIntSafe` (truncation, arithmetic error when not representable).
    FromFloat {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:conditional`: `on_true`/`on_false`, selected by a condition.
    Conditional {
        /// Condition deciding which branch is evaluated.
        condition: &'static LootCondition,
        /// Value used when the condition passes.
        on_true: &'static ContextIntProvider,
        /// Value used when the condition fails.
        on_false: &'static ContextIntProvider,
    },
    /// `minecraft:number_dispatcher`: the value of the first case whose condition
    /// passes, or the default value.
    NumberDispatcher {
        /// Condition/value pairs, tested in order.
        cases: &'static [DispatcherCase<ContextIntProvider>],
        /// Value used when no case matches.
        default: &'static ContextIntProvider,
    },
    /// `minecraft:weighted_list`: a randomly selected provider.
    WeightedList(&'static [WeightedValue<ContextIntProvider>]),
    /// `minecraft:score`: a scoreboard score, or `fallback` when unavailable.
    Scoreboard {
        /// Score holder to read.
        target: ScoreboardTarget,
        /// Objective name.
        score: &'static str,
        /// Value used when the score is unavailable; `0` by default.
        fallback: &'static ContextIntProvider,
    },
    /// `minecraft:storage`: a numeric command-storage tag, or `fallback` when the
    /// tag is missing or not numeric.
    Storage {
        /// Storage id (vanilla `storage`).
        storage: Identifier,
        /// NBT path of the tag (vanilla `path`).
        path: &'static str,
        /// Value used when the tag is unavailable; `0` by default.
        fallback: &'static ContextIntProvider,
    },
    /// `minecraft:environment_attribute`: an environment attribute read at the
    /// context's origin and converted to an int by its attribute type.
    EnvironmentAttribute {
        /// Environment attribute id.
        attribute: Identifier,
    },
}

impl ContextIntProvider {
    /// The constant value of a `minecraft:constant` provider.
    ///
    /// Vanilla data uses constant providers where no loot context exists (for
    /// example brewing-fuel `uses`); this avoids synthesising a throwaway
    /// `LootContext` for a value that cannot vary.
    #[must_use]
    pub const fn as_constant(&self) -> Option<i32> {
        match self {
            Self::Constant(value) => Some(*value),
            _ => None,
        }
    }

    /// Vanilla `ContextIntProvider.getInt`: an arithmetic error yields `0`.
    pub fn get_int<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> i32 {
        self.get_int_unsafe(ctx).unwrap_or(0)
    }

    /// Vanilla `ContextIntProvider.getIntUnsafe`: `None` stands for Java's
    /// `ArithmeticException` (overflow, division by zero, or a non-representable
    /// float conversion).
    pub fn get_int_unsafe<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> Option<i32> {
        match self {
            Self::Constant(value) => Some(*value),
            Self::Uniform { min, max } => {
                let min = min.get_int_unsafe(ctx)?;
                let max = max.get_int_unsafe(ctx)?;
                Some(uniform_int(&mut *ctx.rng, min, max))
            }
            Self::Binomial { n, p } => Some(binomial(&mut *ctx.rng, *n, *p)),
            Self::BinomialProviders { n, p } => {
                let trials = n.get_int_unsafe(ctx)?;
                let chance = p.get_float_or_throw(ctx)?;
                Some(binomial(&mut *ctx.rng, trials, chance))
            }
            Self::Sum { inputs } => {
                let mut total = 0_i64;
                for input in *inputs {
                    total += i64::from(input.get_int_unsafe(ctx)?);
                }
                long_to_int(total)
            }
            Self::Average { inputs } => {
                let mut total = 0_i64;
                let mut count = 0_i64;
                for input in *inputs {
                    total += i64::from(input.get_int_unsafe(ctx)?);
                    count += 1;
                }
                long_to_int(total.checked_div(count)?)
            }
            Self::Minimum { inputs } => {
                let mut value = i32::MAX;
                for input in *inputs {
                    value = value.min(input.get_int_unsafe(ctx)?);
                }
                Some(value)
            }
            Self::Maximum { inputs } => {
                let mut value = i32::MIN;
                for input in *inputs {
                    value = value.max(input.get_int_unsafe(ctx)?);
                }
                Some(value)
            }
            Self::Product { inputs } => {
                let mut value = 1_i64;
                for input in *inputs {
                    value *= i64::from(input.get_int_unsafe(ctx)?);
                }
                long_to_int(value)
            }
            Self::Difference { left, right } => left
                .get_int_unsafe(ctx)?
                .checked_sub(right.get_int_unsafe(ctx)?),
            Self::Quotient { left, right } => left
                .get_int_unsafe(ctx)?
                .checked_div(right.get_int_unsafe(ctx)?),
            Self::FloorQuotient { left, right } => left
                .get_int_unsafe(ctx)?
                .checked_div_euclid(right.get_int_unsafe(ctx)?),
            Self::Modulus { left, right } => left
                .get_int_unsafe(ctx)?
                .checked_rem(right.get_int_unsafe(ctx)?),
            Self::FloorModulus { left, right } => left
                .get_int_unsafe(ctx)?
                .checked_rem_euclid(right.get_int_unsafe(ctx)?),
            Self::Power { base, exponent } => {
                let base = base.get_int_unsafe(ctx)?;
                let exponent = exponent.get_int_unsafe(ctx)?;
                if base == 0 && exponent == 0 {
                    None
                } else {
                    pow_exact(base, exponent)
                }
            }
            Self::Negate { input } => input.get_int_unsafe(ctx)?.checked_neg(),
            Self::Absolute { input } => input.get_int_unsafe(ctx)?.checked_abs(),
            Self::FromFloat { input } => float_to_int(input.get_float_unsafe(ctx)),
            Self::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                if condition.test(ctx) {
                    on_true.get_int_unsafe(ctx)
                } else {
                    on_false.get_int_unsafe(ctx)
                }
            }
            Self::NumberDispatcher { cases, default } => {
                let mut selected = *default;
                for case in *cases {
                    if case.condition.test(ctx) {
                        selected = case.value;
                        break;
                    }
                }
                selected.get_int_unsafe(ctx)
            }
            Self::WeightedList(distribution) => match weighted_pick(*distribution, &mut *ctx.rng) {
                Some(provider) => provider.get_int_unsafe(ctx),
                None => Some(0),
            },
            Self::Scoreboard {
                target,
                score,
                fallback,
            } => match score_of(ctx, target, score) {
                Some(value) => Some(value),
                None => fallback.get_int_unsafe(ctx),
            },
            Self::Storage {
                storage,
                path,
                fallback,
            } => match stored_number(ctx, storage, path) {
                // Vanilla `Number.intValue()`: truncation towards zero.
                Some(value) => Some(value as i32),
                None => fallback.get_int_unsafe(ctx),
            },
            Self::EnvironmentAttribute { attribute } => Some(
                ctx.environment_attributes
                    .and_then(|attributes| attributes.get_attribute(attribute, ctx.origin))
                    .map_or(0, |value| value as i32),
            ),
        }
    }
}

/// A `minecraft:context_int_provider` registry entry: the datapack id paired
/// with the provider tree defined under it.
#[derive(Debug)]
pub struct ContextIntProviderEntry {
    /// Registry key (`minecraft:` is implied).
    pub key: Identifier,
    /// The provider tree.
    pub provider: &'static ContextIntProvider,
}

/// A reference to a registered int provider.
pub type ContextIntProviderRef = &'static ContextIntProviderEntry;

/// Registry of vanilla `minecraft:context_int_provider` entries.
pub struct ContextIntProviderRegistry {
    providers_by_id: Vec<ContextIntProviderRef>,
    providers_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl ContextIntProviderRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            providers_by_id: Vec::new(),
            providers_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    ContextIntProviderRegistry,
    ContextIntProviderRef,
    providers_by_id,
    providers_by_key,
    allows_registering
);

crate::impl_registry!(
    ContextIntProviderRegistry,
    ContextIntProviderEntry,
    providers_by_id,
    providers_by_key,
    context_int_providers
);

/// Vanilla `Mth.nextInt(random, min, max)`: inclusive, and `min` when
/// `min >= max`.
fn uniform_int(rng: &mut impl rand::Rng, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        rng.random_range(min..=max)
    }
}

/// Vanilla `BinomialDistributionGenerator`: `n` trials, each counting as a
/// success when a fresh `nextFloat` draw is below `p`.
fn binomial(rng: &mut impl rand::Rng, n: i32, p: f32) -> i32 {
    let mut result = 0;
    for _ in 0..n {
        if rng.random::<f32>() < p {
            result += 1;
        }
    }
    result
}

/// Vanilla `ContextIntProvider.longToIntSafe`: `None` when the value does not
/// fit in an `i32`.
fn long_to_int(value: i64) -> Option<i32> {
    i32::try_from(value).ok()
}

/// Vanilla `ContextIntProvider.floatToIntSafe`: `(int)` truncation, with a
/// non-finite or out-of-range value reported as an arithmetic error.
fn float_to_int(value: f32) -> Option<i32> {
    if !value.is_finite() {
        return None;
    }
    i32::try_from(value as i64).ok()
}

/// Java `Math.powExact(int, int)`: `None` for a negative exponent or an
/// overflowing result.
fn pow_exact(base: i32, exponent: i32) -> Option<i32> {
    let mut remaining = u32::try_from(exponent).ok()?;
    let mut factor = i64::from(base);
    let mut result = 1_i64;
    while remaining > 0 {
        if remaining & 1 == 1 {
            result = result.checked_mul(factor)?;
        }
        remaining >>= 1;
        if remaining > 0 {
            factor = factor.checked_mul(factor)?;
        }
    }
    long_to_int(result)
}

/// Vanilla `WeightedList.getRandomOrThrow`: a uniform draw over the summed
/// weights, then a scan subtracting each weight. `None` for a list whose weights
/// sum to zero.
///
/// Shared with [`crate::context_float_provider`].
pub(crate) fn weighted_pick<T: 'static>(
    distribution: &'static [WeightedValue<T>],
    rng: &mut impl rand::Rng,
) -> Option<&'static T> {
    let total: i64 = distribution
        .iter()
        .map(|value| i64::from(value.weight))
        .sum();
    if total <= 0 {
        return None;
    }
    let mut selection = rng.random_range(0..total);
    for value in distribution {
        selection -= i64::from(value.weight);
        if selection < 0 {
            return Some(value.data);
        }
    }
    None
}

/// The parameter view of `ctx` handed to the lookup hooks.
///
/// `LootContext::params` is private to the loot-table module, so the provider
/// modules build the view from the public context fields.
pub(crate) fn params_of<'a, R: rand::Rng>(ctx: &LootContext<'a, R>) -> LootContextRef<'a> {
    LootContextRef {
        tool: ctx.tool,
        scoreboard: ctx.scoreboard,
        command_storage: ctx.command_storage,
        environment_attributes: ctx.environment_attributes,
        enchantment_level: ctx.enchantment_level,
        this_entity: ctx.this_entity,
        killer_entity: ctx.killer_entity,
        direct_killer_entity: ctx.direct_killer_entity,
        last_damage_player: ctx.last_damage_player,
        interacting_entity: ctx.interacting_entity,
        origin: ctx.origin,
    }
}

/// Reads the score of a `minecraft:score` provider through the context's
/// scoreboard hook, if the server installed one.
fn score_of<R: rand::Rng>(
    ctx: &LootContext<'_, R>,
    target: &ScoreboardTarget,
    objective: &str,
) -> Option<i32> {
    let hook = ctx.scoreboard?;
    let params = params_of(ctx);
    hook.get_score(&params, target, objective)
}

/// Reads a numeric command-storage tag of a `minecraft:storage` provider through
/// the context's command-storage hook, if the server installed one.
///
/// Shared with [`crate::context_float_provider`].
pub(crate) fn stored_number<R: rand::Rng>(
    ctx: &LootContext<'_, R>,
    storage: &Identifier,
    path: &str,
) -> Option<f64> {
    ctx.command_storage?.get_numeric_tag(storage, path)
}
