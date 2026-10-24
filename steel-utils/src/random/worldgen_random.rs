use crate::random::{
    Random, RandomSource, RandomSplitter, gaussian::MarsagliaPolarGaussian, xoroshiro::Xoroshiro,
};

/// Vanilla's `WorldgenRandom`.
///
/// Vanilla `WorldgenRandom extends LegacyRandomSource` and delegates every draw
/// to a wrapped `RandomSource` (`WorldgenRandom.fork()`,
/// `WorldgenRandom.next(I)I`, `WorldgenRandom.setSeed(J)V` bytecode). Its
/// `next(int)` even branches on `delegate instanceof LegacyRandomSource` and
/// keeps that source's own `LegacyRandomSource.next(int)` path; every other
/// delegate falls back to `nextLong() >>> (64 - bits)`. The gaussian cache
/// lives on the wrapper, not on the delegate: `MarsagliaPolarGaussian` is
/// inherited from `LegacyRandomSource`.
///
/// Feature decoration constructs `WorldgenRandom(new XoroshiroRandomSource(...))`,
/// so sampling goes through `BitRandomSource.next*` and does not match a raw
/// `XoroshiroRandomSource` for `nextInt`, bounded ints, doubles, longs, or
/// gaussians. `/place template` and `/place structure` instead hand the template
/// and structure-piece layer the legacy sources vanilla gives those commands
/// (`StructureBlockEntity.createRandom(seed)` and `Level.getRandom()`), which is
/// why the delegate is the enum and not a concrete source.
#[derive(Clone)]
pub struct WorldgenRandom {
    source: RandomSource,
    next_gaussian: Option<f64>,
}

impl WorldgenRandom {
    /// Creates a new `WorldgenRandom` backed by vanilla's `XoroshiroRandomSource`.
    #[must_use]
    pub const fn from_seed(seed: u64) -> Self {
        Self {
            source: RandomSource::Xoroshiro(Xoroshiro::from_seed(seed)),
            next_gaussian: None,
        }
    }

    /// Wraps an existing source, mirroring vanilla `new WorldgenRandom(RandomSource)`.
    #[must_use]
    pub const fn from_source(source: RandomSource) -> Self {
        Self {
            source,
            next_gaussian: None,
        }
    }

    /// Re-seeds the wrapped source.
    ///
    /// Vanilla `WorldgenRandom` inherits its gaussian cache from
    /// `LegacyRandomSource`, but overrides `setSeed` to only reseed the
    /// wrapped source. That means `setDecorationSeed` / `setFeatureSeed`
    /// intentionally preserve a pending gaussian value.
    pub const fn set_seed(&mut self, seed: i64) {
        match &mut self.source {
            RandomSource::Xoroshiro(source) => source.set_seed(seed),
            RandomSource::Legacy(source) => source.set_seed(seed),
        }
    }

    /// Vanilla's `WorldgenRandom.setDecorationSeed`.
    pub fn set_decoration_seed(&mut self, seed: i64, block_x: i32, block_z: i32) -> i64 {
        self.set_seed(seed);
        let x_scale = self.next_i64() | 1;
        let z_scale = self.next_i64() | 1;
        let decoration_seed = i64::from(block_x)
            .wrapping_mul(x_scale)
            .wrapping_add(i64::from(block_z).wrapping_mul(z_scale))
            ^ seed;
        self.set_seed(decoration_seed);
        decoration_seed
    }

    /// Vanilla's `WorldgenRandom.setFeatureSeed`.
    pub const fn set_feature_seed(&mut self, decoration_seed: i64, feature_index: i32, step: i32) {
        let feature_seed = decoration_seed
            .wrapping_add(feature_index as i64)
            .wrapping_add(10_000_i64.wrapping_mul(step as i64));
        self.set_seed(feature_seed);
    }

    /// Vanilla `WorldgenRandom.next(int)`.
    fn next_bits(&mut self, bits: u64) -> u64 {
        if matches!(self.source, RandomSource::Legacy(_)) {
            // Vanilla keeps the delegate's `LegacyRandomSource.next(int)` path.
            // That path logical-shifts the 48-bit LCG state and truncates, so it
            // must be recovered from `next(32)` with a logical shift, not an
            // arithmetic one (`next(32) >> 1` sign-extends).
            return u64::from(self.source.next_i32() as u32 >> (32 - bits));
        }
        self.source.next_i64() as u64 >> (64 - bits)
    }
}

impl MarsagliaPolarGaussian for WorldgenRandom {
    fn stored_next_gaussian(&self) -> Option<f64> {
        self.next_gaussian
    }

    fn set_stored_next_gaussian(&mut self, value: Option<f64>) {
        self.next_gaussian = value;
    }
}

impl Random for WorldgenRandom {
    fn fork(&mut self) -> Self {
        Self {
            source: self.source.fork(),
            next_gaussian: None,
        }
    }

    fn next_i32(&mut self) -> i32 {
        self.next_bits(32) as i32
    }

