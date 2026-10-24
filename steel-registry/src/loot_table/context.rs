use super::{
    BlockStateId, DyeColor, Identifier, ItemStack, LootCondition, REGISTRY, RegistryExt, RngExt,
};
use crate::advancement::predicates::LevelView;
use crate::data_components::DataComponentMap;
use crate::enchantment::effect::LevelBasedValue;
use crate::equipment::EquipmentSlot;
use crate::game_profile::GameProfile;
use simdnbt::owned::NbtCompound;

/// Entity target for loot context lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LootContextEntity {
    /// The entity being looted (killed mob, block entity owner).
    This,
    /// The entity that killed the target.
    Killer,
    /// The direct attacker (e.g., arrow, not the player who shot it).
    DirectKiller,
    /// The player who dealt the final damage.
    KillerPlayer,
    /// The entity interacting with a block/entity.
    Interacting,
}

/// The type of loot table, determining when/how it's used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LootType {
    Block,
    Entity,
    Chest,
    Fishing,
    Gift,
    Archaeology,
    Vault,
    Shearing,
    Equipment,
    Selector,
    EntityInteract,
    BlockInteract,
    Barter,
}

/// A number provider: vanilla's loot number-provider tree (the
/// `ContextIntProvider` / `ContextFloatProvider` value types).
///
/// Every variant mirrors one vanilla provider type. Int-only and float-only
/// variants still evaluate in the other domain: an int-only provider used where a
/// float is expected yields its integer value, and a float-only provider used
/// where an int is expected goes through vanilla's `(int)` cast
/// (`ContextIntProvider.floatToIntSafe`), which is an arithmetic error for a
/// non-finite or out-of-range value.
#[derive(Debug, Clone)]
pub enum NumberProvider {
    /// `minecraft:constant` (also the shape of a bare number in JSON).
    Constant(f32),
    /// A `minecraft:<path>` reference to a `context_int_provider` /
    /// `context_float_provider` registry entry (vanilla `RegistryCodecs.holder`),
    /// resolved through [`NumberProviderLookup`]. Evaluates to `0` when no hook is
    /// installed or the entry is absent.
    Reference {
        /// Registry key of the referenced provider.
        key: Identifier,
    },
    /// `minecraft:uniform` with constant bounds: `Mth.nextInt` (inclusive) in
    /// integer contexts, `Mth.nextFloat` in float contexts.
    Uniform {
        /// Lower bound.
        min: f32,
        /// Upper bound.
        max: f32,
    },
    /// `minecraft:uniform` whose bounds are computed by nested providers.
    UniformProviders {
        /// Lower bound provider.
        min: &'static NumberProvider,
        /// Upper bound provider.
        max: &'static NumberProvider,
    },
    /// `minecraft:binomial` with constant `n`/`p`, the shape used by all vanilla
    /// data (vanilla stores the two sub-providers as `exactly(...)`).
    Binomial {
        /// Number of trials.
        n: i32,
        /// Success chance of a single trial.
        p: f32,
    },
    /// `minecraft:binomial` whose `n` (int provider) and `p` (float provider) are
    /// computed by nested providers.
    BinomialProviders {
        /// Number of trials.
        n: &'static NumberProvider,
        /// Success chance of a single trial.
        p: &'static NumberProvider,
    },
    /// `minecraft:add`: the sum of the nested providers.
    Add {
        /// Summands.
        inputs: &'static [NumberProvider],
    },
    /// `minecraft:avg`: the arithmetic mean of the nested providers.
    Average {
        /// Operands.
        inputs: &'static [NumberProvider],
    },
    /// `minecraft:min`: the smallest nested provider value.
    Minimum {
        /// Operands.
        inputs: &'static [NumberProvider],
    },
    /// `minecraft:max`: the largest nested provider value.
    Maximum {
        /// Operands.
        inputs: &'static [NumberProvider],
    },
    /// `minecraft:mul`: the product of the nested providers.
    Product {
        /// Factors.
        inputs: &'static [NumberProvider],
    },
    /// `minecraft:sub`: `left` minus `right` (`Math.subtractExact` on ints).
    Difference {
        /// Minuend.
        left: &'static NumberProvider,
        /// Subtrahend.
        right: &'static NumberProvider,
    },
    /// `minecraft:div`: `left` divided by `right`.
    Quotient {
        /// Dividend.
        left: &'static NumberProvider,
        /// Divisor.
        right: &'static NumberProvider,
    },
    /// `minecraft:floor_div`: floored division (`Math.floorDivExact` on ints).
    FloorQuotient {
        /// Dividend.
        left: &'static NumberProvider,
        /// Divisor.
        right: &'static NumberProvider,
    },
    /// `minecraft:mod`: the remainder of `left` divided by `right`.
    Modulus {
        /// Dividend.
        left: &'static NumberProvider,
        /// Divisor.
        right: &'static NumberProvider,
    },
    /// `minecraft:floor_mod`: the floored remainder (`Math.floorMod` on ints).
    FloorModulus {
        /// Dividend.
        left: &'static NumberProvider,
        /// Divisor.
        right: &'static NumberProvider,
    },
    /// `minecraft:pow`: `base` raised to the power of `exponent`.
    Power {
        /// Base.
        base: &'static NumberProvider,
        /// Exponent.
        exponent: &'static NumberProvider,
    },
    /// `minecraft:negate`: the negated input.
    Negate {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:abs`: the absolute value of the input.
    Absolute {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:ceil`: `Mth.ceil` of the input.
    Ceiling {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:floor`: `Math.floor` of the input.
    Floor {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:round`: `Math.round` of the input.
    Round {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:truncate`: the input rounded towards zero.
    Truncate {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:sqrt`: the square root of the input.
    SquareRoot {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:sin`: the sine of the input, in radians.
    Sine {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:cos`: the cosine of the input, in radians.
    Cosine {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:length`: the euclidean length (root of the sum of squares) of
    /// the nested providers.
    Length {
        /// Operands.
        inputs: &'static [NumberProvider],
    },
    /// `minecraft:from_float`: the input float provider converted to an int by
    /// truncation.
    FromFloat {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:from_int`: the input int provider converted to a float.
    FromInt {
        /// Operand.
        input: &'static NumberProvider,
    },
    /// `minecraft:conditional`: `on_true`/`on_false`, selected by a condition.
    Conditional {
        /// Condition deciding which branch is evaluated.
        condition: &'static LootCondition,
        /// Value used when the condition passes.
        on_true: &'static NumberProvider,
        /// Value used when the condition fails.
        on_false: &'static NumberProvider,
    },
    /// `minecraft:number_dispatcher`: the value of the first case whose condition
    /// passes, or the default value.
    Dispatcher {
        /// Condition/value pairs, tested in order.
        cases: &'static [(&'static LootCondition, NumberProvider)],
        /// Value used when no case matches.
        default: &'static NumberProvider,
    },
    /// `minecraft:weighted_list`: a randomly selected provider.
    WeightedList {
        /// Providers and their non-negative weights.
        distribution: &'static [(NumberProvider, u32)],
    },
    /// `minecraft:score`: a scoreboard score (`scale` multiplies it, matching the
    /// legacy field of the same name).
    Score {
        /// Score holder to read.
        target: ScoreboardTarget,
        /// Objective name.
        score: &'static str,
        /// Multiplier applied to the score; `1.0` for vanilla data.
        scale: f32,
        /// Value used when the score is unavailable; `0` by default.
        fallback: &'static NumberProvider,
    },
    /// `minecraft:storage`: a numeric command-storage tag.
    Storage {
        /// Storage id (vanilla `stored`).
        storage: Identifier,
        /// NBT path of the tag (vanilla `path`).
        path: &'static str,
        /// Value used when the tag is missing or not numeric; `0` by default.
        fallback: &'static NumberProvider,
    },
    /// `minecraft:enchantment_level`: the `amount` formula evaluated at the
    /// context's enchantment level (`0` when the context has none).
    EnchantmentLevel {
        /// Value formula (vanilla `LevelBasedValue`).
        amount: &'static LevelBasedValue,
    },
    /// `minecraft:environment_attribute`: an environment attribute read at the
    /// context's origin and converted to a number by its attribute type.
    EnvironmentAttribute {
        /// Environment attribute id.
        attribute: Identifier,
    },
}

/// The score holder read by a `minecraft:score` provider, mirroring vanilla
/// `ScoreboardNameProviders` (`minecraft:fixed` / `minecraft:context`).
#[derive(Debug, Clone, Copy)]
pub enum ScoreboardTarget {
    /// A fixed score holder name (`minecraft:fixed`).
    Fixed(&'static str),
    /// The score holder of a loot-context entity (`minecraft:context`).
    Context(LootContextEntity),
}

/// Read access to scoreboard scores for `minecraft:score` number providers.
///
/// The registry crate cannot reach the world's scoreboard, so the server installs
/// this hook through [`LootContext::with_scoreboard`]; with no hook installed
/// `minecraft:score` evaluates to its `fallback` provider (`0` by default).
pub trait ScoreboardValueLookup {
    /// Score of `objective` for `target`, or `None` when the target entity is
    /// absent, the objective is unknown, or no score is tracked — vanilla
    /// `ScoreboardValue` then falls back to its `fallback` provider.
    fn get_score(
        &self,
        ctx: &LootContextRef<'_>,
        target: &ScoreboardTarget,
        objective: &str,
    ) -> Option<i32>;
}

/// Read access to numeric command-storage tags for `minecraft:storage` number
/// providers.
///
/// Install it through [`LootContext::with_command_storage`]; with no hook
/// installed `minecraft:storage` evaluates to its `fallback` provider
/// (`0` by default).
pub trait CommandStorageLookup {
    /// The single numeric tag at `path` (an NBT path) inside `storage`, or `None`
    /// when no such tag exists or it is not numeric — vanilla `StoredNumberAccess`
    /// then falls back to its `fallback` provider.
    fn get_numeric_tag(&self, storage: &Identifier, path: &str) -> Option<f64>;
}

/// Resolution of `minecraft:<path>` provider references (vanilla
/// `RegistryCodecs.holder` against the `context_int_provider` /
/// `context_float_provider` registries).
///
/// Install it through [`LootContext::with_number_providers`]; with no hook
/// installed, or when the key is absent, a reference evaluates to `0`.
pub trait NumberProviderLookup {
    /// The provider behind `key`, or `None` when the registry has no such entry.
    fn get_number_provider(&self, key: &Identifier) -> Option<&'static NumberProvider>;
}

/// Read access to environment attributes for `minecraft:environment_attribute`
/// number providers.
///
/// Install it through [`LootContext::with_environment_attributes`]; with no hook
/// installed `minecraft:environment_attribute` evaluates to `0`.
pub trait EnvironmentAttributeLookup {
    /// The attribute's value at `origin`, converted to a float by the attribute
    /// type (vanilla `AttributeType.toFloat`), or `None` when the attribute or the
    /// level state is unavailable.
    fn get_attribute(&self, attribute: &Identifier, origin: Option<(f64, f64, f64)>)
    -> Option<f32>;
}

impl NumberProvider {
    /// Get a value from this provider using the given RNG, with float semantics.
    ///
    /// Providers that need context parameters which are absent from `ctx` (or from
    /// a `None` context) behave as if those parameters were absent: conditions are
    /// unmet and scores, storage tags, and environment attributes are unavailable.
    pub fn get<R: rand::Rng>(&self, rng: &mut R, ctx: Option<&LootContextRef<'_>>) -> f32 {
        let mut context = match ctx {
            Some(params) => params.to_context(rng),
            None => LootContext::new(rng),
        };
        self.eval_float(&mut context)
    }

    /// Get a value using the loot context's RNG and parameters.
    pub fn get_float<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> f32 {
        self.eval_float(ctx)
    }

    /// Get the value as an integer using the full loot context, with integer
    /// semantics.
    ///
    /// Unlike [`NumberProvider::get_int_with_ctx`], which only receives the
    /// parameter view, this carries the lookup hooks and entity parameters of
    /// `ctx`, so scores, storage tags, environment attributes, and provider
    /// references resolve instead of collapsing to `0`.
    pub fn get_int_with_context<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> i32 {
        self.eval_int(ctx)
    }

    /// Get a value without context (for backwards compatibility).
    pub fn get_simple(&self, rng: &mut impl rand::Rng) -> f32 {
        let mut context = LootContext::new(rng);
        self.eval_float(&mut context)
    }

    /// Get the value as an integer, using integer semantics.
    pub fn get_int<R: rand::Rng>(&self, rng: &mut R) -> i32 {
        let mut context = LootContext::new(rng);
        self.eval_int(&mut context)
    }

    /// Get the value as an integer with context, using integer semantics.
    pub fn get_int_with_ctx<R: rand::Rng>(
        &self,
        rng: &mut R,
        ctx: Option<&LootContextRef<'_>>,
    ) -> i32 {
        let mut context = match ctx {
            Some(params) => params.to_context(rng),
            None => LootContext::new(rng),
        };
        self.eval_int(&mut context)
    }

    /// Vanilla `ContextFloatProvider.getFloat`: a non-finite result collapses to `0`.
    fn eval_float<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> f32 {
        let value = self.eval_float_unsafe(ctx);
        if value.is_finite() { value } else { 0.0 }
    }

    /// Vanilla `ContextIntProvider.getInt`: an arithmetic error collapses to `0`.
    fn eval_int<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> i32 {
        self.eval_int_unsafe(ctx).unwrap_or(0)
    }

    /// Vanilla `ContextFloatProvider.getFloatUnsafe`: no finiteness check, so
    /// undefined results propagate as `NaN`/`inf`.
    fn eval_float_unsafe<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> f32 {
        match self {
            Self::Constant(value) => *value,
            Self::Reference { key } => {
                referenced(ctx, key).map_or(0.0, |provider| provider.eval_float_unsafe(ctx))
            }
            Self::Uniform { min, max } => mth_next_float(ctx.rng, *min, *max),
            Self::UniformProviders { min, max } => {
                let min = min.eval_float_unsafe(ctx);
                let max = max.eval_float_unsafe(ctx);
                mth_next_float(ctx.rng, min, max)
            }
            Self::Binomial { n, p } => binomial(ctx.rng, *n, *p) as f32,
            Self::BinomialProviders { n, p } => {
                let trials = n.eval_int_unsafe(ctx).unwrap_or(0);
                let chance = p.eval_float_or_throw(ctx).unwrap_or(0.0);
                binomial(ctx.rng, trials, chance) as f32
            }
            Self::Add { inputs } => {
                let mut total = 0.0;
                for input in *inputs {
                    total += input.eval_float_unsafe(ctx);
                }
                total
            }
            Self::Average { inputs } => {
                let mut total = 0.0;
                let mut count = 0_u32;
                for input in *inputs {
                    total += input.eval_float_unsafe(ctx);
                    count += 1;
                }
                total / count as f32
            }
            Self::Minimum { inputs } => {
                let mut value = f32::MAX;
                for input in *inputs {
                    value = value.min(input.eval_float_unsafe(ctx));
                }
                value
            }
            Self::Maximum { inputs } => {
                let mut value = -f32::MAX;
                for input in *inputs {
                    value = value.max(input.eval_float_unsafe(ctx));
                }
                value
            }
            Self::Product { inputs } => {
                let mut value = 1.0;
                for input in *inputs {
                    value *= input.eval_float_unsafe(ctx);
                }
                value
            }
            Self::Difference { left, right } => {
                left.eval_float_unsafe(ctx) - right.eval_float_unsafe(ctx)
            }
            Self::Quotient { left, right } => {
                left.eval_float_unsafe(ctx) / right.eval_float_unsafe(ctx)
            }
            Self::Modulus { left, right } => {
                let divisor = right.eval_float_unsafe(ctx);
                if divisor == 0.0 {
                    f32::NAN
                } else {
                    left.eval_float_unsafe(ctx) % divisor
                }
            }
            Self::Power { base, exponent } => {
                let base = base.eval_float_unsafe(ctx);
                let exponent = exponent.eval_float_unsafe(ctx);
                if base == 0.0 && exponent == 0.0 {
                    f32::NAN
                } else {
                    base.powf(exponent)
                }
            }
            Self::Negate { input } => -input.eval_float_unsafe(ctx),
            Self::Absolute { input } => input.eval_float_unsafe(ctx).abs(),
            Self::Ceiling { input } => input.eval_float_unsafe(ctx).ceil() as i32 as f32,
            Self::Floor { input } => input.eval_float_unsafe(ctx).floor(),
            Self::Round { input } => math_round(input.eval_float_unsafe(ctx)) as f32,
            Self::Truncate { input } => {
                let value = input.eval_float_unsafe(ctx);
                if value > 0.0 {
                    value.floor()
                } else {
                    value.ceil()
                }
            }
            Self::SquareRoot { input } => input.eval_float_unsafe(ctx).sqrt(),
            Self::Sine { input } => input.eval_float_unsafe(ctx).sin(),
            Self::Cosine { input } => input.eval_float_unsafe(ctx).cos(),
            Self::Length { inputs } => {
                let mut sum_of_squares = 0.0;
                for input in *inputs {
                    let value = input.eval_float_unsafe(ctx);
                    sum_of_squares += value * value;
                }
                sum_of_squares.sqrt()
            }
            Self::FromInt { input } => input.eval_int_unsafe(ctx).map_or(0.0, |value| value as f32),
            Self::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                if condition.test(ctx) {
                    on_true.eval_float_unsafe(ctx)
                } else {
                    on_false.eval_float_unsafe(ctx)
                }
            }
            Self::Dispatcher { cases, default } => {
                let mut selected = *default;
                for (condition, value) in *cases {
                    if condition.test(ctx) {
                        selected = value;
                        break;
                    }
                }
                selected.eval_float_unsafe(ctx)
            }
            Self::WeightedList { distribution } => match weighted_pick(*distribution, ctx.rng) {
                Some(provider) => provider.eval_float_unsafe(ctx),
                None => 0.0,
            },
            Self::Score {
                target,
                score,
                scale,
                fallback,
            } => match score_of(ctx, target, score) {
                Some(value) => value as f32 * *scale,
                None => fallback.eval_float_unsafe(ctx),
            },
            Self::Storage {
                storage,
                path,
                fallback,
            } => match stored_number(ctx, storage, path) {
                Some(value) => value as f32,
                None => fallback.eval_float_unsafe(ctx),
            },
            Self::EnchantmentLevel { amount } => {
                amount.calculate(ctx.enchantment_level.unwrap_or(0))
            }
            Self::EnvironmentAttribute { attribute } => ctx
                .environment_attributes
                .and_then(|attributes| attributes.get_attribute(attribute, ctx.origin))
                .unwrap_or(0.0),
            // Int-only providers, used where a float is expected: their integer value.
            Self::FloorQuotient { .. } | Self::FloorModulus { .. } | Self::FromFloat { .. } => {
                self.eval_int_unsafe(ctx).map_or(0.0, |value| value as f32)
            }
        }
    }

    /// Vanilla `ContextIntProvider.getIntUnsafe`: `None` stands for Java's
    /// `ArithmeticException`.
    fn eval_int_unsafe<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> Option<i32> {
        match self {
            Self::Constant(value) => Some(math_round(*value)),
            Self::Reference { key } => {
                referenced(ctx, key).map_or(Some(0), |provider| provider.eval_int_unsafe(ctx))
            }
            Self::Uniform { min, max } => {
                Some(uniform_int(ctx.rng, math_round(*min), math_round(*max)))
            }
            Self::UniformProviders { min, max } => {
                let min = min.eval_int_unsafe(ctx)?;
                let max = max.eval_int_unsafe(ctx)?;
                Some(uniform_int(ctx.rng, min, max))
            }
            Self::Binomial { n, p } => Some(binomial(ctx.rng, *n, *p)),
            Self::BinomialProviders { n, p } => {
                let trials = n.eval_int_unsafe(ctx)?;
                let chance = p.eval_float_or_throw(ctx)?;
                Some(binomial(ctx.rng, trials, chance))
            }
            Self::Add { inputs } => {
                let mut total = 0_i64;
                for input in *inputs {
                    // Vanilla accumulates in a `long`, which wraps on overflow.
                    total = total.wrapping_add(i64::from(input.eval_int_unsafe(ctx)?));
                }
                long_to_int(total)
            }
            Self::Average { inputs } => {
                let mut total = 0_i64;
                let mut count = 0_i64;
                for input in *inputs {
                    total = total.wrapping_add(i64::from(input.eval_int_unsafe(ctx)?));
                    count += 1;
                }
                long_to_int(total.checked_div(count)?)
            }
            Self::Minimum { inputs } => {
                let mut value = i32::MAX;
                for input in *inputs {
                    value = value.min(input.eval_int_unsafe(ctx)?);
                }
                Some(value)
            }
            Self::Maximum { inputs } => {
                let mut value = i32::MIN;
                for input in *inputs {
                    value = value.max(input.eval_int_unsafe(ctx)?);
                }
                Some(value)
            }
            Self::Product { inputs } => {
                let mut value = 1_i64;
                for input in *inputs {
                    // Vanilla accumulates in a `long`, which wraps on overflow.
                    value = value.wrapping_mul(i64::from(input.eval_int_unsafe(ctx)?));
                }
                long_to_int(value)
            }
            Self::Difference { left, right } => left
                .eval_int_unsafe(ctx)?
                .checked_sub(right.eval_int_unsafe(ctx)?),
            Self::Quotient { left, right } => left
                .eval_int_unsafe(ctx)?
                .checked_div(right.eval_int_unsafe(ctx)?),
            Self::FloorQuotient { left, right } => left
                .eval_int_unsafe(ctx)?
                .checked_div_euclid(right.eval_int_unsafe(ctx)?),
            Self::Modulus { left, right } => left
                .eval_int_unsafe(ctx)?
                .checked_rem(right.eval_int_unsafe(ctx)?),
            Self::FloorModulus { left, right } => left
                .eval_int_unsafe(ctx)?
                .checked_rem_euclid(right.eval_int_unsafe(ctx)?),
            Self::Power { base, exponent } => {
                let base = base.eval_int_unsafe(ctx)?;
                let exponent = exponent.eval_int_unsafe(ctx)?;
                if base == 0 && exponent == 0 {
                    None
                } else {
                    pow_exact(base, exponent)
                }
            }
            Self::Negate { input } => input.eval_int_unsafe(ctx)?.checked_neg(),
            Self::Absolute { input } => input.eval_int_unsafe(ctx)?.checked_abs(),
            Self::FromFloat { input } => float_to_int(input.eval_float_unsafe(ctx)),
            Self::FromInt { input } => input.eval_int_unsafe(ctx),
            Self::Conditional {
                condition,
                on_true,
                on_false,
            } => {
                if condition.test(ctx) {
                    on_true.eval_int_unsafe(ctx)
                } else {
                    on_false.eval_int_unsafe(ctx)
                }
            }
            Self::Dispatcher { cases, default } => {
                let mut selected = *default;
                for (condition, value) in *cases {
                    if condition.test(ctx) {
                        selected = value;
                        break;
                    }
                }
                selected.eval_int_unsafe(ctx)
            }
            Self::WeightedList { distribution } => match weighted_pick(*distribution, ctx.rng) {
                Some(provider) => provider.eval_int_unsafe(ctx),
                None => Some(0),
            },
            Self::Score {
                target,
                score,
                scale,
                fallback,
            } => match score_of(ctx, target, score) {
                Some(value) => Some(math_round(value as f32 * *scale)),
                None => fallback.eval_int_unsafe(ctx),
            },
            Self::Storage {
                storage,
                path,
                fallback,
            } => match stored_number(ctx, storage, path) {
                // Vanilla `Number.intValue()`: truncation towards zero.
                Some(value) => Some(value as i32),
                None => fallback.eval_int_unsafe(ctx),
            },
            Self::EnchantmentLevel { amount } => Some(math_round(
                amount.calculate(ctx.enchantment_level.unwrap_or(0)),
            )),
            Self::EnvironmentAttribute { attribute } => Some(
                ctx.environment_attributes
                    .and_then(|attributes| attributes.get_attribute(attribute, ctx.origin))
                    .map_or(0, |value| value as i32),
            ),
            // Float-only providers, used where an int is expected: vanilla's `(int)` cast.
            other => float_to_int(other.eval_float_unsafe(ctx)),
        }
    }

    /// Vanilla `ContextFloatProvider.getFloatOrThrow`: `None` stands for Java's
    /// `ArithmeticException` for a non-finite value.
    fn eval_float_or_throw<R: rand::Rng>(&self, ctx: &mut LootContext<'_, R>) -> Option<f32> {
        let value = self.eval_float_unsafe(ctx);
        value.is_finite().then_some(value)
    }
}

/// `java.lang.Math.round` semantics for a float.
pub(crate) fn math_round(value: f32) -> i32 {
    (f64::from(value) + 0.5).floor() as i32
}

/// Vanilla `Mth.nextInt(random, min, max)` is inclusive and clamps to `min`
/// when `min >= max`.
fn uniform_int(rng: &mut impl rand::Rng, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        rng.random_range(min..=max)
    }
}

/// Vanilla `Mth.nextFloat(random, min, max)`: `min` when `min >= max`, otherwise
/// `min + random * (max - min)`.
fn mth_next_float(rng: &mut impl rand::Rng, min: f32, max: f32) -> f32 {
    if min >= max {
        min
    } else {
        rng.random::<f32>() * (max - min) + min
    }
}

/// Vanilla `BinomialDistributionGenerator`: `n` trials, each counting as a success
/// when a fresh `nextFloat` draw is below `p`.
fn binomial(rng: &mut impl rand::Rng, n: i32, p: f32) -> i32 {
    let mut result = 0;
    for _ in 0..n {
        if rng.random::<f32>() < p {
            result += 1;
        }
    }
    result
}

/// Vanilla `ContextIntProvider.longToIntSafe`: `None` when the value does not fit.
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

/// Java `Math.powExact(int, int)`: `base` raised to `exponent`, with `None` for
/// the cases Java throws on (negative exponent or an overflowing result).
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

/// Picks an entry from a weighted list (vanilla `WeightedList.getRandomOrThrow`):
/// a uniform draw over the summed weights, then a scan subtracting each weight.
/// Returns `None` for a list whose weights sum to zero.
fn weighted_pick(
    distribution: &'static [(NumberProvider, u32)],
    rng: &mut impl rand::Rng,
) -> Option<&'static NumberProvider> {
    let total: i64 = distribution
        .iter()
        .map(|(_, weight)| i64::from(*weight))
        .sum();
    if total <= 0 {
        return None;
    }
    let mut selection = rng.random_range(0..total);
    for (provider, weight) in distribution {
        selection -= i64::from(*weight);
        if selection < 0 {
            return Some(provider);
        }
    }
    None
}

/// Resolves a `minecraft:<path>` provider reference through the context's
/// [`NumberProviderLookup`], if the server installed one.
fn referenced<'r, R: rand::Rng>(
    ctx: &'r LootContext<'_, R>,
    key: &Identifier,
) -> Option<&'static NumberProvider> {
    ctx.number_providers?.get_number_provider(key)
}

