use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use rustc_hash::FxHashMap;
use steel_registry::biome::BiomeRef;
use steel_registry::feature::data::{PlacedFeatureData, PlacedFeatureRef};
use steel_registry::{Registry, RegistryEntry as _, RegistryExt as _};
use steel_utils::Identifier;

/// One placed feature inside a decoration stage.
///
/// Vanilla stage lists hold `Holder<PlacedFeature>` entries; inline placed features
/// (`PlacementUtils.inlinePlaced`, used by superflat's fill-layer features) have no
/// registry holder, so they are carried by value like vanilla's inline holders.
#[derive(Debug)]
pub(crate) enum StagedFeature {
    /// Registry-backed placed feature.
    Registry(Identifier),
    /// Inline placed feature with no registry entry.
    Inline(Box<PlacedFeatureData>),
}

impl StagedFeature {
    /// Creates a staged entry for a registry placed feature key.
    #[must_use]
    pub(crate) fn registry(key: Identifier) -> Self {
        Self::Registry(key)
    }
}

/// Cached vanilla ordering for all placed features reachable from a biome source.
#[derive(Debug)]
pub(super) struct FeatureSorter {
    steps: Box<[FeatureStepData]>,
}

#[derive(Debug)]
pub(super) struct FeatureStepData {
    features: Box<[PlacedFeatureRef]>,
    index_by_feature_id: FxHashMap<FeatureId, usize>,
    feature_indices_by_biome_id: FxHashMap<usize, Box<[usize]>>,
}

/// Identity of a placed feature inside one sorter build.
///
/// Vanilla keys its ordering graph by `PlacedFeature` reference identity; registry
/// entries map to their registry id and inline features get a per-build synthetic id,
/// which is equivalent because inline features are never shared between stages.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum FeatureId {
    Registry(usize),
    Inline(usize),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct FeatureVertex {
    step: usize,
    order: usize,
    feature: FeatureId,
}

impl Ord for FeatureVertex {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.step, self.order, self.feature).cmp(&(other.step, other.order, other.feature))
    }
}

impl PartialOrd for FeatureVertex {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A staged biome resolved to feature identities for one build.
struct ResolvedBiome<'a> {
    biome: BiomeRef,
    stages: &'a [Vec<FeatureId>],
}

impl FeatureSorter {
    /// Builds a sorter from explicit per-biome feature stage lists.
    ///
    /// Used by flat generators, which run vanilla's
    /// `FlatLevelGeneratorSettings.adjustGenerationSettings` semantics: the configured
    /// biome's feature stages with some generation steps dropped rather than the
    /// registry biome's own `features` lists. Each entry is `(biome, stages)` where
    /// `stages` mirrors `Biome.features` (ordered per decoration step).
    pub(super) fn build_with_stages(
        staged_biomes: &[(BiomeRef, &[Vec<StagedFeature>])],
        registry: &Registry,
    ) -> Self {
        let mut feature_order_by_id = FxHashMap::default();
        let mut next_feature_order = 0usize;
        let mut edges = BTreeMap::<FeatureVertex, BTreeSet<FeatureVertex>>::new();
        let mut inline_features = Vec::<Box<PlacedFeatureData>>::new();
        let mut owned_stages = Vec::<Vec<Vec<FeatureId>>>::with_capacity(staged_biomes.len());
        let mut resolved_biomes = Vec::with_capacity(staged_biomes.len());

        for (biome, stages) in staged_biomes {
            let mut biome_features = Vec::new();
            let mut resolved_stages = Vec::with_capacity(stages.len());

            for (step, feature_stage) in stages.iter().enumerate() {
                let mut resolved_stage = Vec::with_capacity(feature_stage.len());

                for staged_feature in feature_stage {
                    let feature_id = match staged_feature {
                        StagedFeature::Registry(feature_key) => {
                            let Some(placed_feature_id) =
                                registry.placed_features.id_from_key(feature_key)
                            else {
                                panic!(
                                    "biome {} references unknown placed feature {}",
                                    biome.key, feature_key
                                );
                            };
                            FeatureId::Registry(placed_feature_id)
                        }
                        StagedFeature::Inline(data) => {
                            let id = inline_features.len();
                            inline_features.push(data.clone());
                            FeatureId::Inline(id)
                        }
                    };

                    let feature_order = if let Some(&order) = feature_order_by_id.get(&feature_id) {
                        order
                    } else {
                        let order = next_feature_order;
                        next_feature_order += 1;
                        feature_order_by_id.insert(feature_id, order);
                        order
                    };

                    let vertex = FeatureVertex {
                        step,
                        order: feature_order,
                        feature: feature_id,
                    };
                    edges.entry(vertex).or_default();
                    biome_features.push(vertex);
                    resolved_stage.push(feature_id);
                }

                resolved_stages.push(resolved_stage);
            }

            for feature_pair in biome_features.windows(2) {
                edges
                    .entry(feature_pair[0])
                    .or_default()
                    .insert(feature_pair[1]);
            }

            owned_stages.push(resolved_stages);
            resolved_biomes.push((*biome, owned_stages.len() - 1));
        }

        let resolved: Vec<ResolvedBiome<'_>> = resolved_biomes
            .iter()
            .map(|&(biome, index)| ResolvedBiome {
                biome,
                stages: &owned_stages[index],
            })
            .collect();

        let sorted_features = Self::topological_sort(&edges);
        Self::from_sorted_features(&sorted_features, &resolved, &inline_features, registry)
    }