    fn next_i32_bounded(&mut self, bound: i32) -> i32 {
        if bound & bound.wrapping_sub(1) == 0 {
            (i64::from(bound).wrapping_mul(i64::from(self.next_bits(31) as i32)) >> 31) as i32
        } else {
            loop {
                let sample = self.next_bits(31) as i32;
                let modulo = sample % bound;
                if sample
                    .wrapping_sub(modulo)
                    .wrapping_add(bound.wrapping_sub(1))
                    >= 0
                {
                    return modulo;
                }
            }
        }
    }

    fn next_i64(&mut self) -> i64 {
        let upper = self.next_i32();
        let lower = self.next_i32();
        (i64::from(upper) << 32).wrapping_add(i64::from(lower))
    }

    fn next_f32(&mut self) -> f32 {
        self.next_bits(24) as f32 * 5.960_464_5e-8_f32
    }

    fn next_f64(&mut self) -> f64 {
        let combined = ((self.next_bits(26) as i64) << 27) + self.next_bits(27) as i64;
        combined as f64 * (1.0 / (1_i64 << 53) as f64)
    }

    fn next_bool(&mut self) -> bool {
        self.next_bits(1) != 0
    }

    fn next_gaussian(&mut self) -> f64 {
        self.calculate_gaussian()
    }

    fn next_positional(&mut self) -> RandomSplitter {
        self.source.next_positional()
    }
}

#[cfg(test)]
mod tests {
    use super::WorldgenRandom;
    use crate::random::legacy_random::LegacyRandom;
    use crate::random::xoroshiro::Xoroshiro;
    use crate::random::{Random, RandomSource};

    #[test]
    fn set_decoration_seed_matches_vanilla_trace() {
        let mut random = WorldgenRandom::from_seed(0);
        assert_eq!(
            random.set_decoration_seed(13_579, -6_695_392, 5_868_656),
            7_632_291_757_650_236_667,
        );
    }

    #[test]
    fn feature_seed_matches_vanilla_first_ore_dirt_origin() {
        let mut random = WorldgenRandom::from_seed(0);
        let decoration_seed = random.set_decoration_seed(13_579, -6_695_392, 5_868_656);
        random.set_feature_seed(decoration_seed, 0, 6);

        let x = -6_695_392 + random.next_i32_bounded(16);
        let z = 5_868_656 + random.next_i32_bounded(16);
        let y = random.next_i32_bounded(161);
        assert_eq!((x, y, z), (-6_695_386, 149, 5_868_662));
    }

    #[test]
    fn feature_seed_preserves_pending_gaussian() {
        let mut random = WorldgenRandom::from_seed(123);
        let _ = random.next_gaussian();
        random.set_feature_seed(456, 7, 8);

        let mut cached_reference = WorldgenRandom::from_seed(123);
        let _ = cached_reference.next_gaussian();
        assert_eq!(random.next_gaussian(), cached_reference.next_gaussian());

        let mut reseeded_reference = WorldgenRandom::from_seed(0);
        reseeded_reference.set_feature_seed(456, 7, 8);
        assert_eq!(random.next_gaussian(), reseeded_reference.next_gaussian());
    }

    #[test]
    fn legacy_delegate_matches_bare_legacy_draws() {
        // Vanilla `WorldgenRandom.next(int)` keeps the delegate's
        // `LegacyRandomSource.next(int)` path, so wrapping a legacy source must
        // draw exactly like the source would on its own.
        let seed = 0x1234_5678_ABCD_EF01u64;
        let mut bare = RandomSource::Legacy(LegacyRandom::from_seed(seed));
        let mut wrapped =
            WorldgenRandom::from_source(RandomSource::Legacy(LegacyRandom::from_seed(seed)));
        for _ in 0..200 {
            assert_eq!(bare.next_i32(), wrapped.next_i32());
            assert_eq!(bare.next_i32_bounded(37), wrapped.next_i32_bounded(37));
            assert_eq!(bare.next_i64(), wrapped.next_i64());
            assert_eq!(bare.next_f32(), wrapped.next_f32());
            assert_eq!(bare.next_f64(), wrapped.next_f64());
            assert_eq!(bare.next_bool(), wrapped.next_bool());
            assert_eq!(bare.next_gaussian(), wrapped.next_gaussian());
        }
    }

    #[test]
    fn xoroshiro_delegate_matches_from_seed() {
        // `from_seed` must keep producing the exact stream decoration code relies on.
        let mut seeded = WorldgenRandom::from_seed(987_654_321);
        let mut wrapped =
            WorldgenRandom::from_source(RandomSource::Xoroshiro(Xoroshiro::from_seed(
                987_654_321,
            )));
        for _ in 0..200 {
            assert_eq!(seeded.next_i32(), wrapped.next_i32());
            assert_eq!(seeded.next_i32_bounded(37), wrapped.next_i32_bounded(37));
            assert_eq!(seeded.next_i64(), wrapped.next_i64());
            assert_eq!(seeded.next_f32(), wrapped.next_f32());
            assert_eq!(seeded.next_f64(), wrapped.next_f64());
            assert_eq!(seeded.next_bool(), wrapped.next_bool());
            assert_eq!(seeded.next_gaussian(), wrapped.next_gaussian());
        }
    }
}
