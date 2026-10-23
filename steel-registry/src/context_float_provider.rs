//! `minecraft:context_float_provider` registry entries.
//!
//! Mirrors vanilla `net.minecraft.world.level.storage.loot.providers.number.floats`:
//! every provider type registered by `ContextFloatProviderTypes` is one variant of
//! [`ContextFloatProvider`], and a registry entry pairs a provider tree with the
//! datapack id it was defined under.
//!
//! See [`crate::context_int_provider`] for the int domain. The two enums keep
//! vanilla's domains apart: a float provider only ever nests float providers (the
//! sole bridge is [`ContextFloatProvider::FromInt`]).
//!
//! Nested operands are `&'static` (and lists `&'static [..]`) because the trees
//! are compiled into the binary by the build script.

use rand::RngExt as _;
use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::context_int_provider::{DispatcherCase, WeightedValue, stored_number, weighted_pick};
use crate::enchantment_effect::LevelBasedValue;
use crate::loot_table::{LootCondition, LootContext};

/// A vanilla `ContextFloatProvider`: a provider tree that produces an `f32` from
/// a loot context.
///
/// Each variant is one type registered in `ContextFloatProviderTypes`.
#[derive(Debug)]
pub enum ContextFloatProvider {
    /// `minecraft:constant` (also the shape of a bare number in JSON).
    Constant(f32),
    /// `minecraft:uniform`: `Mth.nextFloat` between the provider bounds.
    Uniform {
        /// Lower bound.
        min: &'static ContextFloatProvider,
        /// Upper bound.
        max: &'static ContextFloatProvider,
    },
    /// `minecraft:add`: the sum of the nested providers.
    Sum {
        /// Summands.
        inputs: &'static [&'static ContextFloatProvider],
    },
    /// `minecraft:avg`: the arithmetic mean of the nested providers.
    Average {
        /// Operands.
        inputs: &'static [&'static ContextFloatProvider],
    },
    /// `minecraft:min`: the smallest nested provider value.
    Minimum {
        /// Operands.
        inputs: &'static [&'static ContextFloatProvider],
    },
    /// `minecraft:max`: the largest nested provider value.
    Maximum {
        /// Operands.
        inputs: &'static [&'static ContextFloatProvider],
    },
    /// `minecraft:mul`: the product of the nested providers.
    Product {
        /// Factors.
        inputs: &'static [&'static ContextFloatProvider],
    },
    /// `minecraft:length`: the euclidean length (root of the sum of squares) of
    /// the nested providers.
    Length {
        /// Operands.
        inputs: &'static [&'static ContextFloatProvider],
    },
    /// `minecraft:sub`: `left` minus `right`.
    Difference {
        /// Minuend.
        left: &'static ContextFloatProvider,
        /// Subtrahend.
        right: &'static ContextFloatProvider,
    },
    /// `minecraft:div`: `left` divided by `right`.
    Quotient {
        /// Dividend.
        left: &'static ContextFloatProvider,
        /// Divisor.
        right: &'static ContextFloatProvider,
    },
    /// `minecraft:mod`: the remainder of `left` divided by `right` (`NaN` for a
    /// zero divisor).
    Modulus {
        /// Dividend.
        left: &'static ContextFloatProvider,
        /// Divisor.
        right: &'static ContextFloatProvider,
    },
    /// `minecraft:pow`: `base` raised to the power of `exponent` (`NaN` for
    /// `0^0`).
    Power {
        /// Base.
        base: &'static ContextFloatProvider,
        /// Exponent.
        exponent: &'static ContextFloatProvider,
    },
    /// `minecraft:negate`: the negated input.
    Negate {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:abs`: the absolute value of the input.
    Absolute {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:ceil`: `Mth.ceil` of the input.
    Ceiling {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:floor`: `Math.floor` of the input.
    Floor {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:round`: `Math.round` of the input.
    Round {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:truncate`: the input rounded towards zero.
    Truncate {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:sqrt`: the square root of the input.
    SquareRoot {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:sin`: the sine of the input, in radians.
    Sine {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:cos`: the cosine of the input, in radians.
    Cosine {
        /// Operand.
        input: &'static ContextFloatProvider,
    },
    /// `minecraft:from_int`: the input int provider converted to a float.
    FromInt {
        /// Operand.
        input: &'static crate::context_int_provider::ContextIntProvider,
    },
    /// `minecraft:conditional`: `on_true`/`on_false`, selected by a condition.
    Conditional {
        /// Condition deciding which branch is evaluated.
        condition: &'static LootCondition,
        /// Value used when the condition passes.
        on_true: &'static ContextFloatProvider,
        /// Value used when the condition fails.
        on_false: &'static ContextFloatProvider,
    },
    /// `minecraft:number_dispatcher`: the value of the first case whose condition
    /// passes, or the default value.
    NumberDispatcher {
        /// Condition/value pairs, tested in order.
        cases: &'static [DispatcherCase<ContextFloatProvider>],
        /// Value used when no case matches.
        default: &'static ContextFloatProvider,
    },
    /// `minecraft:weighted_list`: a randomly selected provider.
    WeightedList(&'static [WeightedValue<ContextFloatProvider>]),
    /// `minecraft:storage`: a numeric command-storage tag, or `fallback` when the
    /// tag is missing or not numeric.
    Storage {
        /// Storage id (vanilla `storage`).
        storage: Identifier,
        /// NBT path of the tag (vanilla `path`).
        path: &'static str,
        /// Value used when the tag is unavailable; `0` by default.
        fallback: &'static ContextFloatProvider,
    },
    /// `minecraft:environment_attribute`: an environment attribute read at the
    /// context's origin and converted to a float by its attribute type.
    EnvironmentAttribute {
        /// Environment attribute id.
        attribute: Identifier,
    },
    /// `minecraft:enchantment_level`: the `amount` formula evaluated at the
    /// context's enchantment level (`0` when the context has none).
    EnchantmentLevel {
        /// Value formula (vanilla `LevelBasedValue`).
        amount: &'static LevelBasedValue,
    },
}

impl ContextFloatProvider {
    /// The constant value of a `minecraft:constant` provider.
    ///
    /// Vanilla data uses constant providers where no loot context exists (for
    /// example brewing-fuel `speed`); this avoids synthesizing a throwaway
    /// `LootContext` for a value that cannot vary.
    #[must_use]
    pub const fn as_constant(&self) -> Option<f32> {
        match self {
            Self::Constant(value) => Some(*value),
            _ => None,
        }
    }

    /// Vanilla `ContextFloatProvider.getFloat`: a non-finite result yields `0`.
    pub fn get_float<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> f32 {
        let value = self.get_float_unsafe(ctx);
        if value.is_finite() { value } else { 0.0 }
    }

    /// Vanilla `ContextFloatProvider.getFloatUnsafe`: no finiteness check, so
    /// undefined results propagate as `NaN`/`inf`.
    pub fn get_float_unsafe<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform { min, max } => {
                let min = min.get_float_unsafe(ctx);
                let max = max.get_float_unsafe(ctx);
                mth_next_float(&mut *ctx.rng, min, max)
            }
            Self::Sum { inputs } => {
                let mut total = 0.0;
                for input in *inputs {
                    total += input.get_float_unsafe(ctx);
                }
                total
            }
            Self::Average { inputs } => {
                let mut total = 0.0;
                let mut count = 0_u32;
                for input in *inputs {
                    total += input.get_float_unsafe(ctx);
                    count += 1;
                }
                total / count as f32
            }
            Self::Minimum { inputs } => {
                let mut value = f32::MAX;
                for input in *inputs {
                    value = value.min(input.get_float_unsafe(ctx));
                }
                value
            }
            Self::Maximum { inputs } => {
                let mut value = -f32::MAX;
                for input in *inputs {
                    value = value.max(input.get_float_unsafe(ctx));
                }
                value
            }
            Self::Product { inputs } => {
                let mut value = 1.0;
                for input in *inputs {
                    value *= input.get_float_unsafe(ctx);
                }
                value
            }
            Self::Length { inputs } => {
                let mut sum_of_squares = 0.0;
                for input in *inputs {
                    let value = input.get_float_unsafe(ctx);
                    sum_of_squares += value * value;
                }
                sum_of_squares.sqrt()
            }
            Self::Difference { left, right } => {
                left.get_float_unsafe(ctx) - right.get_float_unsafe(ctx)
            }
            Self::Quotient { left, right } => {
                left.get_float_unsafe(ctx) / right.get_float_unsafe(ctx)
            }
            Self::Modulus { left, right } => {
                let divisor = right.get_float_unsafe(ctx);
                if divisor == 0.0 {
                    f32::NAN
                } else {
                    left.get_float_unsafe(ctx) % divisor
                }
            }
            Self::Power { base, exponent } => {
                let base = base.get_float_unsafe(ctx);
                let exponent = exponent.get_float_unsafe(ctx);
                if base == 0.0 && exponent == 0.0 {
                    f32::NAN
                } else {
                    base.powf(exponent)
                }
            }
            Self::Negate { input } => -input.get_float_unsafe(ctx),
            Self::Absolute { input } => input.get_float_unsafe(ctx).abs(),
            Self::Ceiling { input } => mth_ceil(input.get_float_unsafe(ctx)) as f32,
            Self::Floor { input } => input.get_float_unsafe(ctx).floor(),
            Self::Round { input } => math_round(input.get_float_unsafe(ctx)) as f32,
            Self::Truncate { input } => {
                let value = input.get_float_unsafe(ctx);
                if value > 0.0 {
                    value.floor()
                } else {
                    value.ceil()
                }
            }
            Self::SquareRoot { input } => input.get_float_unsafe(ctx).sqrt(),
            Self::Sine { input } => input.get_float_unsafe(ctx).sin(),
            Self::Cosine { input } => input.get_float_unsafe(ctx).cos(),
            Self::FromInt { input } => input.get_int_unsafe(ctx).map_or(0.0, |value| value as f32),
            Self::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                if condition.test(ctx) {
                    on_true.get_float_unsafe(ctx)
                } else {
                    on_false.get_float_unsafe(ctx)
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
                selected.get_float_unsafe(ctx)
            }
            Self::WeightedList(distribution) => match weighted_pick(*distribution, &mut *ctx.rng) {
                Some(provider) => provider.get_float_unsafe(ctx),
                None => 0.0,
            },
            Self::Storage {
                storage,
                path,
                fallback,
            } => match stored_number(ctx, storage, path) {
                Some(value) => value as f32,
                None => fallback.get_float_unsafe(ctx),
            },
            Self::EnvironmentAttribute { attribute } => ctx
                .environment_attributes
                .and_then(|attributes| attributes.get_attribute(attribute, ctx.origin))
                .unwrap_or(0.0),
            Self::EnchantmentLevel { amount } => {
                amount.calculate(ctx.enchantment_level.unwrap_or(0))
            }
        }
    }

    /// Vanilla `ContextFloatProvider.getFloatOrThrow`: `None` stands for Java's
    /// `ArithmeticException` for a non-finite value.
    pub fn get_float_or_throw<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> Option<f32> {
        let value = self.get_float_unsafe(ctx);
        value.is_finite().then_some(value)
    }
}

/// A `minecraft:context_float_provider` registry entry: the datapack id paired
/// with the provider tree defined under it.
#[derive(Debug)]
pub struct ContextFloatProviderEntry {
    /// Registry key (`minecraft:` is implied).
    pub key: Identifier,
    /// The provider tree.
    pub provider: &'static ContextFloatProvider,
}

/// A reference to a registered float provider.
pub type ContextFloatProviderRef = &'static ContextFloatProviderEntry;

/// Registry of vanilla `minecraft:context_float_provider` entries.
pub struct ContextFloatProviderRegistry {
    providers_by_id: Vec<ContextFloatProviderRef>,
    providers_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl ContextFloatProviderRegistry {
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
    ContextFloatProviderRegistry,
    ContextFloatProviderRef,
    providers_by_id,
    providers_by_key,
    allows_registering
);

crate::impl_registry!(
    ContextFloatProviderRegistry,
    ContextFloatProviderEntry,
    providers_by_id,
    providers_by_key,
    context_float_providers
);

/// Vanilla `Mth.nextFloat(random, min, max)`: `min` when `min >= max`, otherwise
/// `min + random * (max - min)`.
fn mth_next_float(rng: &mut impl rand::Rng, min: f32, max: f32) -> f32 {
    if min >= max {
        min
    } else {
        rng.random::<f32>() * (max - min) + min
    }
}

/// Vanilla `Mth.ceil(float)`: the `(int)` cast then a bump when the value is
/// larger, so `NaN` becomes `0`.
fn mth_ceil(value: f32) -> i32 {
    let truncated = value as i32;
    if value > truncated as f32 {
        truncated.wrapping_add(1)
    } else {
        truncated
    }
}

/// `java.lang.Math.round` semantics for a float.
fn math_round(value: f32) -> i32 {
    (f64::from(value) + 0.5).floor() as i32
}