/// Reads the score of a `minecraft:score` provider through the context's
/// [`ScoreboardValueLookup`], if the server installed one.
pub(super) fn score_of<R: rand::Rng>(
    ctx: &LootContext<'_, R>,
    target: &ScoreboardTarget,
    objective: &str,
) -> Option<i32> {
    let hook = ctx.scoreboard?;
    let params = ctx.params();
    hook.get_score(&params, target, objective)
}

/// Reads a numeric command-storage tag of a `minecraft:storage` provider through
/// the context's [`CommandStorageLookup`], if the server installed one.
fn stored_number<R: rand::Rng>(
    ctx: &LootContext<'_, R>,
    storage: &Identifier,
    path: &str,
) -> Option<f64> {
    ctx.command_storage?.get_numeric_tag(storage, path)
}

/// A range for number comparisons (used in `ValueCheck`, `TimeCheck`, `EntityScores`).
#[derive(Debug, Clone)]
pub struct NumberProviderRange {
    pub min: Option<NumberProvider>,
    pub max: Option<NumberProvider>,
}

impl NumberProviderRange {
    /// Check if a value is within this range.
    pub fn test(&self, value: f32, rng: &mut impl rand::Rng) -> bool {
        if let Some(min) = &self.min
            && value < min.get_simple(rng)
        {
            return false;
        }
        if let Some(max) = &self.max
            && value > max.get_simple(rng)
        {
            return false;
        }
        true
    }

