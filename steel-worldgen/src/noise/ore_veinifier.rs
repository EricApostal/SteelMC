//! Ore vein generation during terrain fill.
//!
//! Matches vanilla's `OreVeinRule`, the ore-vein entries of the overworld
//! material rule. Vanilla has no `OreVeinifier` class any more: the vein
//! toggle, mask (ridged veins), edge roundoff, richness and the per-type
//! densities are ordinary density functions
//! (`overworld/ore_vein/{toggle,mask,richness,copper_density,iron_density,gap}`)
//! which the build script compiles straight from the datapack. This type owns
//! only the two things that are not density functions: the `minecraft:ore`
//! positional random factory and the final per-block block selection
//! (`OreVeinRule.compile`).

use steel_utils::BlockStateId;
use steel_utils::random::name_hash::NameHash;
use steel_utils::random::{PositionalRandom, Random, RandomSplitter};

/// Ore vein block selector.
///
/// Holds the `minecraft:ore` positional random factory, derived exactly like
/// vanilla's
/// `MaterialRuleContext.getOrCreateRandomFactory(Identifier.withDefaultNamespace("ore"))`.
pub struct OreVeinifier {
    ore_splitter: RandomSplitter,
}

impl OreVeinifier {
    /// Create a new ore veinifier from the seed's positional splitter.
    ///
    /// The `splitter` should be the same one used to create `OverworldNoises`
    /// (i.e. from `Xoroshiro::from_seed(seed).next_positional()`).
    #[must_use]
    pub fn new(splitter: &RandomSplitter) -> Self {
        const ORE_HASH: NameHash = NameHash::new("minecraft:ore");
        let mut ore_rng = splitter.with_hash_of(&ORE_HASH);
        let ore_splitter = ore_rng.next_positional();

        Self { ore_splitter }
    }

    /// Applies one `OreVeinRule` from the material-rule datapack.
    ///
    /// Mirrors the evaluator returned by vanilla's `OreVeinRule.compile`:
    ///
    /// 1. `density <= 0` → no vein block (vanilla returns `null`). `density` is
    ///    the per-type vein density, which is `0.7` when the y-range, mask,
    ///    toggle and edge-roundoff gates all pass and `-1` otherwise.
    /// 2. `random.nextFloat() > density` → no vein block (the 30 % solidness
    ///    roll).
    /// 3. `random.nextFloat() < richness && fillerGap < 0` → ore (or a raw ore
    ///    block with probability `raw_ore_chance`).
    /// 4. otherwise → the filler block (granite for copper, tuff for iron).
    ///
    /// `density` and `richness` are prefilled per block from the chunk volume
    /// (`MaterialRuleContext.getDensitiesInChunk(.., true)`); `filler_gap` is
    /// sampled lazily and only when the richness roll passes
    /// (`getDensitiesInChunk(.., false)` plus Java's short-circuit `&&`).
    #[expect(
        clippy::too_many_arguments,
        reason = "matches the Vanilla ore-vein rule inputs"
    )]
    #[must_use]
    pub fn try_apply_material_rule(
        &self,
        x: i32,
        y: i32,
        z: i32,
        density: f32,
        richness: f32,
        filler_gap: impl FnOnce() -> f32,
        ore: BlockStateId,
        raw_ore: BlockStateId,
        filler: BlockStateId,
        raw_ore_chance: f32,
    ) -> Option<BlockStateId> {
        if density <= 0.0 {
            return None;
        }

        let mut random = self.ore_splitter.at(x, y, z);
        if random.next_f32() > density {
            return None;
        }

        if random.next_f32() < richness && filler_gap() < 0.0 {
            if random.next_f32() < raw_ore_chance {
                Some(raw_ore)
            } else {
                Some(ore)
            }
        } else {
            Some(filler)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::OreVeinifier;
    use crate::random::{Random, xoroshiro::Xoroshiro};
    use steel_registry::{init_vanilla_registry, vanilla_blocks};

    #[test]
    fn rejected_material_rules_do_not_sample_filler_gap() {
        init_vanilla_registry();
        let splitter = Xoroshiro::from_seed(13579).next_positional();
        let veinifier = OreVeinifier::new(&splitter);
        let ore = vanilla_blocks::COPPER_ORE.default_state();
        let raw = vanilla_blocks::RAW_COPPER_BLOCK.default_state();
        let filler = vanilla_blocks::GRANITE.default_state();
        for (density, richness, expected) in [(0.0, 1.0, None), (1.0, 0.0, Some(filler))] {
            assert_eq!(
                veinifier.try_apply_material_rule(
                    12,
                    20,
                    -34,
                    density,
                    richness,
                    || panic!("Vanilla rejects before sampling the gap"),
                    ore,
                    raw,
                    filler,
                    0.02,
                ),
                expected,
            );
        }
    }
}