    #[must_use]
    pub(super) fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub(super) fn step(&self, step: usize) -> Option<&FeatureStepData> {
        self.steps.get(step)
    }

    fn topological_sort(
        edges: &BTreeMap<FeatureVertex, BTreeSet<FeatureVertex>>,
    ) -> Vec<FeatureVertex> {
        let mut sorted = Vec::with_capacity(edges.len());
        let mut discovered = BTreeSet::new();
        let mut visiting = BTreeSet::new();
        let vertices = edges.keys().copied().collect::<Vec<_>>();

        for vertex in vertices {
            assert!(
                !Self::visit(vertex, edges, &mut discovered, &mut visiting, &mut sorted),
                "biome decoration placed-feature order contains a cycle"
            );
        }

        sorted.reverse();
        sorted
    }

    fn visit(
        vertex: FeatureVertex,
        edges: &BTreeMap<FeatureVertex, BTreeSet<FeatureVertex>>,
        discovered: &mut BTreeSet<FeatureVertex>,
        visiting: &mut BTreeSet<FeatureVertex>,
        sorted: &mut Vec<FeatureVertex>,
    ) -> bool {
        if discovered.contains(&vertex) {
            return false;
        }
        if !visiting.insert(vertex) {
            return true;
        }

        if let Some(neighbors) = edges.get(&vertex) {
            for &neighbor in neighbors {
                if Self::visit(neighbor, edges, discovered, visiting, sorted) {
                    return true;
                }
            }
        }

        visiting.remove(&vertex);
        discovered.insert(vertex);
        sorted.push(vertex);
        false
    }

    fn from_sorted_features(
        sorted_features: &[FeatureVertex],
        resolved_biomes: &[ResolvedBiome<'_>],
        inline_features: &[Box<PlacedFeatureData>],
        registry: &Registry,
    ) -> Self {
        let Some(max_step) = sorted_features.iter().map(|feature| feature.step).max() else {
            return Self {
                steps: Box::new([]),
            };
        };

        let mut steps = Vec::with_capacity(max_step + 1);
        for step in 0..=max_step {
            let mut features = Vec::new();
            let mut index_by_feature_id = FxHashMap::default();

            for feature in sorted_features
                .iter()
                .filter(|feature| feature.step == step)
            {
                let placed_feature = match feature.feature {
                    FeatureId::Registry(placed_feature_id) => {
                        let Some(placed_feature) =
                            registry.placed_features.by_id(placed_feature_id)
                        else {
                            panic!(
                                "feature sorter references unknown placed feature id \
                                 {placed_feature_id}"
                            );
                        };
                        PlacedFeatureRef::Reference(placed_feature)
                    }
                    FeatureId::Inline(inline_id) => {
                        let Some(data) = inline_features.get(inline_id) else {
                            panic!("feature sorter references unknown inline feature {inline_id}");
                        };
                        PlacedFeatureRef::Inline(data.clone())
                    }
                };
                let index = features.len();
                features.push(placed_feature);
                index_by_feature_id.insert(feature.feature, index);
            }

            steps.push(FeatureStepData {
                features: features.into_boxed_slice(),
                index_by_feature_id,
                feature_indices_by_biome_id: FxHashMap::default(),
            });
        }

        for resolved in resolved_biomes {
            let Some(biome_id) = resolved.biome.try_id() else {
                panic!("possible biome {} is not registered", resolved.biome.key);
            };

            for (step, feature_stage) in resolved.stages.iter().enumerate() {
                let Some(step_data) = steps.get_mut(step) else {
                    continue;
                };

                let mut indices = Vec::with_capacity(feature_stage.len());
                for feature_id in feature_stage {
                    let Some(feature_index) = step_data.feature_index(*feature_id) else {
                        panic!(
                            "a feature from biome {} was not included in decoration step {step}",
                            resolved.biome.key
                        );
                    };
                    indices.push(feature_index);
                }

                if indices.is_empty() {
                    continue;
                }

                indices.sort_unstable();
                indices.dedup();
                step_data
                    .feature_indices_by_biome_id
                    .insert(biome_id, indices.into_boxed_slice());
            }
        }

        Self {
            steps: steps.into_boxed_slice(),
        }
    }
}

impl FeatureStepData {
    fn feature_index(&self, feature_id: FeatureId) -> Option<usize> {
        self.index_by_feature_id.get(&feature_id).copied()
    }

    pub(super) fn feature(&self, index: usize) -> Option<&PlacedFeatureRef> {
        self.features.get(index)
    }

    pub(super) fn feature_indices_for_biome(&self, biome_id: usize) -> Option<&[usize]> {
        self.feature_indices_by_biome_id
            .get(&biome_id)
            .map(Box::as_ref)
    }
}
