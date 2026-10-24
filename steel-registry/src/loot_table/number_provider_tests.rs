//! Focused tests for every [`NumberProvider`] variant.
//!
//! Each test pins the vanilla evaluation rule it mirrors (`constant`, `uniform`,
//! `binomial`, the arithmetic/aggregate providers, `score`, `storage`,
//! `enchantment_level`, …) against the class in
//! `net/minecraft/world/level/storage/loot/providers/number/`. Bounds and
//! determinism are checked with a seeded RNG; distributions are checked
//! statistically, since Steel's RNG is not Java's `RandomSource`.

use super::*;
use crate::enchantment::effect::LevelBasedValue;
use rand::SeedableRng;

fn seeded_rng(seed: u64) -> rand::rngs::StdRng {
    rand::rngs::StdRng::seed_from_u64(seed)
}

fn ident(name: &'static str) -> Identifier {
    Identifier::vanilla_static(name)
}

// --- Shared `&'static` provider trees (the enum is not `Copy`, and `Identifier`
// wraps a `Cow`, so array literals cannot be promoted; they live in statics). --

static THREE: NumberProvider = NumberProvider::Constant(3.0);
static FOUR: NumberProvider = NumberProvider::Constant(4.0);
static TWO: NumberProvider = NumberProvider::Constant(2.0);
static ONE: NumberProvider = NumberProvider::Constant(1.0);
static ZERO: NumberProvider = NumberProvider::Constant(0.0);
static NEG_ONE: NumberProvider = NumberProvider::Constant(-1.0);
static NEG_SEVEN: NumberProvider = NumberProvider::Constant(-7.0);
static MIN_INT: NumberProvider = NumberProvider::Constant(i32::MIN as f32);
static HALF: NumberProvider = NumberProvider::Constant(2.5);
static NEG_HALF: NumberProvider = NumberProvider::Constant(-2.5);
static ANGLE: NumberProvider = NumberProvider::Constant(1000.0);

static NO_TERMS: [LootCondition; 0] = [];
static ALWAYS_TRUE: LootCondition = LootCondition::AllOf(&NO_TERMS);
static ALWAYS_FALSE: LootCondition = LootCondition::Inverted(&ALWAYS_TRUE);

static ADD_INPUTS: [NumberProvider; 2] =
    [NumberProvider::Constant(1.0), NumberProvider::Constant(2.0)];
static AVG_INPUTS: [NumberProvider; 2] =
    [NumberProvider::Constant(1.0), NumberProvider::Constant(4.0)];
static MIN_INPUTS: [NumberProvider; 2] =
    [NumberProvider::Constant(5.0), NumberProvider::Constant(2.0)];
static MAX_INPUTS: [NumberProvider; 2] =
    [NumberProvider::Constant(5.0), NumberProvider::Constant(2.0)];
static MUL_INPUTS: [NumberProvider; 2] =
    [NumberProvider::Constant(3.0), NumberProvider::Constant(4.0)];
static LENGTH_INPUTS: [NumberProvider; 2] =
    [NumberProvider::Constant(3.0), NumberProvider::Constant(4.0)];

static DISPATCH_CASES: [(&LootCondition, NumberProvider); 3] = [
    (&ALWAYS_FALSE, NumberProvider::Constant(1.0)),
    (&ALWAYS_TRUE, NumberProvider::Constant(9.0)),
    (&ALWAYS_TRUE, NumberProvider::Constant(11.0)),
];
static DISPATCH_ALL_FALSE: [(&LootCondition, NumberProvider); 1] =
    [(&ALWAYS_FALSE, NumberProvider::Constant(1.0))];

static WEIGHTED_ONE: [(NumberProvider, u32); 1] = [(NumberProvider::Constant(8.0), 5)];
static WEIGHTED_ZERO: [(NumberProvider, u32); 1] = [(NumberProvider::Constant(8.0), 0)];

