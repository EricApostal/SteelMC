use rustc_hash::FxHashMap;
use steel_utils::{Identifier, Rotation};

/// A `test_instance` entry from a data pack JSON file.
///
/// Vanilla's registry value is the polymorphic `GameTestInstance`; the registry
/// key lives outside it, so Steel pairs the two here.
#[derive(Debug)]
pub struct TestInstanceEntry {
    /// The `test_instance` key.
    pub key: Identifier,
    pub instance: TestInstance,
}

/// The registered `test_instance` types of vanilla's `GameTestInstance`.
#[derive(Debug)]
pub enum TestInstance {
    /// Runs a registered test function (vanilla `FunctionGameTestInstance`).
    Function(GameTestFunctionInstance),
    /// Runs the test blocks placed in the structure (vanilla
    /// `BlockBasedTestInstance`).
    BlockBased(GameTestBlockInstance),
}

/// The data shared by every test instance type (vanilla `TestData`).
#[derive(Debug)]
pub struct TestData {
    /// The `test_environment` key applied while the test runs. Vanilla stores a
    /// `Holder<TestEnvironmentDefinition<?>>`; a key is kept so the environment
    /// is resolved at run time.
    pub environment: Identifier,
    /// The dimension the test structure is placed in.
    pub dimension: Identifier,
    /// The structure template id, relative to `structure/`.
    pub structure: Identifier,
    /// Maximum ticks before the test times out; vanilla requires a positive value.
    pub max_ticks: i32,
    /// Ticks spent in setup before the test body runs.
    pub setup_ticks: i32,
    /// Whether a failure of this test fails the whole batch.
    pub required: bool,
    /// Rotation applied to the placed structure.
    pub rotation: Rotation,
    /// Whether the test only runs when explicitly requested.
    pub manual_only: bool,
    /// How many attempts are made before the test is reported as failed.
    pub max_attempts: i32,
    /// How many successful attempts are needed to pass.
    pub required_successes: i32,
    /// Whether the test structure is placed with sky access.
    pub sky_access: bool,
    /// Padding around the structure, in blocks (vanilla allows 0..=128).
    pub padding: i32,
}

/// Vanilla `FunctionGameTestInstance`.
#[derive(Debug)]
pub struct GameTestFunctionInstance {
    /// The test function id, relative to `test_function/`. Functions are loaded at
    /// run time, so this is not resolved while generating data.
    pub function: Identifier,
    pub info: TestData,
}

/// Vanilla `BlockBasedTestInstance`.
#[derive(Debug)]
pub struct GameTestBlockInstance {
    pub info: TestData,
}

/// A reference to a registered `test_instance` entry.
pub type TestInstanceRef = &'static TestInstanceEntry;

/// Registry for `test_instance` definitions.
pub struct TestInstanceRegistry {
    test_instances_by_id: Vec<TestInstanceRef>,
    test_instances_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl TestInstanceRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            test_instances_by_id: Vec::new(),
            test_instances_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    TestInstanceRegistry,
    TestInstanceRef,
    test_instances_by_id,
    test_instances_by_key,
    allows_registering
);

crate::impl_registry!(
    TestInstanceRegistry,
    TestInstanceEntry,
    test_instances_by_id,
    test_instances_by_key,
    test_instances
);