    /// Create an exact match range.
    #[must_use]
    pub const fn exact(value: f32) -> Self {
        Self {
            min: Some(NumberProvider::Constant(value)),
            max: Some(NumberProvider::Constant(value)),
        }
    }

    /// Create an at-least range.
    #[must_use]
    pub const fn at_least(min: f32) -> Self {
        Self {
            min: Some(NumberProvider::Constant(min)),
            max: None,
        }
    }

    /// Create an at-most range.
    #[must_use]
    pub const fn at_most(max: f32) -> Self {
        Self {
            min: None,
            max: Some(NumberProvider::Constant(max)),
        }
    }

    /// Create a between range.
    #[must_use]
    pub const fn between(min: f32, max: f32) -> Self {
        Self {
            min: Some(NumberProvider::Constant(min)),
            max: Some(NumberProvider::Constant(max)),
        }
    }
}

/// The loot-context parameters a number provider can read, without the RNG.
///
/// This is a simplified, copyable view of [`LootContext`] that can be handed to
/// the lookup hooks ([`ScoreboardValueLookup`], [`CommandStorageLookup`],
/// [`EnvironmentAttributeLookup`]) without borrowing the random source.
#[derive(Clone, Copy)]
pub struct LootContextRef<'a> {
    /// The tool used to break the block or kill the entity.
    pub tool: Option<&'a ItemStack>,
    /// Scoreboard lookup; with no hook installed `minecraft:score` yields `0`.
    pub scoreboard: Option<&'a dyn ScoreboardValueLookup>,
    /// Command-storage lookup; with no hook installed `minecraft:storage` yields `0`.
    pub command_storage: Option<&'a dyn CommandStorageLookup>,
    /// Environment-attribute lookup; with no hook installed
    /// `minecraft:environment_attribute` yields `0`.
    pub environment_attributes: Option<&'a dyn EnvironmentAttributeLookup>,
    /// Vanilla `LootContextParams.ENCHANTMENT_LEVEL`, read by
    /// `minecraft:enchantment_level` (`0` when absent).
    pub enchantment_level: Option<i32>,
    /// The entity being looted.
    pub this_entity: Option<EntityRef<'a>>,
    /// The entity that killed the target.
    pub killer_entity: Option<EntityRef<'a>>,
    /// The direct attacker (e.g. the arrow, not the player who shot it).
    pub direct_killer_entity: Option<EntityRef<'a>>,
    /// The player who dealt the final damage.
    pub last_damage_player: Option<EntityRef<'a>>,
    /// The entity interacting with a block or entity.
    pub interacting_entity: Option<EntityRef<'a>>,
    /// Vanilla `LootContextParams.ORIGIN`, used by positional environment attributes.
    pub origin: Option<(f64, f64, f64)>,
}