static LINEAR: LevelBasedValue = LevelBasedValue::Linear {
    base: 2.0,
    per_level_above_first: 1.5,
};

// --- Lookup hooks for the context-reading providers. -----------------------

struct FixedScoreboard;

impl ScoreboardValueLookup for FixedScoreboard {
    fn get_score(
        &self,
        _ctx: &LootContextRef<'_>,
        target: &ScoreboardTarget,
        objective: &str,
    ) -> Option<i32> {
        match (target, objective) {
            (ScoreboardTarget::Fixed("Alice"), "points") => Some(42),
            _ => None,
        }
    }
}

struct FixedStorage;

impl CommandStorageLookup for FixedStorage {
    fn get_numeric_tag(&self, _storage: &Identifier, path: &str) -> Option<f64> {
        (path == "x").then_some(2.75)
    }
}

struct FixedAttributes;

impl EnvironmentAttributeLookup for FixedAttributes {
    fn get_attribute(
        &self,
        attribute: &Identifier,
        origin: Option<(f64, f64, f64)>,
    ) -> Option<f32> {
        (*attribute == ident("minecraft:sky_color") && origin.is_some()).then_some(3.5)
    }
}

struct FixedProviders;

impl NumberProviderLookup for FixedProviders {
    fn get_number_provider(&self, key: &Identifier) -> Option<&'static NumberProvider> {
        static SEVEN: NumberProvider = NumberProvider::Constant(7.0);
        (*key == ident("minecraft:test/seven")).then_some(&SEVEN)
    }
}

// --- constant / reference --------------------------------------------------

#[test]
fn constant_is_java_math_round_in_int_contexts() {
    // `ContextIntProvider.ConstantValue` holds an int; the fused Steel enum keeps
    // the float, so an int use goes through `Math.round(float)`.
    assert_eq!(NumberProvider::Constant(5.0).get_int(&mut seeded_rng(0)), 5);
    assert_eq!(NumberProvider::Constant(2.4).get_int(&mut seeded_rng(0)), 2);
    assert_eq!(NumberProvider::Constant(2.5).get_int(&mut seeded_rng(0)), 3);
    // `Math.round(-0.5f) == 0`, `Math.round(-1.5f) == -1`.
    assert_eq!(
        NumberProvider::Constant(-0.5).get_int(&mut seeded_rng(0)),
        0
    );
    assert_eq!(
        NumberProvider::Constant(-1.5).get_int(&mut seeded_rng(0)),
        -1
    );
    assert_eq!(
        NumberProvider::Constant(-1.6).get_int(&mut seeded_rng(0)),
        -2
    );
    assert_eq!(
        NumberProvider::Constant(-0.5).get_simple(&mut seeded_rng(0)),
        -0.5
    );
}

#[test]
fn reference_resolves_through_the_lookup_and_defaults_to_zero() {
    let provider = NumberProvider::Reference {
        key: ident("minecraft:test/seven"),
    };
    // No hook installed: vanilla has no unresolved-holder case, Steel yields 0.
    assert_eq!(provider.get_int(&mut seeded_rng(0)), 0);

    let mut rng = seeded_rng(1);
    let mut ctx = LootContext::new(&mut rng).with_number_providers(&FixedProviders);
    assert_eq!(provider.get_int_with_context(&mut ctx), 7);
    assert_eq!(provider.get_float(&mut ctx), 7.0);
}

// --- uniform ---------------------------------------------------------------

#[test]
fn uniform_int_is_inclusive_and_float_is_half_open() {
    let provider = NumberProvider::Uniform { min: 1.0, max: 3.0 };
    let mut seen = [false; 4];
    for seed in 0u64..1000 {
        let value = provider.get_int(&mut seeded_rng(seed));
        assert!((1..=3).contains(&value));
        seen[value as usize] = true;
    }
    assert!(seen[1] && seen[2] && seen[3]);

    // `Mth.nextFloat` never returns `max`.
    for seed in 0u64..1000 {
        let value = provider.get_simple(&mut seeded_rng(seed));
        assert!((1.0..3.0).contains(&value));
    }
}

