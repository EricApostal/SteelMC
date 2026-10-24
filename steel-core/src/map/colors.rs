//! Vanilla `MapItem.update`: the terrain color rasterizer behind every map.
//!
//! One map pixel covers a `scale x scale` block area; the pixel's color is the
//! most common `MapColor` of that area's surface blocks (water columns shade by
//! depth instead) and its brightness comes from the height difference to the
//! previous pixel in the same column. Sampling is dithered by the viewer's step
//! counter so a map only rasterizes one sixteenth of its window per tick.

use glam::DVec3;
use steel_registry::blocks::{MapColor, MapColorBrightness, block_state_ext::BlockStateExt as _};
use steel_registry::vanilla_blocks;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Direction};

use crate::chunk::heightmap::HeightmapType;
use crate::world::World;
use crate::world::level_reader::LevelReader as _;

use super::data::{HALF_MAP_SIZE, MapItemSavedData};

/// Vanilla `MapItem.update`: samples the world around one viewer into `data`.
///
/// `viewer` is the carrying player's position and `step` is vanilla
/// `HoldingPlayer.step`.
pub(crate) fn update_colors(data: &mut MapItemSavedData, level: &World, viewer: DVec3, step: u32) {
    if level.key != data.dimension {
        return;
    }
    let scale = data.scale_factor();
    let center_x = data.center_x;
    let center_z = data.center_z;
    let has_ceiling = level.dimension_type.has_ceiling;
    let player_img_x = (viewer.x.floor() as i32 - center_x) / scale + HALF_MAP_SIZE;
    let player_img_y = (viewer.z.floor() as i32 - center_z) / scale + HALF_MAP_SIZE;
    let radius = if has_ceiling {
        128 / scale / 2
    } else {
        128 / scale
    };

    let mut found_consecutive_changes = false;
    for img_x in player_img_x - radius + 1..player_img_x + radius {
        if (img_x & 15) != (step as i32 & 15) && !found_consecutive_changes {
            continue;
        }
        found_consecutive_changes = false;
        let mut previous_average_area_height = 0.0_f64;
        for img_y in player_img_y - radius - 1..player_img_y + radius {
            if !(0..128).contains(&img_x) || !(-1..128).contains(&img_y) {
                continue;
            }
            let distance_to_player_sqr =
                (img_x - player_img_x).pow(2) + (img_y - player_img_y).pow(2);
            let dither_black = distance_to_player_sqr > (radius - 2) * (radius - 2);
            let area_min_x = (center_x / scale + img_x - 64) * scale;
            let area_min_z = (center_z / scale + img_y - 64) * scale;
            // Vanilla loads the chunk here; a map only rasterizes chunks the
            // server already holds at Full status, which for a carried map is
            // the viewer's own surroundings.
            let Some((counts, average_area_height, water_depth)) =
                sample_area(level, area_min_x, area_min_z, scale, has_ceiling)
            else {
                continue;
            };
            if !has_ceiling {
                for delta_x in 0..scale {
                    for delta_z in 0..scale {
                        data.check_banners(level, area_min_x + delta_x, area_min_z + delta_z);
                    }
                }
            }
            let color = most_common(&counts);
            let brightness = if color == MapColor::WATER {
                let depth =
                    f64::from(water_depth) * 0.1 + f64::from(((img_x + img_y) & 1) as u8) * 0.2;
                if depth < 0.5 {
                    MapColorBrightness::High
                } else if depth > 0.9 {
                    MapColorBrightness::Low
                } else {
                    MapColorBrightness::Normal
                }
            } else {
                let slope = (average_area_height - previous_average_area_height) * 4.0
                    / f64::from(scale + 4)
                    + (f64::from(((img_x + img_y) & 1) as u8) - 0.5) * 0.4;
                if slope > 0.6 {
                    MapColorBrightness::High
                } else if slope < -0.6 {
                    MapColorBrightness::Low
                } else {
                    MapColorBrightness::Normal
                }
            };
            previous_average_area_height = average_area_height;
            if img_y >= 0
                && distance_to_player_sqr < radius * radius
                && (!dither_black || ((img_x + img_y) & 1) != 0)
            {
                found_consecutive_changes |=
                    data.update_color(img_x as u8, img_y as u8, color.packed_id(brightness));
            }
        }
    }
}

