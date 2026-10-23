//! Vanilla `PhantomSpawner`: spawns phantom groups above players who have not
//! slept for long enough during a dark night.

use std::sync::Arc;

use steel_registry::RegistryExt as _;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_custom_stats;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_rules::SPAWN_PHANTOMS;
use steel_utils::random::{Random as _, RandomSource};
use steel_utils::types::Difficulty;

use crate::entity::Entity as _;
use crate::entity::{EntitySpawnReason, create_entity_instance};
use crate::world::World;
use crate::world::custom_spawner::CustomSpawner;
use crate::world::spawn::natural::runtime_random;
use crate::world::spawn::placement::is_valid_empty_spawn_block;

/// Vanilla `PhantomSpawner` tick interval bounds.
const TICK_DELAY_BASE: i32 = 60;
/// Vanilla `PhantomSpawner` random extra ticks.
const TICK_DELAY_SPREAD: i32 = 60;
/// Vanilla `PhantomSpawner` sky-darken gate.
const SKY_DARKEN_THRESHOLD: u8 = 5;
/// Vanilla `PhantomSpawner` `TIME_SINCE_REST` gate.
const TIME_SINCE_REST_THRESHOLD: i32 = 72_000;

/// Vanilla `PhantomSpawner`.
#[derive(Default)]
pub struct PhantomSpawner {
    next_tick: i32,
}

impl PhantomSpawner {
    /// Creates the spawner with vanilla's default counters.
    #[must_use]
    pub const fn new() -> Self {
        Self { next_tick: 0 }
    }

    /// Vanilla `PhantomSpawner.tick`.
    pub fn tick_with_settings(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if !spawn_enemies || !world.get_game_rule(&SPAWN_PHANTOMS) {
            return;
        }

        let mut random = runtime_random();
        self.next_tick -= 1;
        if self.next_tick > 0 {
            return;
        }

        self.next_tick += (TICK_DELAY_BASE + random.next_i32_bounded(TICK_DELAY_SPREAD)) * 20;
        if world.sky_darkening() < SKY_DARKEN_THRESHOLD && world.dimension_type.has_skylight {
            return;
        }

        let mut players = Vec::new();
        world.players.iter_players(|_, player| {
            players.push(Arc::clone(player));
            true
        });

        for player in players {
            if player.is_spectator() {
                continue;
            }

            let player_pos = player.block_position();
            if world.dimension_type.has_skylight
                && (player_pos.y() < world.sea_level || !world.can_see_sky(player_pos))
            {
                continue;
            }

            let difficulty = world.current_difficulty_at(player_pos);
            if !difficulty.is_harder_than(random.next_f32() * 3.0) {
                continue;
            }

            let time_since_rest = time_since_rest(&player);
            if random.next_i32_bounded(time_since_rest) < TIME_SINCE_REST_THRESHOLD {
                continue;
            }

            let spawn_pos = player_pos.offset(
                -10 + random.next_i32_bounded(21),
                20 + random.next_i32_bounded(15),
                -10 + random.next_i32_bounded(21),
            );
            let state = world.get_block_state(spawn_pos);
            if !is_valid_empty_spawn_block(world, spawn_pos, state, &vanilla_entities::PHANTOM) {
                continue;
            }

            let mut group_data = None;
            let group_size = 1 + random.next_i32_bounded(difficulty.difficulty().id() + 1);
            for _ in 0..group_size {
                let Ok(phantom) = create_entity_instance(
                    world,
                    &vanilla_entities::PHANTOM,
                    glam::DVec3::new(
                        f64::from(spawn_pos.x()) + 0.5,
                        f64::from(spawn_pos.y()),
                        f64::from(spawn_pos.z()) + 0.5,
                    ),
                ) else {
                    continue;
                };

                phantom.snap_to(
                    glam::DVec3::new(
                        f64::from(spawn_pos.x()) + 0.5,
                        f64::from(spawn_pos.y()),
                        f64::from(spawn_pos.z()) + 0.5,
                    ),
                    0.0,
                    0.0,
                );
                if let Some(mob) = phantom.as_mob() {
                    group_data = mob.finalize_spawn(world, EntitySpawnReason::Natural, group_data);
                }
                let _ = world.try_add_entity(phantom);
            }
        }
    }
}

impl CustomSpawner for PhantomSpawner {
    fn tick(&mut self, world: &Arc<World>) {
        self.tick_with_settings(world, world.is_spawning_monsters());
    }

    fn tick_with_settings(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        Self::tick_with_settings(self, world, spawn_enemies);
    }
}

/// Vanilla `Player.getStats().getValue(Stats.TIME_SINCE_REST)`.
fn time_since_rest(player: &crate::player::Player) -> i32 {
    let stat = vanilla_stat_types::CUSTOM.get(&vanilla_custom_stats::TIME_SINCE_REST);
    player
        .stats()
        .into_iter()
        .find(|(candidate, _)| *candidate == stat)
        .map_or(0, |(_, value)| value)
        .clamp(1, i32::MAX)
}

/// Vanilla `Difficulty.getId`.
trait DifficultyId {
    fn id(self) -> i32;
}

impl DifficultyId for Difficulty {
    fn id(self) -> i32 {
        i32::from(u8::from(self))
    }
}