#[test]
fn uniform_returns_min_when_bounds_are_not_increasing() {
    // `Mth.nextInt`/`Mth.nextFloat` short-circuit when `min >= max`.
    let provider = NumberProvider::Uniform { min: 5.0, max: 5.0 };
    assert_eq!(provider.get_int(&mut seeded_rng(0)), 5);
    assert_eq!(provider.get_simple(&mut seeded_rng(0)), 5.0);

    let inverted = NumberProvider::Uniform { min: 7.0, max: 2.0 };
    assert_eq!(inverted.get_int(&mut seeded_rng(0)), 7);
    assert_eq!(inverted.get_simple(&mut seeded_rng(0)), 7.0);
}

#[test]
fn uniform_with_provider_bounds_evaluates_both_bounds() {
    let provider = NumberProvider::UniformProviders {
        min: &THREE,
        max: &THREE,
    };
    assert_eq!(provider.get_int(&mut seeded_rng(0)), 3);
    assert_eq!(provider.get_simple(&mut seeded_rng(0)), 3.0);

    let range = NumberProvider::UniformProviders {
        min: &ONE,
        max: &FOUR,
    };
    for seed in 0u64..500 {
        assert!((1..=4).contains(&range.get_int(&mut seeded_rng(seed))));
    }
}

// --- binomial --------------------------------------------------------------

#[test]
fn binomial_zero_trials_and_certain_chance() {
    assert_eq!(
        NumberProvider::Binomial { n: 0, p: 0.5 }.get_int(&mut seeded_rng(0)),
        0
    );
    // `nextFloat() < 1.0` is always true, `nextFloat() < 0.0` never.
    assert_eq!(
        NumberProvider::Binomial { n: 12, p: 1.0 }.get_int(&mut seeded_rng(0)),
        12
    );
    assert_eq!(
        NumberProvider::Binomial { n: 12, p: 0.0 }.get_int(&mut seeded_rng(0)),
        0
    );
}

#[test]
fn binomial_is_deterministic_for_a_seed_and_has_the_expected_mean() {
    let provider = NumberProvider::Binomial { n: 20, p: 0.35 };
    assert_eq!(
        provider.get_int(&mut seeded_rng(7)),
        provider.get_int(&mut seeded_rng(7))
    );

    let draws = 4000;
    let total: i64 = (0..draws)
        .map(|seed| i64::from(provider.get_int(&mut seeded_rng(seed))))
        .sum();
    let mean = total as f64 / draws as f64;
    // Binomial(20, 0.35) has mean 7 and standard deviation ~2.13.
    assert!((mean - 7.0).abs() < 0.5, "mean was {mean}");
}

#[test]
fn binomial_with_provider_params_uses_nested_values() {
    let provider = NumberProvider::BinomialProviders { n: &THREE, p: &ONE };
    assert_eq!(provider.get_int(&mut seeded_rng(0)), 3);

    // `p` is read through `getFloatOrThrow`; a chance below 0 never succeeds.
    let zero_p = NumberProvider::BinomialProviders {
        n: &THREE,
        p: &NEG_ONE,
    };
    assert_eq!(zero_p.get_int(&mut seeded_rng(0)), 0);
    assert_eq!(zero_p.get_simple(&mut seeded_rng(0)), 0.0);
}

// --- aggregates and arithmetic --------------------------------------------