impl<'a> LootContextRef<'a> {
    /// A parameter view that carries nothing but the hooks of `self`, backed by
    /// `rng` (used by the context-free entry points such as
    /// [`NumberProvider::get_simple`]).
    fn to_context<'r, R: rand::Rng>(&self, rng: &'r mut R) -> LootContext<'r, R>
    where
        'a: 'r,
    {
        let mut ctx = LootContext::new(rng);
        ctx.tool = self.tool;
        ctx.scoreboard = self.scoreboard;
        ctx.command_storage = self.command_storage;
        ctx.environment_attributes = self.environment_attributes;
        ctx.enchantment_level = self.enchantment_level;
        ctx.this_entity = self.this_entity;
        ctx.killer_entity = self.killer_entity;
        ctx.direct_killer_entity = self.direct_killer_entity;
        ctx.last_damage_player = self.last_damage_player;
        ctx.interacting_entity = self.interacting_entity;
        ctx.origin = self.origin;
        ctx
    }

    /// The entity for `target`, mirroring [`LootContext::get_entity`].
    #[must_use]
    pub const fn get_entity(&self, target: LootContextEntity) -> Option<EntityRef<'a>> {
        match target {
            LootContextEntity::This => self.this_entity,
            LootContextEntity::Killer => self.killer_entity,
            LootContextEntity::DirectKiller => self.direct_killer_entity,
            LootContextEntity::KillerPlayer => self.last_damage_player,
            LootContextEntity::Interacting => self.interacting_entity,
        }
    }
}

