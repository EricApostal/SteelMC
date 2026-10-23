//! Vanilla `SpawnTargetPoint` / `NoiseSpawnFinder` support.
//!
//! A noise settings JSON may declare a `spawn_target` list. Each entry maps a
//! subset of the noise router's climate functions to the range their sampled
//! value must fall in. Vanilla's `NoiseBasedChunkGenerator.getOrigin` turns the
//! best-fitting position into the dimension origin chunk, which is what
//! `minecraft:dimension_origin` structure placement and the spawn search use.
//!
//! Unlike a biome `ParameterPoint`, a spawn target only constrains the
//! parameters it lists: vanilla `SpawnTargetPoint.sampleFitness` sums the squared
//! distances of the present entries only (there is no depth or offset term).
//! The per-dimension `spawn_target` entries themselves are generated from the
//! datapack (`<dimension>::SPAWN_TARGETS`), so this module only owns the
//! algorithm and the per-entry fitness rule.

use std::f32::consts::TAU;

use steel_utils::BlockPos;
use steel_utils::climate::{Parameter, TargetPoint};

/// Vanilla `SpawnTargetPoint.sampleFitness`: the sum of squared distances of the
/// listed parameters, `0` when every listed parameter matches.
///
/// `temperature`/`vegetation`/`continents`/`erosion`/`depth`/`ridges` are the
/// optional ranges of one generated `spawn_target` entry; `None` means the JSON
/// entry did not constrain that parameter.
#[must_use]
pub fn spawn_target_fitness(
    temperature: Option<Parameter>,
    vegetation: Option<Parameter>,
    continents: Option<Parameter>,
    erosion: Option<Parameter>,
    depth: Option<Parameter>,
    ridges: Option<Parameter>,
    target: &TargetPoint,
) -> i64 {
    let mut fitness = 0_i64;
    for (parameter, value) in [
        (temperature, target.temperature),
        (vegetation, target.humidity),
        (continents, target.continentalness),
        (erosion, target.erosion),
        (depth, target.depth),
        (ridges, target.weirdness),
    ] {
        if let Some(parameter) = parameter {
            let distance = parameter.distance(value);
            fitness += distance * distance;
        }
    }
    fitness
}

/// Vanilla `NoiseSpawnFinder.Result`.
#[derive(Debug, Clone, Copy)]
struct SpawnSearchResult {
    pos: BlockPos,
    fitness: i64,
}

/// Vanilla `NoiseSpawnFinder.findSpawnPosition`.
///
/// `fitness` returns one `spawn_target` entry's `SpawnTargetPoint.sampleFitness`
/// for a sampled climate target (see [`spawn_target_fitness`]); `sample` returns
/// the climate target at a block position. `sample` is only ever called with
/// block coordinates rounded down to the quart grid and `y = 0`, matching
/// `QuartPos.toBlock(QuartPos.fromBlock(..))`.
///
/// The per-dimension `spawn_target` entries come from the generated datapack
/// modules (`<dimension>::SPAWN_TARGETS`), so this stays generic over their
/// generated `SpawnTarget` type. An empty list yields the world origin,
/// matching vanilla's `ChunkGenerator.getOrigin` fallback (`ChunkPos.ZERO`).
#[must_use]
pub fn find_spawn_position<T>(
    targets: &[T],
    fitness: impl Fn(&T, &TargetPoint) -> i64,
    sample: impl Fn(i32, i32) -> TargetPoint,
) -> BlockPos {
    if targets.is_empty() {
        return BlockPos::new(0, 0, 0);
    }

    let min_fitness = |target: &TargetPoint| {
        targets
            .iter()
            .map(|entry| fitness(entry, target))
            .min()
            .unwrap_or(i64::MAX)
    };

    let mut result = spawn_position_and_fitness(&min_fitness, &sample, 0, 0);
    result = radial_search(&min_fitness, &sample, result, 2048.0, 512.0);
    radial_search(&min_fitness, &sample, result, 512.0, 32.0).pos
}

/// Vanilla `NoiseSpawnFinder.radialSearch`.
fn radial_search(
    fitness: &impl Fn(&TargetPoint) -> i64,
    sample: &impl Fn(i32, i32) -> TargetPoint,
    mut result: SpawnSearchResult,
    max_radius: f32,
    radius_increment: f32,
) -> SpawnSearchResult {
    let mut angle = 0.0_f32;
    let mut radius = radius_increment;
    let origin = result.pos;

    while radius <= max_radius {
        let x = origin.x() + (angle.sin() * radius) as i32;
        let z = origin.z() + (angle.cos() * radius) as i32;
        let candidate = spawn_position_and_fitness(fitness, sample, x, z);
        if candidate.fitness < result.fitness {
            result = candidate;
        }

        angle += radius_increment / radius;
        if angle > TAU {
            angle = 0.0;
            radius += radius_increment;
        }
    }

    result
}

/// Vanilla `NoiseSpawnFinder.getSpawnPositionAndFitness`.
fn spawn_position_and_fitness(
    fitness: &impl Fn(&TargetPoint) -> i64,
    sample: &impl Fn(i32, i32) -> TargetPoint,
    block_x: i32,
    block_z: i32,
) -> SpawnSearchResult {
    // `QuartPos.toBlock(QuartPos.fromBlock(...))` — round down to the quart grid.
    let quart_block_x = (block_x >> 2) << 2;
    let quart_block_z = (block_z >> 2) << 2;
    let target = sample(quart_block_x, quart_block_z);

    let min_fitness = fitness(&target);
    let distance_bias =
        i64::from(block_x) * i64::from(block_x) + i64::from(block_z) * i64::from(block_z);

    SpawnSearchResult {
        pos: BlockPos::new(block_x, 0, block_z),
        fitness: min_fitness * 2048_i64 * 2048_i64 + distance_bias,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point() -> TargetPoint {
        TargetPoint::new(0, 0, 0, 0, 0, 0)
    }

    #[test]
    fn absent_parameters_do_not_contribute() {
        assert_eq!(
            spawn_target_fitness(None, None, None, None, None, None, &point()),
            0
        );
    }

    #[test]
    fn only_listed_parameters_contribute() {
        // Ridges span [0.16, 1.0]; a target of 0 is below the range, so that is
        // the only contributing term even though depth is unconstrained.
        let ridges = Some(Parameter::span(0.16, 1.0));
        let distance = Parameter::span(0.16, 1.0).distance(0);
        assert_eq!(
            spawn_target_fitness(None, None, None, None, None, ridges, &point()),
            distance * distance
        );
    }

    #[test]
    fn empty_targets_use_the_world_origin() {
        assert_eq!(
            find_spawn_position(&[] as &[()], |(), _| 0, |_, _| point()),
            BlockPos::new(0, 0, 0)
        );
    }

    #[test]
    fn perfect_fit_prefers_the_world_origin() {
        // Every candidate fits equally, so the distance bias keeps (0, 0).
        assert_eq!(
            find_spawn_position(&[()], |(), _| 0, |_, _| point()),
            BlockPos::new(0, 0, 0)
        );
    }
}