#[test]
fn aggregate_providers_follow_vanilla_int_and_float_rules() {
    let add = NumberProvider::Add {
        inputs: &ADD_INPUTS,
    };
    assert_eq!(add.get_int(&mut seeded_rng(0)), 3);
    assert_eq!(add.get_simple(&mut seeded_rng(0)), 3.0);

    let avg = NumberProvider::Average {
        inputs: &AVG_INPUTS,
    };
    // Int average divides in `long` (truncating): (1 + 4) / 2 == 2.
    assert_eq!(avg.get_int(&mut seeded_rng(0)), 2);
    assert_eq!(avg.get_simple(&mut seeded_rng(0)), 2.5);

    let min = NumberProvider::Minimum {
        inputs: &MIN_INPUTS,
    };
    assert_eq!(min.get_int(&mut seeded_rng(0)), 2);
    assert_eq!(min.get_simple(&mut seeded_rng(0)), 2.0);

    let max = NumberProvider::Maximum {
        inputs: &MAX_INPUTS,
    };
    assert_eq!(max.get_int(&mut seeded_rng(0)), 5);
    assert_eq!(max.get_simple(&mut seeded_rng(0)), 5.0);

    let mul = NumberProvider::Product {
        inputs: &MUL_INPUTS,
    };
    assert_eq!(mul.get_int(&mut seeded_rng(0)), 12);
    assert_eq!(mul.get_simple(&mut seeded_rng(0)), 12.0);
}

#[test]
fn integer_average_truncates_towards_zero() {
    static INPUTS: [NumberProvider; 2] = [
        NumberProvider::Constant(-7.0),
        NumberProvider::Constant(2.0),
    ];
    let avg = NumberProvider::Average { inputs: &INPUTS };
    // Vanilla accumulates `long`s and uses `ldiv` (truncation), not floorDiv.
    assert_eq!(avg.get_int(&mut seeded_rng(0)), -2);
}

#[test]
fn binary_providers_match_java_integer_semantics() {
    let sub = NumberProvider::Difference {
        left: &NEG_SEVEN,
        right: &TWO,
    };
    assert_eq!(sub.get_int(&mut seeded_rng(0)), -9);

    // `div` truncates towards zero, `floor_div` floors.
    assert_eq!(
        NumberProvider::Quotient {
            left: &NEG_SEVEN,
            right: &TWO,
        }
        .get_int(&mut seeded_rng(0)),
        -3
    );
    assert_eq!(
        NumberProvider::FloorQuotient {
            left: &NEG_SEVEN,
            right: &TWO,
        }
        .get_int(&mut seeded_rng(0)),
        -4
    );
    // `mod` keeps the dividend's sign, `floor_mod` the divisor's.
    assert_eq!(
        NumberProvider::Modulus {
            left: &NEG_SEVEN,
            right: &TWO,
        }
        .get_int(&mut seeded_rng(0)),
        -1
    );
    assert_eq!(
        NumberProvider::FloorModulus {
            left: &NEG_SEVEN,
            right: &TWO,
        }
        .get_int(&mut seeded_rng(0)),
        1
    );
}

#[test]
fn division_by_zero_is_an_arithmetic_error_that_collapses_to_zero() {
    let div = NumberProvider::Quotient {
        left: &THREE,
        right: &ZERO,
    };
    assert_eq!(div.get_int(&mut seeded_rng(0)), 0);
    // The float domain has no exception: 3 / 0 == +inf, which `getFloat` maps to 0.
    assert_eq!(div.get_simple(&mut seeded_rng(0)), 0.0);
    assert_eq!(
        NumberProvider::Modulus {
            left: &THREE,
            right: &ZERO,
        }
        .get_int(&mut seeded_rng(0)),
        0
    );
}