/// Context for loot table evaluation, containing all relevant game state.
///
/// This mirrors vanilla's `LootContext` / `LootParams` system.
pub struct LootContext<'a, R: rand::Rng> {
    /// Random number generator.
    pub rng: &'a mut R,
    /// Luck value (e.g., from Luck of the Sea enchantment).
    pub luck: f32,
    /// The block state being broken (for block loot tables).
    pub block_state: Option<BlockStateId>,
    /// The tool used to break the block.
    pub tool: Option<&'a ItemStack>,
    /// Explosion radius if caused by an explosion.
    pub explosion_radius: Option<f32>,
    /// Whether the entity was killed by a player.
    pub killed_by_player: bool,

    /// World position where the loot is generated (block position or entity death location).
    pub origin: Option<(f64, f64, f64)>,
    /// Current game time in ticks (for `TimeCheck` condition).
    pub game_time: Option<i64>,
    /// Current weather state.
    pub weather: Option<WeatherState>,
    /// The entity being looted (the killed mob, block entity owner, etc.).
    /// This is a type-erased pointer; actual entity data depends on game implementation.
    pub this_entity: Option<EntityRef<'a>>,
    /// The entity that killed `this_entity` (for mob loot tables).
    pub killer_entity: Option<EntityRef<'a>>,
    /// The direct attacker entity (e.g., arrow, not the player who shot it).
    pub direct_killer_entity: Option<EntityRef<'a>>,
    /// The player who dealt the final damage (may be different from killer).
    pub last_damage_player: Option<EntityRef<'a>>,
    /// Damage source information for entity deaths.
    pub damage_source: Option<DamageSourceInfo<'a>>,
    /// Block entity reference for container/block loot.
    pub block_entity: Option<BlockEntityRef<'a>>,
    /// The entity interacting with a block/entity (e.g., player opening a chest).
    pub interacting_entity: Option<EntityRef<'a>>,
    /// Scoreboard lookup for `minecraft:score` number providers; `None` yields `0`.
    pub scoreboard: Option<&'a dyn ScoreboardValueLookup>,
    /// Command-storage lookup for `minecraft:storage` number providers; `None`
    /// yields `0`.
    pub command_storage: Option<&'a dyn CommandStorageLookup>,
    /// Environment-attribute lookup for `minecraft:environment_attribute` number
    /// providers; `None` yields `0`.
    pub environment_attributes: Option<&'a dyn EnvironmentAttributeLookup>,
    /// Vanilla `LootContextParams.ENCHANTMENT_LEVEL`, read by
    /// `minecraft:enchantment_level`.
    pub enchantment_level: Option<i32>,
    /// Resolution of `minecraft:<path>` provider references; `None` yields `0`.
    pub number_providers: Option<&'a dyn NumberProviderLookup>,
    /// World view read by `minecraft:location_check`
    /// (vanilla `LootContextParams.ORIGIN` plus the level the loot runs in).
    ///
    /// With no view installed a constrained location predicate fails closed; an
    /// unconstrained one still passes whenever an origin is present.
    pub location: Option<&'a dyn LevelView>,
    /// Live entity/block-entity state read by the item functions that copy
    /// components, custom data, player-head profiles, or allocate exploration
    /// maps. With no hooks installed those functions leave the stack untouched,
    /// matching vanilla's behaviour when the referenced source is absent.
    pub item_source: Option<&'a dyn LootItemSourceHooks>,
}