/// Samples one map pixel's block area, returning its color multiset, average
/// surface height and water depth.
fn sample_area(
    level: &World,
    area_min_x: i32,
    area_min_z: i32,
    scale: i32,
    has_ceiling: bool,
) -> Option<(Vec<(MapColor, u32)>, f64, i32)> {
    let chunk_pos = ChunkPos::new(area_min_x >> 4, area_min_z >> 4);
    let min_y = level.min_y();
    level.chunk_map.with_full_chunk(chunk_pos, |chunk| {
        let mut counts: Vec<(MapColor, u32)> = Vec::new();
        if has_ceiling {
            // Vanilla shades ceiling dimensions from a hash of the pixel's
            // origin instead of sampling blocks, because the roof is not
            // walkable.
            let mut noise = area_min_x.wrapping_add(area_min_z.wrapping_mul(231_871));
            noise = noise
                .wrapping_mul(noise)
                .wrapping_mul(31_287_121)
                .wrapping_add(noise.wrapping_mul(11));
            let (block, amount) = if (noise >> 20) & 1 == 0 {
                (&vanilla_blocks::DIRT, 10)
            } else {
                (&vanilla_blocks::STONE, 100)
            };
            add_count(&mut counts, block.default_state().map_color(), amount);
            return (counts, 100.0, 0);
        }

        let mut water_depth = 0;
        let mut average_area_height = 0.0_f64;
        for delta_x in 0..scale {
            for delta_z in 0..scale {
                let x = area_min_x + delta_x;
                let z = area_min_z + delta_z;
                let (column_y, mut state) = surface_state(chunk, x, z, min_y);
                if column_y > min_y && !state.get_fluid_state().is_empty() {
                    let mut solid_y = column_y - 1;
                    loop {
                        let below = chunk.get_block_state(BlockPos::new(x, solid_y, z));
                        solid_y -= 1;
                        water_depth += 1;
                        if solid_y <= min_y || below.get_fluid_state().is_empty() {
                            break;
                        }
                    }
                    state =
                        correct_state_for_fluid_block(level, state, BlockPos::new(x, column_y, z));
                }
                average_area_height += f64::from(column_y) / f64::from(scale * scale);
                add_count(&mut counts, state.map_color(), 1);
            }
        }
        (counts, average_area_height, water_depth / (scale * scale))
    })
}

/// Vanilla's column walk: the first non-`NONE` map color below the world
/// surface, or bedrock outside the build height.
fn surface_state(
    chunk: crate::chunk::full_chunk::FullChunkRef<'_>,
    x: i32,
    z: i32,
    min_y: i32,
) -> (i32, BlockStateId) {
    // Vanilla `chunk.getHeight(...) + 1` with `getHeight` returning
    // `getFirstAvailable() - 1`; Steel's `get_height` is `getFirstAvailable`.
    let mut column_y = chunk.get_height(
        HeightmapType::WorldSurface,
        (x & 15) as usize,
        (z & 15) as usize,
    );
    if column_y <= min_y {
        return (column_y, vanilla_blocks::BEDROCK.default_state());
    }
    loop {
        column_y -= 1;
        let state = chunk.get_block_state(BlockPos::new(x, column_y, z));
        if state.map_color() != MapColor::NONE || column_y <= min_y {
            return (column_y, state);
        }
    }
}

/// Vanilla `MapItem.getCorrectStateForFluidBlock`: an open fluid column renders
/// as the fluid's own block when the block above it is not sturdy.
fn correct_state_for_fluid_block(
    level: &World,
    state: BlockStateId,
    pos: BlockPos,
) -> BlockStateId {
    let fluid_state = state.get_fluid_state();
    if !fluid_state.is_empty() && !level.is_face_sturdy(state, pos, Direction::Up) {
        fluid_state.create_legacy_block()
    } else {
        state
    }
}

/// Adds `amount` to the color's bucket, keeping first-seen insertion order.
fn add_count(counts: &mut Vec<(MapColor, u32)>, color: MapColor, amount: u32) {
    if let Some(entry) = counts.iter_mut().find(|(existing, _)| *existing == color) {
        entry.1 += amount;
    } else {
        counts.push((color, amount));
    }
}

/// Vanilla `Iterables.getFirst(Multisets.copyHighestCountFirst(counts), NONE)`:
/// the most frequent color, ties resolved towards the first sampled color.
fn most_common(counts: &[(MapColor, u32)]) -> MapColor {
    let mut best: Option<(MapColor, u32)> = None;
    for (color, amount) in counts {
        if best.is_none_or(|(_, best_amount)| *amount > best_amount) {
            best = Some((*color, *amount));
        }
    }
    best.map_or(MapColor::NONE, |(color, _)| color)
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use steel_registry::blocks::MapColor;
    use steel_registry::init_vanilla_registry;
    use steel_registry::vanilla_blocks;
    use steel_utils::{BlockPos, ChunkPos};

    use crate::map::MapItemSavedData;
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
    use steel_utils::types::UpdateFlags;

    /// Rasterizing a stone platform must paint the pixel under the viewer with
    /// the stone palette entry, so a filled map shows terrain colors.
    #[test]
    fn terrain_rasterizer_paints_surface_blocks() {
        init_vanilla_registry();
        let fixture = fresh_test_world("map_rasterizer_colors");
        let world = &fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let stone = vanilla_blocks::STONE.default_state();
        for x in 0..16 {
            for z in 0..16 {
                assert!(world.set_block(BlockPos::new(x, 63, z), stone, UpdateFlags::UPDATE_ALL));
            }
        }

        let mut data = MapItemSavedData::create_fresh(8.0, 8.0, 0, true, false, world.key.clone());
        // The map is centered on (0, 0), so the viewer at (8, 8) sits on pixel
        // (72, 72); `step` selects which pixel columns this tick samples.
        super::update_colors(&mut data, world, DVec3::new(8.0, 70.0, 8.0), 8);

        let pixel = data.colors[72 * super::super::MAP_SIZE + 72];
        assert_eq!(
            pixel >> 2,
            MapColor::STONE.id(),
            "the viewer's pixel should be painted with the stone palette entry"
        );
    }
}