#[test]
fn power_is_exact_and_zero_to_zero_is_an_error() {
    let pow = NumberProvider::Power {
        base: &TWO,
        exponent: &NumberProvider::Constant(10.0),
    };
    assert_eq!(pow.get_int(&mut seeded_rng(0)), 1024);
    assert_eq!(pow.get_simple(&mut seeded_rng(0)), 1024.0);

    // `0^0` throws `ArithmeticException` (int) and yields NaN (float), which the
    // finiteness check turns into 0.
    let zero_pow = NumberProvider::Power {
        base: &ZERO,
        exponent: &ZERO,
    };
    assert_eq!(zero_pow.get_int(&mut seeded_rng(0)), 0);
    assert_eq!(zero_pow.get_simple(&mut seeded_rng(0)), 0.0);

    // Negative exponents overflow int and are an error; the float domain is fine.
    let neg = NumberProvider::Power {
        base: &TWO,
        exponent: &NEG_ONE,
    };
    assert_eq!(neg.get_int(&mut seeded_rng(0)), 0);
    assert_eq!(neg.get_simple(&mut seeded_rng(0)), 0.5);
}

#[test]
fn negate_and_absolute_overflow_to_the_error_default() {
    assert_eq!(
        NumberProvider::Negate { input: &THREE }.get_int(&mut seeded_rng(0)),
        -3
    );
    assert_eq!(
        NumberProvider::Absolute { input: &THREE }.get_int(&mut seeded_rng(0)),
        3
    );

    // `Math.negateExact`/`Math.absExact` throw for `Integer.MIN_VALUE`.
    assert_eq!(
        NumberProvider::Negate { input: &MIN_INT }.get_int(&mut seeded_rng(0)),
        0
    );
    assert_eq!(
        NumberProvider::Absolute { input: &MIN_INT }.get_int(&mut seeded_rng(0)),
        0
    );
}

// --- rounding and float maths ---------------------------------------------

#[test]
fn rounding_providers_match_mth() {
    assert_eq!(
        NumberProvider::Ceiling { input: &HALF }.get_simple(&mut seeded_rng(0)),
        3.0
    );
    assert_eq!(
        NumberProvider::Floor { input: &HALF }.get_simple(&mut seeded_rng(0)),
        2.0
    );
    assert_eq!(
        NumberProvider::Round { input: &HALF }.get_simple(&mut seeded_rng(0)),
        3.0
    );
    assert_eq!(
        NumberProvider::Truncate { input: &HALF }.get_simple(&mut seeded_rng(0)),
        2.0
    );

    assert_eq!(
        NumberProvider::Ceiling { input: &NEG_HALF }.get_simple(&mut seeded_rng(0)),
        -2.0
    );
    assert_eq!(
        NumberProvider::Floor { input: &NEG_HALF }.get_simple(&mut seeded_rng(0)),
        -3.0
    );
    assert_eq!(
        NumberProvider::Truncate { input: &NEG_HALF }.get_simple(&mut seeded_rng(0)),
        -2.0
    );
}

#[test]
fn square_root_and_length_use_mth_sqrt() {
    // `Mth.sqrt` is `(float) Math.sqrt((double) value)`.
    assert_eq!(
        NumberProvider::SquareRoot { input: &TWO }.get_simple(&mut seeded_rng(0)),
        (f64::from(2.0_f32)).sqrt() as f32
    );

    let length = NumberProvider::Length {
        inputs: &LENGTH_INPUTS,
    };
    assert_eq!(length.get_simple(&mut seeded_rng(0)), 5.0);
}

#[test]
fn sine_and_cosine_use_the_mth_lookup_table() {
    let sin = NumberProvider::Sine { input: &ANGLE }.get_simple(&mut seeded_rng(0));
    let cos = NumberProvider::Cosine { input: &ANGLE }.get_simple(&mut seeded_rng(0));

    assert_eq!(sin, steel_math::trig::sin(1000.0));
    assert_eq!(cos, steel_math::trig::cos(1000.0));
    // The table quantises the angle to 2π/65536, so it cannot match the FPU sine
    // at this magnitude.
    assert_ne!(sin, 1000.0_f32.sin());
    assert_ne!(cos, 1000.0_f32.cos());

    assert_eq!(
        NumberProvider::Sine { input: &ZERO }.get_simple(&mut seeded_rng(0)),
        0.0
    );
    assert_eq!(
        NumberProvider::Cosine { input: &ZERO }.get_simple(&mut seeded_rng(0)),
        1.0
    );
}