/// Host hooks for loot item functions that need live world or entity state the
/// registry cannot model by itself.
///
/// Every method defaults to `None`, so a host installs only what it can supply.
/// The hooks mirror the vanilla accessors named on each method; an absent hook
/// behaves exactly like vanilla's `null` lookup result.
pub trait LootItemSourceHooks {
    /// Vanilla `DataComponentGetter` for `target` (`Entity.collectComponents`),
    /// used by `minecraft:copy_components`.
    fn entity_components(&self, _target: LootContextEntity) -> Option<&DataComponentMap> {
        None
    }

    /// Vanilla `BlockEntity.collectComponents`, used by
    /// `minecraft:copy_components` with a `block_entity` source.
    fn block_entity_components(&self) -> Option<&DataComponentMap> {
        None
    }

    /// Vanilla `NbtProvider` payload of an entity, used by
    /// `minecraft:copy_custom_data`.
    fn entity_nbt(&self, _target: LootContextEntity) -> Option<&NbtCompound> {
        None
    }

    /// Vanilla `NbtProvider` payload of the looted block entity, used by
    /// `minecraft:copy_custom_data`.
    fn block_entity_nbt(&self) -> Option<&NbtCompound> {
        None
    }

    /// Vanilla `Player.getGameProfile`, used by `minecraft:fill_player_head`.
    fn player_profile(&self, _target: LootContextEntity) -> Option<&GameProfile> {
        None
    }

