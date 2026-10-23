use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::loot_table::LootCondition;

/// A `predicate` entry from a data pack JSON file.
///
/// Vanilla registers these as `Registries.PREDICATE`, a resource registry of
/// `LootItemCondition`s that other data pack files reference by id.
#[derive(Debug)]
pub struct Predicate {
    /// The `predicate` key.
    pub key: Identifier,
    pub condition: LootCondition,
}

/// A reference to a registered `predicate`.
pub type PredicateRef = &'static Predicate;

/// Registry for `predicate` conditions.
pub struct PredicateRegistry {
    predicates_by_id: Vec<PredicateRef>,
    predicates_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl PredicateRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            predicates_by_id: Vec::new(),
            predicates_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    PredicateRegistry,
    PredicateRef,
    predicates_by_id,
    predicates_by_key,
    allows_registering
);

crate::impl_registry!(
    PredicateRegistry,
    Predicate,
    predicates_by_id,
    predicates_by_key,
    predicates
);