#[test]
fn domain_bridge_providers_truncate_and_widen() {
    // `from_float` is `floatToIntSafe`: `(int)` truncation.
    static POS: NumberProvider = NumberProvider::Constant(3.9);
    static NEG: NumberProvider = NumberProvider::Constant(-3.9);
    static NAN: NumberProvider = NumberProvider::Constant(f32::NAN);
    assert_eq!(
        NumberProvider::FromFloat { input: &POS }.get_int(&mut seeded_rng(0)),
        3
    );
    assert_eq!(
        NumberProvider::FromFloat { input: &NEG }.get_int(&mut seeded_rng(0)),
        -3
    );
    // A non-finite input is an arithmetic error.
    assert_eq!(
        NumberProvider::FromFloat { input: &NAN }.get_int(&mut seeded_rng(0)),
        0
    );
    // `from_int` is `intToFloatSafe`: a plain `(float)` widening.
    assert_eq!(
        NumberProvider::FromInt { input: &THREE }.get_simple(&mut seeded_rng(0)),
        3.0
    );
}

// --- conditional / dispatcher / weighted_list ------------------------------

#[test]
fn conditional_selects_the_branch_of_its_condition() {
    let on_true = NumberProvider::Conditional {
        condition: &ALWAYS_TRUE,
        on_true: &THREE,
        on_false: &FOUR,
    };
    assert_eq!(on_true.get_int(&mut seeded_rng(0)), 3);

    let on_false = NumberProvider::Conditional {
        condition: &ALWAYS_FALSE,
        on_true: &THREE,
        on_false: &FOUR,
    };
    assert_eq!(on_false.get_int(&mut seeded_rng(0)), 4);
}

#[test]
fn dispatcher_takes_the_first_passing_case_then_the_default() {
    // Cases are tested in order; the first `true` case (9) wins, not the second.
    let provider = NumberProvider::Dispatcher {
        cases: &DISPATCH_CASES,
        default: &NEG_ONE,
    };
    assert_eq!(provider.get_int(&mut seeded_rng(0)), 9);

    let fallback = NumberProvider::Dispatcher {
        cases: &DISPATCH_ALL_FALSE,
        default: &NEG_ONE,
    };
    assert_eq!(fallback.get_int(&mut seeded_rng(0)), -1);
    assert_eq!(fallback.get_simple(&mut seeded_rng(0)), -1.0);
}

#[test]
fn weighted_list_picks_by_weight_and_defaults_to_zero() {
    let provider = NumberProvider::WeightedList {
        distribution: &WEIGHTED_ONE,
    };
    assert_eq!(provider.get_int(&mut seeded_rng(0)), 8);
    assert_eq!(provider.get_simple(&mut seeded_rng(0)), 8.0);

    // A zero total weight has no selectable entry; Steel yields 0.
    let empty = NumberProvider::WeightedList {
        distribution: &WEIGHTED_ZERO,
    };
    assert_eq!(empty.get_int(&mut seeded_rng(0)), 0);
    assert_eq!(empty.get_simple(&mut seeded_rng(0)), 0.0);
}

// --- score / storage / enchantment_level / environment_attribute -----------