    /// Vanilla `ExplorationMapFunction.run`: claims/finds the nearest structure
    /// for `destination` and allocates the map's saved data. Returns the
    /// structure's block X/Z and the new map id.
    fn create_exploration_map(
        &self,
        _destination: &Identifier,
        _origin: (f64, f64, f64),
        _zoom: i32,
        _search_radius: i32,
        _skip_existing_chunks: bool,
    ) -> Option<((i32, i32), i32)> {
        None
    }
}

/// Weather state for `WeatherCheck` condition.
#[derive(Debug, Clone, Copy, Default)]
pub struct WeatherState {
    pub raining: bool,
    pub thundering: bool,
}

/// A reference to an entity for loot context.
/// This is intentionally opaque - the actual entity type depends on game implementation.
#[derive(Debug, Clone, Copy)]
pub struct EntityRef<'a> {
    /// Type identifier for the entity.
    pub entity_type: Option<&'a Identifier>,
    /// Entity flags for predicate checking.
    pub flags: EntityRefFlags,
    /// Equipment slots for equipment predicates. `None` for non-living entities.
    pub equipment: Option<EntityEquipmentRef<'a>>,
    /// Entity name (for `copy_name` function).
    pub custom_name: Option<&'a str>,
    /// Vanilla `minecraft:components.sheep/color` entity data component.
    pub sheep_color: Option<DyeColor>,
    /// Vanilla `minecraft:type_specific/sheep.sheared`. `None` when the entity is
    /// not a sheep, matching vanilla `SheepPredicate.matches`' non-sheep rejection.
    pub sheep_sheared: Option<bool>,
    /// Vanilla `minecraft:components.chicken/variant` entity data component key.
    pub chicken_variant: Option<&'a Identifier>,
}

/// Entity flags for predicate checking.
#[derive(Debug, Clone, Copy, Default)]
pub struct EntityRefFlags {
    pub is_on_fire: bool,
    pub is_sneaking: bool,
    pub is_sprinting: bool,
    pub is_swimming: bool,
    pub is_baby: bool,
}

/// One item stack per equipment slot, indexed by [`EquipmentSlot::index`].
pub type EntityEquipmentSlots = [ItemStack; EquipmentSlot::ALL.len()];

/// Every equipment slot of a living entity.
#[derive(Debug, Clone, Copy)]
pub struct EntityEquipmentRef<'a>(&'a EntityEquipmentSlots);

impl<'a> EntityEquipmentRef<'a> {
    #[must_use]
    pub const fn new(slots: &'a EntityEquipmentSlots) -> Self {
        Self(slots)
    }

    /// Returns the item in the given equipment slot.
    #[must_use]
    pub const fn get(self, slot: EquipmentSlot) -> &'a ItemStack {
        &self.0[slot.index()]
    }

    /// Returns the highest enchantment level from matching equipment slots.
    #[must_use]
    pub fn get_enchantment_level(self, enchantment: &Identifier) -> i32 {
        let Some(definition) = REGISTRY.enchantments.by_key(enchantment) else {
            return 0;
        };
        EquipmentSlot::ALL
            .into_iter()
            .filter(|slot| definition.matching_slot(*slot))
            .filter_map(|slot| {
                let stack = self.get(slot);
                (!stack.is_empty()).then(|| stack.get_enchantment_level(enchantment))
            })
            .max()
            .unwrap_or(0)
    }
}

/// Damage source information for loot context.
#[derive(Debug, Clone, Copy)]
pub struct DamageSourceInfo<'a> {
    /// The damage type identifier.
    pub damage_type: Option<&'a Identifier>,
    /// Tags associated with this damage source.
    pub tags: &'a [Identifier],
    /// Whether this is direct damage (not from a projectile).
    pub is_direct: bool,
}

/// A reference to a block entity for loot context.
#[derive(Debug, Clone, Copy)]
pub struct BlockEntityRef<'a> {
    /// The block entity type identifier.
    pub block_entity_type: Option<&'a Identifier>,
    /// Custom name of the block entity (for `copy_name`).
    pub custom_name: Option<&'a str>,
    /// Inventory contents (for dynamic/slots entries).
    pub inventory: Option<&'a [ItemStack]>,
}

impl<'a, R: rand::Rng> LootContext<'a, R> {
    /// Create a new loot context with just an RNG.
    pub const fn new(rng: &'a mut R) -> Self {
        Self {
            rng,
            luck: 0.0,
            block_state: None,
            tool: None,
            explosion_radius: None,
            killed_by_player: false,
            origin: None,
            game_time: None,
            weather: None,
            this_entity: None,
            killer_entity: None,
            direct_killer_entity: None,
            last_damage_player: None,
            damage_source: None,
            block_entity: None,
            interacting_entity: None,
            scoreboard: None,
            command_storage: None,
            environment_attributes: None,
            enchantment_level: None,
            number_providers: None,
            location: None,
            item_source: None,
        }
    }

    /// Set the luck value.
    #[must_use]
    pub const fn with_luck(mut self, luck: f32) -> Self {
        self.luck = luck;
        self
    }

