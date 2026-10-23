use rustc_hash::FxHashMap;
use steel_utils::Identifier;
use steel_utils::types::Difficulty;

/// A `test_environment` entry from a data pack JSON file.
///
/// Vanilla keeps the environment's serialized key separately from the polymorphic
/// definition, so the registry entry wraps the definition to carry its key.
#[derive(Debug)]
pub struct TestEnvironment {
    /// The `test_environment` key.
    pub key: Identifier,
    pub definition: TestEnvironmentDefinition,
}

/// The registered environment definition types of vanilla's
/// `TestEnvironmentDefinition` interface.
#[derive(Debug)]
pub enum TestEnvironmentDefinition {
    /// Applies nested definitions in order, tearing them down in reverse order.
    AllOf(TestEnvironmentAllOf),
    /// Moves a world clock to a fixed time, restoring the previous time on teardown.
    ClockTime(TestEnvironmentClockTime),
    /// Sets the server difficulty, restoring the previous one on teardown.
    Difficulty(TestEnvironmentDifficulty),
    /// Runs command functions before and after the test.
    Functions(TestEnvironmentFunctions),
    /// Overrides game rule values, restoring the previous values on teardown.
    GameRules(TestEnvironmentGameRules),
    /// Adds timeline layers to the level's environment attributes.
    Timelines(TestEnvironmentTimelines),
    /// Forces a weather state for the duration of the test.
    Weather(TestEnvironmentWeather),
}

/// Vanilla `TestEnvironmentDefinition.AllOf`.
#[derive(Debug)]
pub struct TestEnvironmentAllOf {
    /// Nested definitions. Vanilla's codec accepts either an id reference or an
    /// inline definition for every element.
    pub definitions: &'static [TestEnvironmentHolder],
}

/// A `Holder<TestEnvironmentDefinition<?>>`: a reference to another
/// `test_environment` entry or an inline definition.
///
/// Steel resolves vanilla registry references while generating constants, but a
/// definition may also be given inline in the data pack JSON, so both shapes are
/// preserved.
#[derive(Debug)]
pub enum TestEnvironmentHolder {
    /// A reference by `test_environment` key.
    Reference(Identifier),
    /// A definition written inline in the JSON.
    Inline(&'static TestEnvironmentDefinition),
}

/// Vanilla `TestEnvironmentDefinition.ClockTime`.
#[derive(Debug)]
pub struct TestEnvironmentClockTime {
    /// The `world_clock` key to move.
    pub clock: Identifier,
    /// The non-negative total tick value to set.
    pub time: i32,
}

/// Vanilla `TestEnvironmentDefinition.SetDifficulty`.
#[derive(Debug)]
pub struct TestEnvironmentDifficulty {
    pub difficulty: Difficulty,
}

/// Vanilla `TestEnvironmentDefinition.Functions`.
#[derive(Debug)]
pub struct TestEnvironmentFunctions {
    /// Function run before the test starts, by minecraft-function id.
    pub setup_function: Option<Identifier>,
    /// Function run after the test ends, by minecraft-function id.
    pub teardown_function: Option<Identifier>,
}

/// Vanilla `TestEnvironmentDefinition.SetGameRules`.
#[derive(Debug)]
pub struct TestEnvironmentGameRules {
    /// Rule overrides. Vanilla stores a `GameRuleMap`, which is keyed by game rule
    /// and holds typed values; generated data stores the same pairs as literals.
    pub rules: &'static [GameRuleSetting],
}

/// One entry of a game rule override map.
#[derive(Debug)]
pub struct GameRuleSetting {
    /// The game rule key.
    pub rule: Identifier,
    /// The value to apply.
    pub value: GameRuleSettingValue,
}

/// A serialized game rule value (`bool` or `int` in vanilla's game rule codec).
#[derive(Debug, Clone, Copy)]
pub enum GameRuleSettingValue {
    Bool(bool),
    Int(i32),
}

/// Vanilla `TestEnvironmentDefinition.Timelines`.
#[derive(Debug)]
pub struct TestEnvironmentTimelines {
    /// `timeline` keys to add as environment attribute layers.
    pub timelines: &'static [Identifier],
}

/// Vanilla `TestEnvironmentDefinition.Weather`.
#[derive(Debug)]
pub struct TestEnvironmentWeather {
    pub weather: WeatherType,
}

/// The weather state forced by a `weather` test environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeatherType {
    /// Clear weather for 100000 ticks.
    Clear,
    /// Rain for 100000 ticks.
    Rain,
    /// Rain and thunder for 100000 ticks.
    Thunder,
}

/// A reference to a registered `test_environment` definition.
pub type TestEnvironmentRef = &'static TestEnvironment;

/// Registry for `test_environment` definitions.
pub struct TestEnvironmentRegistry {
    test_environments_by_id: Vec<TestEnvironmentRef>,
    test_environments_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl TestEnvironmentRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            test_environments_by_id: Vec::new(),
            test_environments_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    TestEnvironmentRegistry,
    TestEnvironmentRef,
    test_environments_by_id,
    test_environments_by_key,
    allows_registering
);

crate::impl_registry!(
    TestEnvironmentRegistry,
    TestEnvironment,
    test_environments_by_id,
    test_environments_by_key,
    test_environments
);