#[test]
fn score_reads_the_lookup_and_falls_back_when_unavailable() {
    let provider = NumberProvider::Score {
        target: ScoreboardTarget::Fixed("Alice"),
        score: "points",
        scale: 1.0,
        fallback: &NEG_ONE,
    };

    let mut rng = seeded_rng(0);
    let mut ctx = LootContext::new(&mut rng).with_scoreboard(&FixedScoreboard);
    assert_eq!(provider.get_int_with_context(&mut ctx), 42);
    assert_eq!(provider.get_float(&mut ctx), 42.0);

    // Unknown objective (or absent entity) uses the fallback.
    let missing = NumberProvider::Score {
        target: ScoreboardTarget::Fixed("Bob"),
        score: "points",
        scale: 1.0,
        fallback: &NEG_ONE,
    };
    assert_eq!(missing.get_int_with_context(&mut ctx), -1);

    // Without a hook the provider evaluates to its fallback too.
    let mut rng = seeded_rng(0);
    let mut bare = LootContext::new(&mut rng);
    assert_eq!(provider.get_int_with_context(&mut bare), -1);
}

#[test]
fn score_scale_multiplies_the_raw_score() {
    let provider = NumberProvider::Score {
        target: ScoreboardTarget::Fixed("Alice"),
        score: "points",
        scale: 2.0,
        fallback: &NEG_ONE,
    };
    let mut rng = seeded_rng(0);
    let mut ctx = LootContext::new(&mut rng).with_scoreboard(&FixedScoreboard);
    assert_eq!(provider.get_int_with_context(&mut ctx), 84);
    assert_eq!(provider.get_float(&mut ctx), 84.0);
}

#[test]
fn storage_reads_a_numeric_tag_and_truncates_in_int_contexts() {
    let provider = NumberProvider::Storage {
        storage: ident("minecraft:test/values"),
        path: "x",
        fallback: &NEG_ONE,
    };
    let mut rng = seeded_rng(0);
    let mut ctx = LootContext::new(&mut rng).with_command_storage(&FixedStorage);
    // Vanilla `Number.intValue()` truncates towards zero.
    assert_eq!(provider.get_int_with_context(&mut ctx), 2);
    assert_eq!(provider.get_float(&mut ctx), 2.75);

    let missing = NumberProvider::Storage {
        storage: ident("minecraft:test/values"),
        path: "y",
        fallback: &NEG_ONE,
    };
    assert_eq!(missing.get_int_with_context(&mut ctx), -1);

    let mut rng = seeded_rng(0);
    let mut bare = LootContext::new(&mut rng);
    assert_eq!(provider.get_int_with_context(&mut bare), -1);
}

#[test]
fn enchantment_level_evaluates_the_formula_at_the_context_level() {
    let provider = NumberProvider::EnchantmentLevel { amount: &LINEAR };

    let mut rng = seeded_rng(0);
    let mut ctx = LootContext::new(&mut rng).with_enchantment_level(3);
    // Linear { base: 2, per_level_above_first: 1.5 } at level 3 == 2 + 1.5 * 2.
    assert_eq!(provider.get_float(&mut ctx), 5.0);

    // With no level, vanilla passes 0 into `calculate`, not a constant 0 result.
    let mut rng = seeded_rng(0);
    let mut bare = LootContext::new(&mut rng);
    assert_eq!(provider.get_float(&mut bare), 0.5);
}

#[test]
fn environment_attribute_reads_at_the_origin() {
    let provider = NumberProvider::EnvironmentAttribute {
        attribute: ident("minecraft:sky_color"),
    };
    let mut rng = seeded_rng(0);
    let mut ctx = LootContext::new(&mut rng)
        .with_environment_attributes(&FixedAttributes)
        .with_origin(0.0, 64.0, 0.0);
    assert_eq!(provider.get_float(&mut ctx), 3.5);
    assert_eq!(provider.get_int_with_context(&mut ctx), 3);

    // No origin, or no hook, yields 0.
    let mut rng = seeded_rng(0);
    let mut no_origin = LootContext::new(&mut rng).with_environment_attributes(&FixedAttributes);
    assert_eq!(provider.get_int_with_context(&mut no_origin), 0);

    let mut rng = seeded_rng(0);
    let mut bare = LootContext::new(&mut rng).with_origin(0.0, 64.0, 0.0);
    assert_eq!(provider.get_int_with_context(&mut bare), 0);
}