    /// Set the block state.
    #[must_use]
    pub const fn with_block_state(mut self, state: BlockStateId) -> Self {
        self.block_state = Some(state);
        self
    }

    /// Set the tool used.
    #[must_use]
    pub const fn with_tool(mut self, tool: &'a ItemStack) -> Self {
        self.tool = Some(tool);
        self
    }

    /// Set the explosion radius.
    #[must_use]
    pub const fn with_explosion(mut self, radius: f32) -> Self {
        self.explosion_radius = Some(radius);
        self
    }

    /// Set whether killed by player.
    #[must_use]
    pub const fn with_killed_by_player(mut self, killed: bool) -> Self {
        self.killed_by_player = killed;
        self
    }

    /// Set the world origin position.
    #[must_use]
    pub const fn with_origin(mut self, x: f64, y: f64, z: f64) -> Self {
        self.origin = Some((x, y, z));
        self
    }

    /// Set the game time.
    #[must_use]
    pub const fn with_game_time(mut self, time: i64) -> Self {
        self.game_time = Some(time);
        self
    }

    /// Set the weather state.
    #[must_use]
    pub const fn with_weather(mut self, weather: WeatherState) -> Self {
        self.weather = Some(weather);
        self
    }

    /// Set the entity being looted.
    #[must_use]
    pub const fn with_this_entity(mut self, entity: EntityRef<'a>) -> Self {
        self.this_entity = Some(entity);
        self
    }

    /// Set the killer entity.
    #[must_use]
    pub const fn with_killer_entity(mut self, entity: EntityRef<'a>) -> Self {
        self.killer_entity = Some(entity);
        self
    }

    /// Set the direct killer entity (e.g., projectile).
    #[must_use]
    pub const fn with_direct_killer_entity(mut self, entity: EntityRef<'a>) -> Self {
        self.direct_killer_entity = Some(entity);
        self
    }

    /// Set the player who dealt the final damage.
    #[must_use]
    pub const fn with_last_damage_player(mut self, entity: EntityRef<'a>) -> Self {
        self.last_damage_player = Some(entity);
        self
    }

    /// Set the damage source information.
    #[must_use]
    pub const fn with_damage_source(mut self, damage_source: DamageSourceInfo<'a>) -> Self {
        self.damage_source = Some(damage_source);
        self
    }

    /// Set the block entity reference.
    #[must_use]
    pub const fn with_block_entity(mut self, block_entity: BlockEntityRef<'a>) -> Self {
        self.block_entity = Some(block_entity);
        self
    }

    /// Set the interacting entity (e.g., player opening a chest).
    #[must_use]
    pub const fn with_interacting_entity(mut self, entity: EntityRef<'a>) -> Self {
        self.interacting_entity = Some(entity);
        self
    }

    /// Set the scoreboard lookup used by `minecraft:score` number providers.
    #[must_use]
    pub const fn with_scoreboard(mut self, scoreboard: &'a dyn ScoreboardValueLookup) -> Self {
        self.scoreboard = Some(scoreboard);
        self
    }

    /// Set the command-storage lookup used by `minecraft:storage` number providers.
    #[must_use]
    pub const fn with_command_storage(
        mut self,
        command_storage: &'a dyn CommandStorageLookup,
    ) -> Self {
        self.command_storage = Some(command_storage);
        self
    }

    /// Set the environment-attribute lookup used by
    /// `minecraft:environment_attribute` number providers.
    #[must_use]
    pub const fn with_environment_attributes(
        mut self,
        environment_attributes: &'a dyn EnvironmentAttributeLookup,
    ) -> Self {
        self.environment_attributes = Some(environment_attributes);
        self
    }

    /// Set the world view read by `minecraft:location_check`.
    #[must_use]
    pub const fn with_location(mut self, location: &'a dyn LevelView) -> Self {
        self.location = Some(location);
        self
    }

    /// Set the enchantment level read by `minecraft:enchantment_level`.
    #[must_use]
    pub const fn with_enchantment_level(mut self, level: i32) -> Self {
        self.enchantment_level = Some(level);
        self
    }

    /// Set the lookup used to resolve `minecraft:<path>` provider references.
    #[must_use]
    pub const fn with_number_providers(
        mut self,
        number_providers: &'a dyn NumberProviderLookup,
    ) -> Self {
        self.number_providers = Some(number_providers);
        self
    }

    /// Set the hooks the item functions use to read live entity, block-entity,
    /// and world state.
    #[must_use]
    pub const fn with_item_source(mut self, item_source: &'a dyn LootItemSourceHooks) -> Self {
        self.item_source = Some(item_source);
        self
    }

    /// The number-provider parameter view of this context.
    fn params(&self) -> LootContextRef<'_> {
        LootContextRef {
            tool: self.tool,
            scoreboard: self.scoreboard,
            command_storage: self.command_storage,
            environment_attributes: self.environment_attributes,
            enchantment_level: self.enchantment_level,
            this_entity: self.this_entity,
            killer_entity: self.killer_entity,
            direct_killer_entity: self.direct_killer_entity,
            last_damage_player: self.last_damage_player,
            interacting_entity: self.interacting_entity,
            origin: self.origin,
        }
    }

    /// Get the level of an enchantment on the tool by identifier.
    #[must_use]
    pub fn get_enchantment_level_by_id(&self, enchantment: &Identifier) -> i32 {
        self.tool
            .map_or(0, |t| t.get_enchantment_level(enchantment))
    }

    /// Get the level of an enchantment on the attacking entity's equipment.
    #[must_use]
    pub fn get_attacking_entity_enchantment_level(&self, enchantment: &Identifier) -> i32 {
        self.killer_entity
            .and_then(|entity| entity.equipment)
            .map_or(0, |equipment| equipment.get_enchantment_level(enchantment))
    }

    /// Get an entity reference by target.
    #[must_use]
    pub const fn get_entity(&self, target: LootContextEntity) -> Option<EntityRef<'a>> {
        match target {
            LootContextEntity::This => self.this_entity,
            LootContextEntity::Killer => self.killer_entity,
            LootContextEntity::DirectKiller => self.direct_killer_entity,
            LootContextEntity::KillerPlayer => self.last_damage_player,
            LootContextEntity::Interacting => self.interacting_entity,
        }
    }
}
